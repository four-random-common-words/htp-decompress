use std::io::ErrorKind;

use lzma_rust2::{Action, Status, XzStream};

use crate::{Error, FormatDecoder, Limits, Outcome, Step};

const MAGIC: [u8; 6] = [0xFD, b'7', b'z', b'X', b'Z', 0x00];

/// Where the decoder is between and inside streams.
enum Stage {
    /// Zero padding after a stream, whose length so far is in `padding`.
    Padding,
    /// The bytes in `pending` can still turn out to be the magic number that starts a stream.
    Magic,
    /// A stream's bytes go to `stream`.
    Stream,
}

pub(crate) struct XzDecoder {
    stream: XzStream,
    limit_kib: u32,
    stage: Stage,
    padding: usize,
    pending: Vec<u8>,
    /// Whether a stream has ended, which makes bytes that start no stream trailing data.
    after_stream: bool,
}

impl XzDecoder {
    pub(crate) fn new(limits: Limits) -> XzDecoder {
        let limit_kib = limits.memory.div_ceil(1024).min(u64::from(u32::MAX - 1)) as u32;

        XzDecoder {
            stream: XzStream::new_mem_limit(false, limit_kib),
            limit_kib,
            stage: Stage::Magic,
            padding: 0,
            pending: Vec::new(),
            after_stream: false,
        }
    }

    fn not_a_stream(&self) -> Error {
        if self.after_stream {
            Error::TrailingData
        } else {
            Error::Corrupt("not xz")
        }
    }

    /// The outcome when the input ends outside a stream.
    fn cut_off(&self) -> Outcome {
        match self.stage {
            Stage::Padding if !self.padding.is_multiple_of(4) => {
                Outcome::Failed(Error::Corrupt("xz stream padding"))
            }
            Stage::Padding => Outcome::Done,
            _ => Outcome::Failed(Error::Truncated),
        }
    }

    /// Runs `input` through the current stream. `Finish` is only used once `Run` has nothing left to do, because it fails before the decoder hands out what it already decoded.
    fn run(&mut self, input: &[u8], output: &mut [u8], end_of_input: bool) -> (Step, bool) {
        let (step, ended) = self.process(input, output, Action::Run);
        let idle = step.consumed == 0 && step.produced == 0 && !ended;

        if end_of_input && idle && matches!(step.outcome, Outcome::Continue) {
            return self.process(input, output, Action::Finish);
        }

        (step, ended)
    }

    fn process(&mut self, input: &[u8], output: &mut [u8], action: Action) -> (Step, bool) {
        let before_in = self.stream.total_in();
        let before_out = self.stream.total_out();

        match self.stream.process(input, output, action) {
            Ok(result) => {
                let ended = result.status == Status::StreamEnd;

                (
                    Step {
                        consumed: result.bytes_consumed,
                        produced: result.bytes_produced,
                        outcome: Outcome::Continue,
                    },
                    ended,
                )
            }
            Err(error) => {
                let failure = match error.kind() {
                    ErrorKind::UnexpectedEof => Error::Truncated,
                    ErrorKind::OutOfMemory => Error::MemoryLimit {
                        needed: u64::from(self.stream.memory_usage().unwrap_or(self.limit_kib))
                            * 1024,
                    },
                    ErrorKind::Unsupported => Error::Unsupported("xz feature"),
                    ErrorKind::InvalidData if error.to_string().contains("not supported") => {
                        Error::Unsupported("chain of xz filters")
                    }
                    _ => Error::Corrupt("invalid xz data"),
                };

                (
                    Step {
                        consumed: (self.stream.total_in() - before_in) as usize,
                        produced: (self.stream.total_out() - before_out) as usize,
                        outcome: Outcome::Failed(failure),
                    },
                    false,
                )
            }
        }
    }
}

impl FormatDecoder for XzDecoder {
    fn step(&mut self, input: &[u8], output: &mut [u8], end_of_input: bool) -> Step {
        let mut consumed = 0;

        loop {
            let rest = &input[consumed..];

            match self.stage {
                Stage::Stream => {
                    let (mut step, ended) = self.run(rest, output, end_of_input);
                    consumed += step.consumed;

                    if !ended {
                        step.consumed = consumed;

                        return step;
                    }

                    self.after_stream = true;
                    self.stage = Stage::Padding;
                    self.padding = 0;

                    if step.produced > 0 {
                        step.consumed = consumed;

                        return step;
                    }
                }
                Stage::Padding => {
                    let zeros = rest.iter().take_while(|&&byte| byte == 0).count();
                    self.padding += zeros;
                    consumed += zeros;

                    if zeros < rest.len() {
                        if !self.padding.is_multiple_of(4) {
                            return Step {
                                consumed,
                                produced: 0,
                                outcome: Outcome::Failed(Error::Corrupt("xz stream padding")),
                            };
                        }

                        self.stage = Stage::Magic;
                    } else {
                        return Step {
                            consumed,
                            produced: 0,
                            outcome: if end_of_input {
                                self.cut_off()
                            } else {
                                Outcome::Continue
                            },
                        };
                    }
                }
                Stage::Magic => {
                    let take = rest.len().min(MAGIC.len() - self.pending.len());
                    self.pending.extend_from_slice(&rest[..take]);
                    consumed += take;

                    if !MAGIC.starts_with(&self.pending) {
                        return Step {
                            consumed,
                            produced: 0,
                            outcome: Outcome::Failed(self.not_a_stream()),
                        };
                    }

                    if self.pending.len() < MAGIC.len() {
                        return Step {
                            consumed,
                            produced: 0,
                            outcome: if end_of_input {
                                self.cut_off()
                            } else {
                                Outcome::Continue
                            },
                        };
                    }

                    if self.after_stream {
                        self.stream = XzStream::new_mem_limit(false, self.limit_kib);
                    }

                    let magic = std::mem::take(&mut self.pending);
                    let (step, _) = self.run(&magic, &mut [], false);
                    self.stage = Stage::Stream;

                    if let Outcome::Failed(_) = step.outcome {
                        return Step { consumed, ..step };
                    }
                }
            }
        }
    }
}
