use crc32fast::Hasher;
use miniz_oxide::inflate::stream::{InflateState, inflate};
use miniz_oxide::{DataFormat, MZError, MZFlush, MZStatus};

use crate::{Error, FormatDecoder, Outcome, Step};

const FLAG_HEADER_CRC: u8 = 0x02;
const FLAG_EXTRA: u8 = 0x04;
const FLAG_NAME: u8 = 0x08;
const FLAG_COMMENT: u8 = 0x10;
const FLAG_RESERVED: u8 = 0xe0;

#[derive(Clone, Copy)]
enum Stage {
    Fixed,
    ExtraLength,
    ExtraData,
    Name,
    Comment,
    HeaderCrc,
    Complete,
}

enum Phase {
    Header,
    Body,
    Trailer,
}

pub(crate) struct GzipDecoder {
    phase: Phase,
    members: u32,
    started: bool,
    stage: Stage,
    flags: u8,
    position: usize,
    extra_length: usize,
    header_crc: Hasher,
    header_check: [u8; 2],
    inflater: Box<InflateState>,
    crc: Hasher,
    size: u32,
    trailer: [u8; 8],
}

impl GzipDecoder {
    pub(crate) fn new() -> GzipDecoder {
        GzipDecoder {
            phase: Phase::Header,
            members: 0,
            started: false,
            stage: Stage::Fixed,
            flags: 0,
            position: 0,
            extra_length: 0,
            header_crc: Hasher::new(),
            header_check: [0; 2],
            inflater: InflateState::new_boxed(DataFormat::Raw),
            crc: Hasher::new(),
            size: 0,
            trailer: [0; 8],
        }
    }

    /// Moves to the first optional header field at or after `order` that the flags announce.
    fn enter(&mut self, order: u8) {
        self.position = 0;

        self.stage = if order == 0 && self.flags & FLAG_EXTRA != 0 {
            Stage::ExtraLength
        } else if order <= 1 && self.flags & FLAG_NAME != 0 {
            Stage::Name
        } else if order <= 2 && self.flags & FLAG_COMMENT != 0 {
            Stage::Comment
        } else if order <= 3 && self.flags & FLAG_HEADER_CRC != 0 {
            Stage::HeaderCrc
        } else {
            Stage::Complete
        };
    }

    fn header_byte(&mut self, byte: u8) -> Result<(), Error> {
        if !matches!(self.stage, Stage::HeaderCrc) {
            self.header_crc.update(&[byte]);
        }

        match self.stage {
            Stage::Fixed => {
                let invalid = match self.position {
                    0 => byte != 0x1f,
                    1 => byte != 0x8b,
                    _ => false,
                };

                if invalid {
                    return Err(if self.members > 0 {
                        Error::TrailingData
                    } else {
                        Error::Corrupt("not a gzip stream")
                    });
                }

                if self.position == 2 && byte != 8 {
                    return Err(Error::Unsupported("gzip compression method"));
                }

                if self.position == 3 {
                    if byte & FLAG_RESERVED != 0 {
                        return Err(Error::Corrupt("reserved gzip flag"));
                    }

                    self.flags = byte;
                }

                self.position += 1;

                if self.position == 10 {
                    self.enter(0);
                }
            }
            Stage::ExtraLength => {
                self.extra_length |= usize::from(byte) << (8 * self.position);
                self.position += 1;

                if self.position == 2 {
                    if self.extra_length == 0 {
                        self.enter(1);
                    } else {
                        self.stage = Stage::ExtraData;
                        self.position = 0;
                    }
                }
            }
            Stage::ExtraData => {
                self.position += 1;

                if self.position == self.extra_length {
                    self.enter(1);
                }
            }
            Stage::Name => {
                if byte == 0 {
                    self.enter(2);
                }
            }
            Stage::Comment => {
                if byte == 0 {
                    self.enter(3);
                }
            }
            Stage::HeaderCrc => {
                self.header_check[self.position] = byte;
                self.position += 1;

                if self.position == 2 {
                    let expected = (self.header_crc.clone().finalize() & 0xffff) as u16;

                    if u16::from_le_bytes(self.header_check) != expected {
                        return Err(Error::Corrupt("gzip header checksum mismatch"));
                    }

                    self.stage = Stage::Complete;
                }
            }
            Stage::Complete => {}
        }

        Ok(())
    }

