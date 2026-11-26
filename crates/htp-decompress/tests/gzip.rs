mod support;

use htp_decompress::{Error, Format, Limits};
use support::Expect;

#[test]
fn decodes_every_done_fixture() {
    for fixture in support::fixtures(Format::Gzip) {
        if !matches!(fixture.expect, Expect::Done) {
            continue;
        }

        let input = support::read(&fixture.file);

        let decoded = support::decode(Format::Gzip, &input, 65_536, 65_536, Limits::default());

        assert!(decoded.output == fixture.original, "{}", fixture.file);
        assert_eq!(decoded.result, Ok(()), "{}", fixture.file);
    }
}

#[test]
fn decodes_with_random_chunk_sizes() {
    for fixture in support::fixtures(Format::Gzip) {
        if !matches!(fixture.expect, Expect::Done) {
            continue;
        }

        let input = support::read(&fixture.file);

        for seed in 0..20 {
            let mut sizes = support::chunk_sizes(seed);
            let input_chunk = sizes.next().unwrap();
            let output_chunk = sizes.next().unwrap();

            let decoded = support::decode(
                Format::Gzip,
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
    for fixture in support::fixtures(Format::Gzip) {
        if !matches!(fixture.expect, Expect::Done) || fixture.original.len() >= 300_000 {
            continue;
        }

        let input = support::read(&fixture.file);

        let decoded = support::decode(Format::Gzip, &input, 1, 1, Limits::default());

        assert!(decoded.output == fixture.original, "{}", fixture.file);
        assert_eq!(decoded.result, Ok(()), "{}", fixture.file);
    }
}

#[test]
fn cut_prefixes_keep_their_output() {
    for fixture in support::fixtures(Format::Gzip) {
        for cut in &fixture.cuts {
            let input = support::read(&fixture.file);

            let decoded = support::decode(
                Format::Gzip,
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
    for fixture in support::fixtures(Format::Gzip) {
        let Expect::Error(expected) = &fixture.expect else {
            continue;
        };

        let input = support::read(&fixture.file);
        let limits = Limits {
            memory: fixture.limit.unwrap_or(Limits::default().memory),
        };

        let decoded = support::decode(Format::Gzip, &input, 65_536, 65_536, limits);

        let error = decoded.result.expect_err(&fixture.file);

        assert!(
            support::same_kind(&error, expected),
            "{}: {error:?}",
            fixture.file
        );

        // Only the flipped-byte fixture can decode to wrong bytes before the checksum exposes it, so only its first 64 KiB must match.
        let checked = if fixture.file == "gzip/corrupt.gz" {
            &decoded.output[..decoded.output.len().min(65_536)]
        } else {
            &decoded.output
        };

        assert!(fixture.original.starts_with(checked), "{}", fixture.file);
    }
}

#[test]
fn hands_out_everything_before_a_checksum_error() {
    let fixture = support::fixtures(Format::Gzip)
        .into_iter()
        .find(|fixture| fixture.file == "gzip/bad-crc.gz")
        .expect("gzip/bad-crc.gz is in the manifest");
    let input = support::read(&fixture.file);

    let decoded = support::decode(Format::Gzip, &input, 65_536, 65_536, Limits::default());

    assert!(decoded.output == fixture.original);
    assert_eq!(
        decoded.result,
        Err(Error::Corrupt("gzip checksum mismatch"))
    );
}
