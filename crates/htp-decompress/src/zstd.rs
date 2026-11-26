use std::ops::RangeInclusive;

use ruzstd::decoding::errors::{FrameDecoderError, FrameHeaderError};
use ruzstd::decoding::{BlockDecodingStrategy, FrameDecoder};

use crate::{Error, FormatDecoder, Limits, Outcome, Step};

const MAGIC: u32 = 0xFD2F_B528;
const LEGACY_MAGICS: RangeInclusive<u32> = 0xFD2F_B522..=0xFD2F_B527;
const SKIPPABLE_MAGICS: RangeInclusive<u32> = 0x184D_2A50..=0x184D_2A5F;

const DICTIONARY_ID_FLAG: u8 = 0x03;
const CHECKSUM_FLAG: u8 = 0x04;
const RESERVED_BIT: u8 = 0x08;
const SINGLE_SEGMENT_FLAG: u8 = 0x20;

const MAX_BLOCK_SIZE: usize = 131_072;

/// What the bytes in `pending` belong to.
#[derive(Clone, Copy)]
enum Unit {
    /// The magic number that starts every frame.
    Magic,
    /// A skippable frame's magic number and content length.
    SkipLength,
    /// A skippable frame's content, with this many bytes left. They are skipped, not buffered.
    Skip(usize),
    /// A frame's magic number and Frame_Header_Descriptor.
    Descriptor,
    /// The whole frame header.
    FrameHeader,
    /// A block header.
    BlockHeader,
    /// A block header and content, followed by the checksum after a frame's last block.
    Block { last: bool },
    /// The frame's last block is decoded.
    End,
}

pub(crate) struct ZstdDecoder {
    frame: FrameDecoder,
    unit: Unit,
    pending: Vec<u8>,
    /// How many bytes `pending` holds once the current unit is complete.
    need: usize,
    /// Whether the current frame ends with a content checksum.
    checksum: bool,
    /// Whether a frame has ended, which makes bytes that start no frame trailing data.
    after_frame: bool,
}

impl ZstdDecoder {
    pub(crate) fn new(limits: Limits) -> ZstdDecoder {
        let mut frame = FrameDecoder::new();
        frame.set_max_window_size(limits.memory);

        ZstdDecoder {
            frame,
            unit: Unit::Magic,
            pending: Vec::new(),
            need: 4,
            checksum: false,
            after_frame: false,
        }
    }

    /// Moves on to `unit`, which is complete once `pending` holds `need` bytes.
    fn expect(&mut self, unit: Unit, need: usize) {
        self.unit = unit;
        self.need = need;
    }

    fn read_magic(&mut self) -> Result<(), Error> {
        let magic = u32::from_le_bytes(self.pending[..4].try_into().unwrap());

        if magic == MAGIC {
            self.expect(Unit::Descriptor, 5);
        } else if SKIPPABLE_MAGICS.contains(&magic) {
            self.expect(Unit::SkipLength, 8);
        } else if LEGACY_MAGICS.contains(&magic) {
            return Err(Error::Unsupported("legacy zstd frame"));
        } else {
            return Err(self.not_a_frame());
        }

        Ok(())
    }

    fn read_skip_length(&mut self) {
        let length = u32::from_le_bytes(self.pending[4..8].try_into().unwrap());
        self.unit = Unit::Skip(length as usize);
        self.pending.clear();
    }

    fn read_descriptor(&mut self) -> Result<(), Error> {
        let descriptor = self.pending[4];

        if descriptor & DICTIONARY_ID_FLAG != 0 {
            return Err(Error::Unsupported("zstd dictionary"));
        }

        if descriptor & RESERVED_BIT != 0 {
            return Err(Error::Corrupt("reserved zstd frame header bit"));
        }

        let single_segment = descriptor & SINGLE_SEGMENT_FLAG != 0;
        let content_size_length =
            [usize::from(single_segment), 2, 4, 8][usize::from(descriptor >> 6)];
        self.expect(
            Unit::FrameHeader,
            5 + usize::from(!single_segment) + content_size_length,
        );

        Ok(())
    }

    fn start_frame(&mut self) -> Result<(), Error> {
        self.frame
            .init(self.pending.as_slice())
            .map_err(|error| match error {
                FrameDecoderError::WindowSizeTooBig { requested, .. } => {
                    Error::MemoryLimit { needed: requested }
                }
                FrameDecoderError::FrameHeaderError(FrameHeaderError::WindowTooBig { got }) => {
                    Error::MemoryLimit { needed: got }
                }
                _ => Error::Corrupt("invalid zstd frame header"),
            })?;

        self.checksum = self.pending[4] & CHECKSUM_FLAG != 0;
        self.pending.clear();
        self.expect(Unit::BlockHeader, 3);

        Ok(())
    }

