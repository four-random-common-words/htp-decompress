mod support;

use htp_decompress::Format;

#[test]
fn manifest_lists_every_format() {
    let gzip = support::fixtures(Format::Gzip);
    let xz = support::fixtures(Format::Xz);
    let zstd = support::fixtures(Format::Zstd);

    assert_eq!(gzip.len(), 10);
    assert_eq!(xz.len(), 17);
    assert_eq!(zstd.len(), 14);
    assert!(gzip.iter().any(|f| f.original.len() == 16_777_216));
}
