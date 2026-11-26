#![no_main]

use htp_decompress::{Decoder, Format, Limits, Status};
use libfuzzer_sys::fuzz_target;

const MEMORY_LIMIT: u64 = 16_777_216;

/// Endless pseudo-random chunk sizes in `1..=4096` (xorshift64).
fn chunk_sizes(seed: u64) -> impl Iterator<Item = usize> {
    let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;

    std::iter::repeat_with(move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;

        (state % 4096) as usize + 1
    })
}

fuzz_target!(|data: &[u8]| {
    let [selector, seed, data @ ..] = data else {
        return;
    };

    let format = match selector % 3 {
        0 => Format::Gzip,
        1 => Format::Xz,
        _ => Format::Zstd,
    };

    let mut decoder = Decoder::new(format, Limits { memory: MEMORY_LIMIT });

    let mut input_sizes = chunk_sizes(u64::from(*seed) + 1);
    let mut output_sizes = chunk_sizes(u64::from(*seed) + 257);

    let mut pending: Vec<u8> = Vec::new();
    let mut fed = 0;
    let mut stalled = 0;

    loop {
        let mut added = false;

        if fed < data.len() && pending.is_empty() {
            let end = (fed + input_sizes.next().unwrap()).min(data.len());

            pending.extend_from_slice(&data[fed..end]);
            fed = end;
            added = true;
        }

        let end_of_input = fed == data.len();

        let mut output = vec![0u8; output_sizes.next().unwrap()];

        let Ok(progress) = decoder.process(&pending, &mut output, end_of_input) else {
            return;
        };

        assert!(progress.consumed <= pending.len(), "consumed more than offered");
        assert!(progress.produced <= output.len(), "produced more than the buffer holds");

        pending.drain(..progress.consumed);

        if progress.status == Status::Done {
            return;
        }

        if progress.consumed == 0 && progress.produced == 0 && !added {
            stalled += 1;
        } else {
            stalled = 0;
        }

        assert!(stalled < 3, "decoder made no progress in 3 calls in a row");
    }
});