    fn finish_member(&mut self) -> Result<(), Error> {
        let crc = u32::from_le_bytes(self.trailer[..4].try_into().unwrap());
        let size = u32::from_le_bytes(self.trailer[4..].try_into().unwrap());

        if crc != std::mem::replace(&mut self.crc, Hasher::new()).finalize() {
            return Err(Error::Corrupt("gzip checksum mismatch"));
        }

        if size != self.size {
            return Err(Error::Corrupt("gzip size mismatch"));
        }

        self.members += 1;
        self.started = false;
        self.stage = Stage::Fixed;
        self.position = 0;
        self.extra_length = 0;
        self.header_crc = Hasher::new();
        self.size = 0;
        self.inflater.reset(DataFormat::Raw);
        self.phase = Phase::Header;

        Ok(())
    }
}

impl FormatDecoder for GzipDecoder {
    fn step(&mut self, input: &[u8], output: &mut [u8], end_of_input: bool) -> Step {
        let mut consumed = 0;
        let mut produced = 0;

        let outcome = loop {
            let rest = &input[consumed..];

            match self.phase {
                Phase::Header => {
                    let mut failure = None;

                    for &byte in rest {
                        if let Err(error) = self.header_byte(byte) {
                            failure = Some(error);
                            break;
                        }

                        consumed += 1;
                        self.started = true;

                        if matches!(self.stage, Stage::Complete) {
                            self.phase = Phase::Body;
                            break;
                        }
                    }

                    if let Some(error) = failure {
                        break Outcome::Failed(error);
                    }

                    if matches!(self.phase, Phase::Header) {
                        if !end_of_input {
                            break Outcome::Continue;
                        }

                        break if self.members > 0 && !self.started {
                            Outcome::Done
                        } else {
                            Outcome::Failed(Error::Truncated)
                        };
                    }
                }
                Phase::Body => {
                    if produced == output.len() {
                        break Outcome::Continue;
                    }

                    let result = inflate(
                        &mut self.inflater,
                        rest,
                        &mut output[produced..],
                        MZFlush::None,
                    );
                    let written = &output[produced..produced + result.bytes_written];

                    self.crc.update(written);
                    self.size = self.size.wrapping_add(result.bytes_written as u32);
                    consumed += result.bytes_consumed;
                    produced += result.bytes_written;

                    match result.status {
                        Ok(MZStatus::StreamEnd) => {
                            self.phase = Phase::Trailer;
                            self.position = 0;
                        }
                        Ok(_) | Err(MZError::Buf) => {
                            if result.bytes_consumed == 0 && result.bytes_written == 0 {
                                break if end_of_input {
                                    Outcome::Failed(Error::Truncated)
                                } else {
                                    Outcome::Continue
                                };
                            }
                        }
                        Err(_) => {
                            break Outcome::Failed(Error::Corrupt("invalid deflate data"));
                        }
                    }
                }
                Phase::Trailer => {
                    let take = rest.len().min(8 - self.position);

                    self.trailer[self.position..self.position + take]
                        .copy_from_slice(&rest[..take]);
                    self.position += take;
                    consumed += take;

                    if self.position < 8 {
                        break if end_of_input {
                            Outcome::Failed(Error::Truncated)
                        } else {
                            Outcome::Continue
                        };
                    }

                    if let Err(error) = self.finish_member() {
                        break Outcome::Failed(error);
                    }
                }
            }
        };

        Step {
            consumed,
            produced,
            outcome,
        }
    }
}
