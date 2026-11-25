use std::fmt;

#[cfg(feature = "gzip")]
mod gzip;
#[cfg(feature = "xz")]
mod xz;
#[cfg(feature = "zstd")]
mod zstd;

pub const DEFAULT_MEMORY_LIMIT: u64 = 134_217_728;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    #[cfg(feature = "gzip")]
    Gzip,
    #[cfg(feature = "xz")]
    Xz,
    #[cfg(feature = "zstd")]
    Zstd,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub memory: u64,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            memory: DEFAULT_MEMORY_LIMIT,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    pub consumed: usize,
    pub produced: usize,
    pub status: Status,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Continue,
    Done,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Truncated,
    Corrupt(&'static str),
    Unsupported(&'static str),
    MemoryLimit { needed: u64 },
    TrailingData,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Truncated => write!(f, "input ends before the end of the stream"),
            Error::Corrupt(what) => write!(f, "corrupt data: {what}"),
            Error::Unsupported(what) => write!(f, "unsupported feature: {what}"),
            Error::MemoryLimit { needed } => {
                write!(
                    f,
                    "decoding needs {needed} bytes, which exceeds the memory limit"
                )
            }
            Error::TrailingData => write!(f, "unexpected data after the end of the stream"),
        }
    }
}

impl std::error::Error for Error {}

pub(crate) trait FormatDecoder {
    fn step(&mut self, input: &[u8], output: &mut [u8], end_of_input: bool) -> Step;
}

pub(crate) struct Step {
    pub consumed: usize,
    pub produced: usize,
    pub outcome: Outcome,
}

pub(crate) enum Outcome {
    Continue,
    Done,
    Failed(Error),
}

pub struct Decoder {
    inner: Box<dyn FormatDecoder>,
    state: Option<Result<(), Error>>,
}

impl Decoder {
    pub fn new(format: Format, limits: Limits) -> Decoder {
        let inner = match format {
            #[cfg(feature = "gzip")]
            Format::Gzip => gzip::new(limits),
            #[cfg(feature = "xz")]
            Format::Xz => xz::new(limits),
            #[cfg(feature = "zstd")]
            Format::Zstd => zstd::new(limits),
        };

        Decoder { inner, state: None }
    }

    #[cfg(test)]
    fn from_inner(inner: Box<dyn FormatDecoder>) -> Decoder {
        Decoder { inner, state: None }
    }

    pub fn process(
        &mut self,
        input: &[u8],
        output: &mut [u8],
        end_of_input: bool,
    ) -> Result<Progress, Error> {
        match &self.state {
            Some(Ok(())) => {
                return Ok(Progress {
                    consumed: 0,
                    produced: 0,
                    status: Status::Done,
                });
            }
            Some(Err(error)) => return Err(error.clone()),
            None => {}
        }

        let step = self.inner.step(input, output, end_of_input);
        let status = match step.outcome {
            Outcome::Continue => Status::Continue,
            Outcome::Done => {
                self.state = Some(Ok(()));
                Status::Done
            }
            Outcome::Failed(error) => {
                self.state = Some(Err(error.clone()));

                // Output produced before the failure is delivered first; the error follows on the next call.
                if step.produced == 0 {
                    return Err(error);
                }

                Status::Continue
            }
        };

        Ok(Progress {
            consumed: step.consumed,
            produced: step.produced,
            status,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    struct Scripted(VecDeque<Step>);

    impl FormatDecoder for Scripted {
        fn step(&mut self, _input: &[u8], _output: &mut [u8], _end_of_input: bool) -> Step {
            self.0.pop_front().expect("script exhausted")
        }
    }

    fn step(consumed: usize, produced: usize, outcome: Outcome) -> Step {
        Step {
            consumed,
            produced,
            outcome,
        }
    }

    fn scripted(steps: Vec<Step>) -> Decoder {
        Decoder::from_inner(Box::new(Scripted(steps.into())))
    }

    #[test]
    fn output_comes_before_the_error() {
        let mut decoder = scripted(vec![step(3, 5, Outcome::Failed(Error::Truncated))]);

        let first = decoder.process(b"abc", &mut [0; 16], true);
        let second = decoder.process(b"", &mut [0; 16], true);

        assert_eq!(
            first,
            Ok(Progress {
                consumed: 3,
                produced: 5,
                status: Status::Continue
            })
        );
        assert_eq!(second, Err(Error::Truncated));
    }

    #[test]
    fn an_error_repeats() {
        let mut decoder = scripted(vec![step(0, 0, Outcome::Failed(Error::Corrupt("x")))]);

        let first = decoder.process(b"abc", &mut [0; 16], false);
        let second = decoder.process(b"abc", &mut [0; 16], false);
        let third = decoder.process(b"abc", &mut [0; 16], false);

        assert_eq!(first, Err(Error::Corrupt("x")));
        assert_eq!(second, Err(Error::Corrupt("x")));
        assert_eq!(third, Err(Error::Corrupt("x")));
    }

    #[test]
    fn done_stays_done() {
        let mut decoder = scripted(vec![step(2, 4, Outcome::Done)]);

        let first = decoder.process(b"ab", &mut [0; 16], true);
        let later = decoder.process(b"more", &mut [0; 16], true);

        assert_eq!(
            first,
            Ok(Progress {
                consumed: 2,
                produced: 4,
                status: Status::Done
            })
        );
        assert_eq!(
            later,
            Ok(Progress {
                consumed: 0,
                produced: 0,
                status: Status::Done
            })
        );
    }

    #[test]
    fn continue_passes_through() {
        let mut decoder = scripted(vec![
            step(2, 1, Outcome::Continue),
            step(1, 0, Outcome::Done),
        ]);

        let first = decoder.process(b"ab", &mut [0; 16], false);
        let second = decoder.process(b"c", &mut [0; 16], true);

        assert_eq!(
            first,
            Ok(Progress {
                consumed: 2,
                produced: 1,
                status: Status::Continue
            })
        );
        assert_eq!(
            second,
            Ok(Progress {
                consumed: 1,
                produced: 0,
                status: Status::Done
            })
        );
    }

    #[test]
    fn default_limit_is_128_mib() {
        assert_eq!(Limits::default().memory, 134_217_728);
    }
}
