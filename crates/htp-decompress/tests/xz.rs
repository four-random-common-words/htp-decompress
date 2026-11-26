mod support;

use htp_decompress::{Error, Format, Limits};
use support::Expect;

#[test]
fn decodes_every_done_fixture() {
    for fixture in support::fixtures(Format::Xz) {
        if !matches!(fixture.expect, Expect::Done) {
            continue;
        }

        let input = support::read(&fixture.file);

        let decoded = support::decode(Format::Xz, &input, 65_536, 65_536, Limits::default());

        assert!(decoded.output == fixture.original, "{}", fixture.file);
        assert_eq!(decoded.result, Ok(()), "{}", fixture.file);
    }
}

#[test]
fn decodes_with_random_chunk_sizes() {
    for fixture in support::fixtures(Format::Xz) {
        if !matches!(fixture.expect, Expect::Done) {
            continue;
        }

        let input = support::read(&fixture.file);

        for seed in 0..20 {
            let mut sizes = support::chunk_sizes(seed);
            let input_chunk = sizes.next().unwrap();
            let output_chunk = sizes.next().unwrap();

            let decoded = support::decode(
                Format::Xz,
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
    for fixture in support::fixtures(Format::Xz) {
        if !matches!(fixture.expect, Expect::Done) || fixture.original.len() >= 300_000 {
            continue;
        }

        let input = support::read(&fixture.file);

        let decoded = support::decode(Format::Xz, &input, 1, 1, Limits::default());

        assert!(decoded.output == fixture.original, "{}", fixture.file);
        assert_eq!(decoded.result, Ok(()), "{}", fixture.file);
    }
}

#[test]
fn cut_prefixes_keep_their_output() {
    for fixture in support::fixtures(Format::Xz) {
        for cut in &fixture.cuts {
            let input = support::read(&fixture.file);

            let decoded = support::decode(
                Format::Xz,
                &input[..cut.at],
                16_384,
                65_536,
                Limits::default(),
            );

            // lzma-rust2 decodes a pass of up to 64 KiB before it hands any of it out, and the pass that runs into the cut
            // fails and is lost, so in highly repetitive data the output falls short of `xz -dc` by up to 64 KiB.
            assert!(fixture.original.starts_with(&decoded.output));
            assert!(
                decoded.output.len() + 65_536 >= cut.reference_len,
                "{} {} {}",
                fixture.file,
                decoded.output.len(),
                cut.reference_len
            );
            assert_eq!(decoded.result, Err(Error::Truncated));
        }
    }
}

#[test]
fn reports_expected_errors() {
    for fixture in support::fixtures(Format::Xz) {
        let Expect::Error(expected) = &fixture.expect else {
            continue;
        };

        let input = support::read(&fixture.file);
        let limits = Limits {
            memory: fixture.limit.unwrap_or(Limits::default().memory),
        };

        let decoded = support::decode(Format::Xz, &input, 65_536, 65_536, limits);

        // Damage inside compressed data can decode to wrong bytes until a check catches it, so only the flipped-byte fixture's first 64 KiB must match; the other error fixtures decode a clean prefix.
        let checked = if fixture.file == "xz/corrupt.xz" {
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
fn tells_foreign_bytes_from_a_cut_off_magic() {
    let frame = support::read("xz/hex.xz");

    let foreign = support::decode(Format::Xz, b"not xz", 65_536, 65_536, Limits::default());
    let tail = support::decode(
        Format::Xz,
        &[&frame[..], b"\x00\x00\x00\x00ga"].concat(),
        65_536,
        65_536,
        Limits::default(),
    );
    let cut_magic = support::decode(
        Format::Xz,
        &[&frame[..], &[0xfd, 0x37, 0x7a]].concat(),
        65_536,
        65_536,
        Limits::default(),
    );

    assert_eq!(foreign.result, Err(Error::Corrupt("not xz")));
    assert_eq!(tail.result, Err(Error::TrailingData));
    assert_eq!(cut_magic.result, Err(Error::Truncated));
}

#[test]
fn reports_the_dictionary_needed() {
    let input = support::read("xz/hex-dict65.xz");
    let limits = Limits { memory: 67_108_864 };

    let decoded = support::decode(Format::Xz, &input, 65_536, 65_536, limits);

    let Err(Error::MemoryLimit { needed }) = decoded.result else {
        panic!("{:?}", decoded.result);
    };
    assert!(needed >= 68_157_440, "{needed}");
}

#[test]
fn loses_a_short_pass_before_damage() {
    let input = support::read("xz/corrupt.xz");

    let whole = support::decode(Format::Xz, &input, input.len(), 65_536, Limits::default());
    let bytewise = support::decode(Format::Xz, &input, 1, 65_536, Limits::default());

    // lzma-rust2 loses the pass that runs into damage. Each of its calls gets at most 4 KiB of input, so that pass is
    // short however the input arrives, where 64 KiB of input made it 19 KB shorter than a byte at a time.
    assert_eq!(whole.result, Err(Error::Corrupt("invalid xz data")));
    assert_eq!(bytewise.result, Err(Error::Corrupt("invalid xz data")));
    assert!(
        whole.output.len().abs_diff(bytewise.output.len()) < 8_192,
        "{} {}",
        whole.output.len(),
        bytewise.output.len()
    );
}