    fn read_block_header(&mut self) -> Result<(), Error> {
        let header = u32::from_le_bytes([self.pending[0], self.pending[1], self.pending[2], 0]);
        let last = header & 1 != 0;
        let size = (header >> 3) as usize;

        if size > MAX_BLOCK_SIZE {
            return Err(Error::Corrupt("zstd block too large"));
        }

        let content_length = match (header >> 1) & 3 {
            1 => 1,
            3 => return Err(Error::Corrupt("reserved zstd block type")),
            _ => size,
        };
        let checksum_length = if last && self.checksum { 4 } else { 0 };
        self.expect(Unit::Block { last }, 3 + content_length + checksum_length);

        Ok(())
    }

    /// Decodes the block in `pending` and hands out as much of its output as `output` holds.
    fn decode_block(&mut self, last: bool, output: &mut [u8]) -> Result<usize, Error> {
        self.frame
            .decode_blocks(
                self.pending.as_slice(),
                BlockDecodingStrategy::UptoBlocks(1),
            )
            .map_err(|_| Error::Corrupt("invalid zstd block"))?;

        // ruzstd does not bound a block's literals. Nothing was unread before, so this is the block's output alone.
        if self.frame.decoded_unread() > MAX_BLOCK_SIZE {
            return Err(Error::Corrupt("zstd block too large"));
        }

        self.pending.clear();

        if last {
            self.unit = Unit::End;
        } else {
            self.expect(Unit::BlockHeader, 3);
        }

        Ok(self.frame.read_decoded(output))
    }

    /// Checks the checksum once all of the frame's output is handed out.
    fn finish_frame(&mut self) -> Result<(), Error> {
        if let Some(expected) = self.frame.get_checksum_from_data()
            && self.frame.get_calculated_checksum() != Some(expected)
        {
            return Err(Error::Corrupt("zstd checksum mismatch"));
        }

        self.after_frame = true;
        self.expect(Unit::Magic, 4);

        Ok(())
    }

    fn not_a_frame(&self) -> Error {
        if self.after_frame {
            Error::TrailingData
        } else {
            Error::Corrupt("not zstd")
        }
    }

    /// The outcome when the input ends before the current unit is complete.
    fn cut_off(&self) -> Outcome {
        match self.unit {
            Unit::Magic if self.pending.is_empty() && self.after_frame => Outcome::Done,
            Unit::Magic if !self.pending.is_empty() && !begins_a_magic(&self.pending) => {
                Outcome::Failed(self.not_a_frame())
            }
            _ => Outcome::Failed(Error::Truncated),
        }
    }
}

/// Whether `prefix`, shorter than a magic number, can be the start of a frame's magic number.
fn begins_a_magic(prefix: &[u8]) -> bool {
    LEGACY_MAGICS
        .chain([MAGIC])
        .chain(SKIPPABLE_MAGICS)
        .any(|magic| magic.to_le_bytes().starts_with(prefix))
}

impl FormatDecoder for ZstdDecoder {
    fn step(&mut self, input: &[u8], output: &mut [u8], end_of_input: bool) -> Step {
        // A new block is decoded only once the previous one is handed out, so one call produces at most one block.
        if self.frame.decoded_unread() > 0 {
            return Step {
                consumed: 0,
                produced: self.frame.read_decoded(output),
                outcome: Outcome::Continue,
            };
        }

        if matches!(self.unit, Unit::End)
            && let Err(error) = self.finish_frame()
        {
            return Step {
                consumed: 0,
                produced: 0,
                outcome: Outcome::Failed(error),
            };
        }

        let mut consumed = 0;
        let mut produced = 0;

        let outcome = loop {
            let rest = &input[consumed..];

            if let Unit::Skip(left) = self.unit {
                let take = rest.len().min(left);
                consumed += take;

                if take == left {
                    self.after_frame = true;
                    self.expect(Unit::Magic, 4);

                    break Outcome::Continue;
                }

                self.unit = Unit::Skip(left - take);

                break if end_of_input {
                    Outcome::Failed(Error::Truncated)
                } else {
                    Outcome::Continue
                };
            }

            let take = rest.len().min(self.need - self.pending.len());
            self.pending.extend_from_slice(&rest[..take]);
            consumed += take;

            if self.pending.len() < self.need {
                break if end_of_input {
                    self.cut_off()
                } else {
                    Outcome::Continue
                };
            }

            // Completing a frame header or a block ends the step.
            let completes = matches!(self.unit, Unit::FrameHeader | Unit::Block { .. });

            let result = match self.unit {
                Unit::Magic => self.read_magic(),
                Unit::SkipLength => {
                    self.read_skip_length();

                    Ok(())
                }
                Unit::Descriptor => self.read_descriptor(),
                Unit::FrameHeader => self.start_frame(),
                Unit::BlockHeader => self.read_block_header(),
                Unit::Block { last } => self
                    .decode_block(last, output)
                    .map(|written| produced = written),
                Unit::Skip(_) | Unit::End => unreachable!("handled above"),
            };

            if let Err(error) = result {
                break Outcome::Failed(error);
            }

            if completes {
                break Outcome::Continue;
            }
        };

        Step {
            consumed,
            produced,
            outcome,
        }
    }
}
