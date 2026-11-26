mod support;

use htp_decompress::{Error, Format, Limits};
use support::Expect;

#[test]
fn decodes_every_done_fixture() {
    for fixture in support::fixtures(Format::Zstd) {
        if !matches!(fixture.expect, Expect::Done) {
            continue;
        }

        let input = support::read(&fixture.file);

        let decoded = support::decode(Format::Zstd, &input, 65_536, 65_536, Limits::default());

        assert!(decoded.output == fixture.original, "{}", fixture.file);
        assert_eq!(decoded.result, Ok(()), "{}", fixture.file);
    }
}

#[test]
fn decodes_with_random_chunk_sizes() {
    for fixture in support::fixtures(Format::Zstd) {
        if !matches!(fixture.expect, Expect::Done) {
            continue;
        }

        let input = support::read(&fixture.file);

        for seed in 0..20 {
            let mut sizes = support::chunk_sizes(seed);
            let input_chunk = sizes.next().unwrap();
            let output_chunk = sizes.next().unwrap();

            let decoded = support::decode(
                Format::Zstd,
                &input,
                input_chunk,
                output_chunk,
                Limits::default(),
            );

            assert!(
                decoded.output == fixture.original,
                "{} seed {seed}",
                fixture.file
            );
            assert_eq!(decoded.result, Ok(()), "{} seed {seed}", fixture.file);
        }
    }
}

#[test]
fn decodes_one_byte_at_a_time() {
    for fixture in support::fixtures(Format::Zstd) {
        if !matches!(fixture.expect, Expect::Done) || fixture.original.len() >= 300_000 {
            continue;
        }

        let input = support::read(&fixture.file);

        let decoded = support::decode(Format::Zstd, &input, 1, 1, Limits::default());

        assert!(decoded.output == fixture.original, "{}", fixture.file);
        assert_eq!(decoded.result, Ok(()), "{}", fixture.file);
    }
}

#[test]
fn cut_prefixes_keep_their_output() {
    for fixture in support::fixtures(Format::Zstd) {
        for cut in &fixture.cuts {
            let input = support::read(&fixture.file);

            let decoded = support::decode(
                Format::Zstd,
                &input[..cut.at],
                16_384,
                65_536,
                Limits::default(),
            );

            assert!(fixture.original.starts_with(&decoded.output));
            assert!(decoded.output.len() >= cut.reference_len);
            assert_eq!(decoded.result, Err(Error::Truncated));
        }
    }
}

#[test]
fn reports_expected_errors() {
    for fixture in support::fixtures(Format::Zstd) {
        let Expect::Error(expected) = &fixture.expect else {
            continue;
        };

        let input = support::read(&fixture.file);
        let limits = Limits {
            memory: fixture.limit.unwrap_or(Limits::default().memory),
        };

        let decoded = support::decode(Format::Zstd, &input, 65_536, 65_536, limits);

        // Damage inside compressed data can decode to wrong bytes until a checksum catches it, so only the flipped-byte fixture's first 64 KiB must match.
        let checked = if fixture.file == "zstd/corrupt.zst" {
            &decoded.output[..decoded.output.len().min(65_536)]
        } else {
            &decoded.output
        };
        let error = decoded.result.expect_err(&fixture.file);
        assert!(
            support::same_kind(&error, expected),
            "{}: {error:?}",
            fixture.file
        );
        assert!(fixture.original.starts_with(checked), "{}", fixture.file);
    }
}

#[test]
fn hands_out_everything_before_a_checksum_error() {
    let fixture = support::fixtures(Format::Zstd)
        .into_iter()
        .find(|fixture| fixture.file == "zstd/bad-checksum.zst")
        .expect("zstd/bad-checksum.zst is in the manifest");
    let input = support::read(&fixture.file);

    let decoded = support::decode(Format::Zstd, &input, 65_536, 65_536, Limits::default());

    assert!(decoded.output == fixture.original);
    assert_eq!(
        decoded.result,
        Err(Error::Corrupt("zstd checksum mismatch"))
    );
}

#[test]
fn tells_foreign_bytes_from_a_cut_off_magic() {
    let frame = support::read("zstd/hex-3.zst");

    let foreign = support::decode(Format::Zstd, b"not zstd", 65_536, 65_536, Limits::default());
    let tail = support::decode(
        Format::Zstd,
        &[&frame[..], b"ga"].concat(),
        65_536,
        65_536,
        Limits::default(),
    );
    let cut_magic = support::decode(
        Format::Zstd,
        &[&frame[..], &[0x28, 0xb5]].concat(),
        65_536,
        65_536,
        Limits::default(),
    );

    assert_eq!(foreign.result, Err(Error::Corrupt("not zstd")));
    assert_eq!(tail.result, Err(Error::TrailingData));
    assert_eq!(cut_magic.result, Err(Error::Truncated));
}

#[test]
fn decodes_at_most_one_block_per_call() {
    let input = support::read("zstd/aaaa-long27.zst");

    let decoded = support::decode(
        Format::Zstd,
        &input,
        input.len(),
        1_048_576,
        Limits::default(),
    );

    assert_eq!(decoded.output.len(), 16_777_216);
    assert!(decoded.max_produced_per_call <= 131_072);
}

#[test]
fn rejects_a_block_that_decodes_to_more_than_128_kib() {
    // One compressed block whose 3-byte RLE literals header declares 1,000,000 literals.
    let input = [
        0x28, 0xb5, 0x2f, 0xfd, 0x00, 0x50, 0x2d, 0x00, 0x00, 0x0d, 0x24, 0xf4, 0x41, 0x00,
    ];

    let decoded = support::decode(
        Format::Zstd,
        &input,
        input.len(),
        2_000_000,
        Limits::default(),
    );

    assert_eq!(decoded.result, Err(Error::Corrupt("zstd block too large")));
    assert!(decoded.max_produced_per_call <= 131_072);
}

#[test]
fn rejects_a_window_over_the_limit() {
    let input = support::read("zstd/aaaa-long28.zst");

    let decoded = support::decode(Format::Zstd, &input, 65_536, 65_536, Limits::default());

    assert_eq!(
        decoded.result,
        Err(Error::MemoryLimit {
            needed: 268_435_456
        })
    );
    assert!(decoded.output.is_empty());
}

#[test]
fn accepts_a_window_equal_to_the_limit() {
    let input = support::read("zstd/aaaa-long27.zst");
    let limits = Limits {
        memory: 134_217_728,
    };

    let decoded = support::decode(Format::Zstd, &input, 65_536, 65_536, limits);

    assert_eq!(decoded.result, Ok(()));
}
