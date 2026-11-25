#![allow(dead_code)]

use std::fs;
use std::mem::discriminant;

use htp_decompress::{Decoder, Error, Format, Limits, Status};
use serde_json::Value;

const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures");

pub struct Fixture {
    pub file: String,
    pub format: Format,
    pub original: Vec<u8>,
    pub expect: Expect,
    pub limit: Option<u64>,
    pub cuts: Vec<Cut>,
}

#[derive(Debug)]
pub enum Expect {
    Done,
    Error(Error),
}

pub struct Cut {
    pub at: usize,
    pub reference_len: usize,
}

pub struct Decoded {
    pub output: Vec<u8>,
    pub result: Result<(), Error>,
    pub max_produced_per_call: usize,
}

/// Compares the error variant only, ignoring the payload of `Corrupt`, `Unsupported` and `MemoryLimit`.
pub fn same_kind(a: &Error, b: &Error) -> bool {
    discriminant(a) == discriminant(b)
}

/// Reads a file relative to the `fixtures` directory.
pub fn read(file: &str) -> Vec<u8> {
    fs::read(format!("{ROOT}/{file}")).unwrap_or_else(|e| panic!("cannot read fixture {file}: {e}"))
}

pub fn fixtures(format: Format) -> Vec<Fixture> {
    let name = match format {
        Format::Gzip => "gzip",
        Format::Xz => "xz",
        Format::Zstd => "zstd",
    };

    let manifest: Value =
        serde_json::from_slice(&read("manifest.json")).expect("manifest.json is valid JSON");

    manifest["fixtures"]
        .as_array()
        .expect("manifest has a fixtures array")
        .iter()
        .filter(|entry| entry["format"] == name)
        .map(|entry| parse(entry, format))
        .collect()
}

fn parse(entry: &Value, format: Format) -> Fixture {
    let original = entry["original"]
        .as_array()
        .expect("original is an array")
        .iter()
        .flat_map(|source| source_bytes(source.as_str().expect("original entries are strings")))
        .collect();

    let cuts = entry["cuts"]
        .as_array()
        .expect("cuts is an array")
        .iter()
        .map(|cut| Cut {
            at: cut["at"].as_u64().expect("cut.at") as usize,
            reference_len: cut["referenceLength"]
                .as_u64()
                .expect("cut.referenceLength") as usize,
        })
        .collect();

    Fixture {
        file: entry["file"].as_str().expect("file").to_string(),
        format,
        original,
        expect: expect(entry["expect"].as_str().expect("expect")),
        limit: entry["limit"].as_u64(),
        cuts,
    }
}

fn source_bytes(source: &str) -> Vec<u8> {
    let Some(spec) = source.strip_prefix("repeat:") else {
        return read(&format!("originals/{source}"));
    };

    let (text, count) = spec.rsplit_once(':').expect("repeat:<text>:<count>");

    text.repeat(count.parse().expect("repeat count"))
        .into_bytes()
}

fn expect(name: &str) -> Expect {
    match name {
        "done" => Expect::Done,
        "truncated" => Expect::Error(Error::Truncated),
        "corrupt" => Expect::Error(Error::Corrupt("")),
        "unsupported" => Expect::Error(Error::Unsupported("")),
        "memory-limit" => Expect::Error(Error::MemoryLimit { needed: 0 }),
        "trailing-data" => Expect::Error(Error::TrailingData),
        other => panic!("unknown expectation {other}"),
    }
}

/// Feeds `input` in slices of `input_chunk` bytes and collects the output via `output_chunk` byte buffers.
///
/// Unconsumed input is kept and offered again. Panics when the decoder makes no progress three calls in a row.
pub fn decode(
    format: Format,
    input: &[u8],
    input_chunk: usize,
    output_chunk: usize,
    limits: Limits,
) -> Decoded {
    let mut decoder = Decoder::new(format, limits);

    let mut output = Vec::new();
    let mut buffer = vec![0u8; output_chunk];
    let mut pending: Vec<u8> = Vec::new();
    let mut fed = 0;
    let mut stalled = 0;
    let mut max_produced_per_call = 0;

    let result = loop {
        let mut added = false;

        if fed < input.len() && pending.len() < input_chunk {
            let end = (fed + input_chunk).min(input.len());

            pending.extend_from_slice(&input[fed..end]);
            fed = end;
            added = true;
        }

        let end_of_input = fed == input.len();

        let progress = match decoder.process(&pending, &mut buffer, end_of_input) {
            Ok(progress) => progress,
            Err(error) => break Err(error),
        };

        assert!(
            progress.consumed <= pending.len(),
            "consumed more than offered"
        );
        assert!(
            progress.produced <= buffer.len(),
            "produced more than the buffer holds"
        );

        pending.drain(..progress.consumed);
        output.extend_from_slice(&buffer[..progress.produced]);
        max_produced_per_call = max_produced_per_call.max(progress.produced);

        if progress.status == Status::Done {
            break Ok(());
        }

        if progress.consumed == 0 && progress.produced == 0 && !added {
            stalled += 1;
        } else {
            stalled = 0;
        }

        assert!(stalled < 3, "decoder made no progress in 3 calls in a row");
    };

    Decoded {
        output,
        result,
        max_produced_per_call,
    }
}

/// Endless pseudo-random chunk sizes in `1..=4096` (xorshift64), reproducible per seed.
pub fn chunk_sizes(seed: u64) -> impl Iterator<Item = usize> {
    let mut state = seed ^ 0x9E37_79B9_7F4A_7C15;

    std::iter::repeat_with(move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;

        (state % 4096) as usize + 1
    })
}
