# htp-decompress Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Rust decoder for gzip, xz and zstd, compiled to one wasm module per format and shipped as
the npm package `htp-decompress`, whose `HtpDecompressionStream` works like the browser's
`DecompressionStream`.

**Architecture:** A pure-Rust core crate exposes one push decoder (`Decoder::process`) with the same
contract for all formats. A thin `cdylib` crate exposes it through raw wasm exports and is built once
per format. The npm package inlines each module as base64, loads it on first use, gives every stream
its own instance, and drives it from a pull-based `{ readable, writable }` pair.

**Tech Stack:** Rust stable (edition 2024), `wasm32-unknown-unknown`, miniz_oxide 0.9, crc32fast 1.5,
lzma-rust2 0.25.0, ruzstd 0.9.1 (vendored with a patch), TypeScript ~6.0.3, Vitest ^5.0.3, Node 24.

**Spec:** `docs/superpowers/specs/2026-11-25-htp-decompress-design.md`

**Scope:** this repository only. The web-ui pull request (the spec's last phase) gets its own plan
once the owner has published the first npm version; it depends on that publish and lives in another
repository.

## Global Constraints

- Commit locally at the end of every task on branch `initial-implementation`: stage only that
  task's files, a single-line subject, no body, no attribution of any kind. Never push, tag or
  publish. A correction to an earlier task after its checkpoint goes in as
  `git commit --fixup=<sha of that task's commit>`. Do not use the `gh` CLI or the GitHub API.
- No C code anywhere in the build; no clang needed.
- Rust: stable channel pinned in `rust-toolchain.toml` with target `wasm32-unknown-unknown` and
  components `rustfmt`, `clippy`; edition 2024; `cargo clippy --all-targets -- -D warnings` clean.
- Dependencies, exact: `lzma-rust2 = { version = "=0.25.0", default-features = false, features = ["std", "xz"] }`,
  `ruzstd = { version = "=0.9.1", default-features = false, features = ["std", "hash"] }` (patched,
  see Task 4), `miniz_oxide = { version = "0.9", default-features = false, features = ["with-alloc"] }`,
  `crc32fast = { version = "1.5", default-features = false }`.
- Default memory limit: 134,217,728 bytes (128 MiB). Output chunk: 65,536 bytes. Input slice per
  wasm call: 65,536 bytes. Yield to the event loop after 8 ms of decoding.
- npm package: name `htp-decompress`, version `0.1.0`, `"type": "module"`, ESM only, `.d.ts` included,
  no runtime dependencies, `"sideEffects": false`, license `GPL-3.0-only`, repository
  `git+https://github.com/four-random-common-words/htp-decompress.git`.
- Dev tooling versions match web-ui: `typescript ~6.0.3`, `vitest ^5.0.3`, `@types/node ^24.19.0`.
- Runtime targets: Node 24 and web-ui's browsers (last Chrome, last Firefox, Firefox ESR, last 2 Edge,
  Safari, iOS).
- License file: GPL-3.0 text, copied from `../web-ui/LICENSE`.
- Code style: the blank-line rules in `/Users/uhashy/projects/hashtopolis/CLAUDE.md` apply to Rust,
  TypeScript and scripts.
- Writing: no long dashes in docs or messages.

## Review Focus

1. Zero-length writes between real chunks: must neither stall the stream nor count as end of input
   (test `ignores empty chunks`, Task 8).
2. Headers split across 1-byte writes (gzip FNAME/FEXTRA, zstd frame and block headers, xz block
   headers): decoding must match the whole-input result (test `decodes_one_byte_at_a_time`, Tasks 3,
   5, 6).
3. A writer that reuses or overwrites its buffer after `write()` resolves: output must be
   unaffected (test `copies written bytes`, Task 8).
4. Cancel or abort while the wasm module is still loading, or before any input arrived: every
   promise settles and nothing is left running (test `cancel during module load settles everything`,
   Task 8).
5. Every `BufferSource` kind (ArrayBuffer, DataView, a `Uint16Array` view with an offset), as
   `DecompressionStream` accepts (test `accepts every buffer source`, Task 8).

---

### Task 1: Workspace and the core decoder contract

**Files:**
- Create: `Cargo.toml`, `rust-toolchain.toml`, `.gitignore`, `LICENSE`
- Create: `crates/htp-decompress/Cargo.toml`, `crates/htp-decompress/src/lib.rs`

**Interfaces:**
- Produces (in `htp_decompress`):
  - `pub enum Format { #[cfg(feature = "gzip")] Gzip, #[cfg(feature = "xz")] Xz, #[cfg(feature = "zstd")] Zstd }`
  - `pub const DEFAULT_MEMORY_LIMIT: u64 = 134_217_728;`
  - `#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub struct Limits { pub memory: u64 }`, `Default` gives `DEFAULT_MEMORY_LIMIT`
  - `pub struct Decoder`, `Decoder::new(format: Format, limits: Limits) -> Decoder`,
    `Decoder::process(&mut self, input: &[u8], output: &mut [u8], end_of_input: bool) -> Result<Progress, Error>`
  - `#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub struct Progress { pub consumed: usize, pub produced: usize, pub status: Status }`
  - `#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum Status { Continue, Done }`
  - `#[derive(Clone, Debug, PartialEq, Eq)] pub enum Error { Truncated, Corrupt(&'static str), Unsupported(&'static str), MemoryLimit { needed: u64 }, TrailingData }`, with `Display` and `std::error::Error`
  - crate-internal, used by Tasks 3, 5, 6: `pub(crate) trait FormatDecoder { fn step(&mut self, input: &[u8], output: &mut [u8], end_of_input: bool) -> Step; }`,
    `pub(crate) struct Step { pub consumed: usize, pub produced: usize, pub outcome: Outcome }`,
    `pub(crate) enum Outcome { Continue, Done, Failed(Error) }`

Workspace `Cargo.toml`: members `crates/htp-decompress`, `crates/htp-decompress-wasm` (added in Task 7;
list only the first now), `exclude = ["vendor/ruzstd", "crates/htp-decompress/fuzz"]`,
`[patch.crates-io] ruzstd = { path = "vendor/ruzstd" }` (added in Task 4), and
`[profile.release] lto = true, codegen-units = 1, panic = "abort", strip = true, opt-level = "s"`.
Core crate features: `default = ["gzip", "xz", "zstd"]`, each enabling its optional dependencies.
`.gitignore`: `target/`, `npm/node_modules/`, `npm/dist/`, `npm/build/`, `npm/src/wasm/`.

- [ ] **Step 1: Write the failing contract tests** in `crates/htp-decompress/src/lib.rs` (`#[cfg(test)]`,
  using a scripted `FormatDecoder` that returns a fixed list of `Step`s; `Decoder` gets a
  `#[cfg(test)] fn from_inner(inner: Box<dyn FormatDecoder>) -> Decoder`):

```rust
#[test]
fn output_comes_before_the_error() {
    let mut decoder = scripted(vec![step(3, 5, Outcome::Failed(Error::Truncated))]);

    let first = decoder.process(b"abc", &mut [0; 16], true);
    let second = decoder.process(b"", &mut [0; 16], true);

    assert_eq!(first, Ok(Progress { consumed: 3, produced: 5, status: Status::Continue }));
    assert_eq!(second, Err(Error::Truncated));
}

#[test]
fn an_error_repeats() { /* Failed(Corrupt("x")) with produced 0: first and third call both Err(Corrupt("x")) */ }

#[test]
fn done_stays_done() { /* after Outcome::Done, a later call returns Ok(Progress { 0, 0, Done }) */ }

#[test]
fn default_limit_is_128_mib() { assert_eq!(Limits::default().memory, 134_217_728); }
```

- [ ] **Step 2: Run them and see them fail**
  Run: `cargo test -p htp-decompress --lib`. Expected: compile errors for the missing items.

- [ ] **Step 3: Implement the types and `Decoder::process`** in `src/lib.rs`. `Decoder` holds
  `inner: Box<dyn FormatDecoder>` and `state: Option<Result<(), Error>>` (finished or failed). A
  `Failed` step with `produced > 0` is stashed and returned as `Ok(Continue)`; the stashed error is
  returned on every later call. `Decoder::new` matches on `Format` and calls each format module's
  constructor (stub them with `todo!()` behind their features until Tasks 3, 5, 6).

- [ ] **Step 4: Run the tests**
  Run: `cargo test -p htp-decompress --lib`. Expected: PASS. (Clippy is enforced from Task 6 on,
  once the stubs are gone.)

- [ ] **Step 5: Commit** (first run `git init -b main && git checkout -b initial-implementation`).

---

### Task 2: Test fixtures and shared test support

**Files:**
- Create: `fixtures/generate.py`, `fixtures/README.md`, `fixtures/manifest.json` (generated),
  `fixtures/originals/*`, `fixtures/{gzip,xz,zstd}/*` (generated)
- Create: `crates/htp-decompress/tests/support/mod.rs`
- Modify: `crates/htp-decompress/Cargo.toml` (dev-dependency `serde_json = "1"`)

**Interfaces:**
- Produces (in `tests/support/mod.rs`, used by Tasks 3, 5, 6, 10):
  - `pub struct Fixture { pub file: String, pub format: Format, pub original: Vec<u8>, pub expect: Expect, pub limit: Option<u64>, pub cuts: Vec<Cut> }`
  - `pub enum Expect { Done, Error(Error) }` (manifest strings: `done`, `truncated`, `corrupt`,
    `unsupported`, `memory-limit`, `trailing-data`; for `Corrupt`/`Unsupported`/`MemoryLimit` compare
    the variant only, via `fn same_kind(&Error, &Error) -> bool`)
  - `pub struct Cut { pub at: usize, pub reference_len: usize }`
  - `pub fn fixtures(format: Format) -> Vec<Fixture>`, `pub fn read(file: &str) -> Vec<u8>` (path relative to `fixtures/`)
  - `pub fn decode(format: Format, input: &[u8], input_chunk: usize, output_chunk: usize, limits: Limits) -> Decoded`
  - `pub struct Decoded { pub output: Vec<u8>, pub result: Result<(), Error>, pub max_produced_per_call: usize }`
  - `pub fn chunk_sizes(seed: u64) -> impl Iterator<Item = usize>` (xorshift, values 1..=4096, no `rand` dependency)

`generate.py` runs with `python3 -I fixtures/generate.py` and needs `gzip`, `xz`, `zstd` and `pzstd`
on `PATH`. It writes originals, compressed files and `manifest.json`:
`{ "fixtures": [ { "file": "gzip/hex.gz", "format": "gzip", "original": ["hex.txt"], "expect": "done", "limit": null, "cuts": [ { "at": 1234, "referenceLength": 5678 } ] } ] }`.
`original` lists sources concatenated in order: a file under `originals/`, or `"repeat:a\n:8388608"`
(the text repeated, 16 MiB, never stored). For every `done` fixture over 64 bytes, `cuts` holds
cuts at 10, 25, 50 and 90 % of the compressed size, each with the stdout length of `gzip -dc`,
`xz -dc` or `zstd -dc` on that prefix (exit status ignored).

Originals: `numbered.txt` = `line {n}\n` for n in 1..=1000; `hex.txt` = lowercase MD5 hex of `str(i)`
plus `\n` for i in 0..8192 (270,336 bytes); `empty.txt` = no bytes.

| File | Made with | Original | Expect |
|---|---|---|---|
| gzip/hex.gz | `gzip -9 -n` | hex | done |
| gzip/hex-headers.gz | Python: header with FEXTRA (4 bytes), FNAME `hex.txt`, FCOMMENT `fixture`, FHCRC, mtime 0; raw deflate (`zlib`, wbits -15); CRC32 and ISIZE trailer | hex | done |
| gzip/members.gz | `gzip -n` numbered, then hex, concatenated | numbered, hex | done |
| gzip/empty.gz | `gzip -n` of empty | empty | done |
| gzip/aaaa.gz | `gzip -1 -n` of the repeat | repeat | done |
| gzip/corrupt.gz | hex.gz, byte at len/2 XOR 0xff | hex | corrupt |
| gzip/bad-crc.gz | hex.gz, byte at len-8 XOR 0xff | hex | corrupt |
| gzip/reserved-flag.gz | hex.gz with FLG bit 5 set | hex | corrupt |
| gzip/trailing.gz | hex.gz + `garbage!` | hex | trailing-data |
| gzip/zero.gz | 0 bytes | empty | truncated |
| xz/hex.xz | `xz -6` | hex | done |
| xz/hex-none.xz, hex-crc32.xz, hex-sha256.xz | `xz -6 --check=none/crc32/sha256` | hex | done |
| xz/hex-x86.xz | `xz --x86 --lzma2=preset=6` | hex | done |
| xz/hex-delta.xz | `xz --delta=dist=4 --lzma2=preset=6` | hex | done |
| xz/hex-blocks.xz | `xz -6 -T2 --block-size=16KiB` | hex | done |
| xz/concat.xz | numbered.xz + 4 zero bytes + hex.xz | numbered, hex | done |
| xz/empty.xz | `xz -6` of empty | empty | done |
| xz/hex-dict65.xz | `xz --lzma2=preset=6,dict=65MiB` from stdin | hex | done |
| xz/hex-dict65.xz (second entry, `limit` 67,108,864) | same file | hex | memory-limit |
| xz/aaaa-dict65.xz | same options, repeat from stdin | repeat | done |
| xz/hex-chain.xz | `xz --x86 --delta=dist=4 --lzma2=preset=6` | hex | unsupported |
| xz/corrupt.xz | hex.xz, byte at len/2 XOR 0xff | hex | corrupt |
| xz/bad-padding.xz | hex.xz + 2 zero bytes | hex | corrupt |
| xz/trailing.xz | hex.xz + `garbage!` | hex | trailing-data |
| xz/zero.xz | 0 bytes | empty | truncated |
| zstd/hex-3.zst, hex-19.zst | `zstd -3` / `zstd -19` | hex | done |
| zstd/hex-nocheck.zst | `zstd -3 --no-check` | hex | done |
| zstd/hex-pzstd.zst | `pzstd -p 2 -3` | hex | done |
| zstd/frames.zst | `zstd -3` numbered + hex concatenated | numbered, hex | done |
| zstd/empty.zst | `zstd -3` of empty | empty | done |
| zstd/aaaa-long27.zst | `zstd -3 --long=27` from stdin (window 128 MiB) | repeat | done |
| zstd/aaaa-long28.zst | `zstd -3 --long=28` from stdin (window 256 MiB) | repeat | memory-limit |
| zstd/dict.zst | Python: `28 b5 2f fd`, FHD `0x21`, dict id `0x07`, FCS `0x00`, block header `01 00 00` | empty | unsupported |
| zstd/legacy.zst | Python: legacy v0.7 magic `27 b5 2f fd` + 16 zero bytes | empty | unsupported |
| zstd/corrupt.zst | hex-3.zst, byte at len/2 XOR 0xff | hex | corrupt |
| zstd/bad-checksum.zst | hex-3.zst, last byte XOR 0xff | hex | corrupt |
| zstd/trailing.zst | hex-3.zst + `garbage!` | hex | trailing-data |
| zstd/zero.zst | 0 bytes | empty | truncated |

- [ ] **Step 1: Write `generate.py` and run it**
  Run: `python3 -I fixtures/generate.py`. Expected: the files above and `manifest.json`; total size
  under 5 MB (`du -sh fixtures`). `fixtures/README.md` lists the tool versions used (`gzip --version`,
  `xz --version`, `zstd --version`) and how to regenerate.

- [ ] **Step 2: Write a failing self-test** `tests/support_smoke.rs`:

```rust
mod support;

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
```

- [ ] **Step 3: Run it and see it fail**, then implement `support/mod.rs` (`decode` feeds input in
  `input_chunk` slices, keeps unconsumed bytes, sets `end_of_input` once the last slice is passed,
  stops at `Done` or `Err`, and panics after 3 calls in a row without progress).
  Run: `cargo test -p htp-decompress --test support_smoke`. Expected: FAIL before, PASS after.

- [ ] **Step 4: Commit.**

---

### Task 3: gzip decoder

**Files:**
- Create: `crates/htp-decompress/src/gzip.rs`, `crates/htp-decompress/tests/gzip.rs`
- Modify: `crates/htp-decompress/src/lib.rs` (wire `Format::Gzip`)

**Interfaces:**
- Consumes: `FormatDecoder`, `Step`, `Outcome`, `Error` (Task 1); `support::*` (Task 2)
- Produces: `pub(crate) struct GzipDecoder`, `GzipDecoder::new() -> GzipDecoder`, `impl FormatDecoder`

- [ ] **Step 1: Write the failing tests** in `tests/gzip.rs`. These five tests are the per-format
  suite; Tasks 5 and 6 copy them with their `Format`:

```rust
#[test]
fn decodes_every_done_fixture() { /* each Expect::Done fixture, chunks 65_536/65_536: output == original, result Ok(()) */ }

#[test]
fn decodes_with_random_chunk_sizes() { /* each Done fixture, 20 seeds: input and output chunks from chunk_sizes(seed): output == original */ }

#[test]
fn decodes_one_byte_at_a_time() { /* each Done fixture whose original is under 300_000 bytes, chunks 1/1: output == original */ }

#[test]
fn cut_prefixes_keep_their_output() {
    for fixture in support::fixtures(Format::Gzip) {
        for cut in &fixture.cuts {
            let input = support::read(&fixture.file);

            let decoded = support::decode(Format::Gzip, &input[..cut.at], 16_384, 65_536, Limits::default());

            assert!(fixture.original.starts_with(&decoded.output));
            assert!(decoded.output.len() >= cut.reference_len);
            assert_eq!(decoded.result, Err(Error::Truncated));
        }
    }
}

#[test]
fn reports_expected_errors() { /* each Expect::Error fixture (with its limit): same_kind(result, expected), original.starts_with(output) */ }
```

- [ ] **Step 2: Run them and see them fail**
  Run: `cargo test -p htp-decompress --test gzip`. Expected: FAIL (`todo!()` panics).

- [ ] **Step 3: Implement `GzipDecoder`** in `src/gzip.rs`: a state machine over member header
  (magic `1f 8b`, method `08`, FLG bits FTEXT/FHCRC/FEXTRA/FNAME/FCOMMENT; bits 5 to 7 set is
  `Corrupt("reserved gzip flag")`; with FHCRC, the 2 bytes must equal the low 16 bits of the CRC32
  of the header so far), raw inflate with `miniz_oxide::inflate::stream::inflate` and
  `InflateState::new_boxed(DataFormat::Raw)`, trailer (CRC32 via `crc32fast::Hasher`, ISIZE modulo
  2^32; mismatch is `Corrupt`), then the next member. Only take the input bytes the current part
  needs, so `consumed` never runs ahead. With `end_of_input`: no input at all, or input ending inside
  a member, is `Truncated`; after a complete member, bytes that do not start with `1f 8b` are
  `TrailingData`.

- [ ] **Step 4: Run the tests**
  Run: `cargo test -p htp-decompress --test gzip`. Expected: PASS.

- [ ] **Step 5: Commit.**

---

### Task 4: Vendored ruzstd patch

**Files:**
- Create: `vendor/ruzstd/` (ruzstd 0.9.1 unpacked from `https://static.crates.io/crates/ruzstd/ruzstd-0.9.1.crate`), `vendor/PATCHES.md`
- Modify: `vendor/ruzstd/src/decoding/decode_buffer.rs`, `vendor/ruzstd/src/decoding/frame_decoder.rs`, `vendor/ruzstd/src/decoding/ringbuffer.rs`
- Modify: `Cargo.toml` (`[patch.crates-io]`)

**Interfaces:**
- Produces (used by Task 5):
  - `FrameDecoder::read_decoded(&mut self, target: &mut [u8]) -> usize`: copies bytes decoded since
    the last call, oldest first, up to `target.len()`, without waiting for them to leave the window.
  - `FrameDecoder::decoded_unread(&self) -> usize`: bytes decoded but not yet handed out.
  - `DecodeBuffer::reset` no longer reserves the window.

Patch design: `DecodeBuffer` gets `emitted: usize`, the number of leading buffer bytes already handed
out. `read_decoded` copies from position `emitted`, feeds the copied bytes to `hash` (feature
`hash`), adds them to `emitted`, then drops `min(emitted, len - window_size)` leading bytes when
`len > window_size` and lowers `emitted` by the same amount. `reset` sets `emitted = 0` and drops
the `self.buffer.reserve(self.window_size)` line. The existing drain paths lower `emitted` by what
they drop (saturating) and are otherwise untouched; the zstd decoder uses only `read_decoded`.
`RingBuffer` gets `pub(crate) fn capacity(&self) -> usize` (returns `cap`) for the test.

- [ ] **Step 1: Unpack the crate, add `[patch.crates-io]`, write the failing unit tests** in
  `decode_buffer.rs`:

```rust
#[test]
fn read_decoded_hands_out_bytes_inside_the_window() {
    let mut buffer = DecodeBuffer::new(1024);
    buffer.push(b"0123456789");

    let mut target = [0u8; 64];
    let copied = buffer.read_decoded(&mut target);

    assert_eq!(&target[..copied], b"0123456789");
}

#[test]
fn read_decoded_keeps_the_window_for_back_references() { /* window 8: push "abcdefgh", read all, push "ij", read all, repeat(8, 3) then read: "cde" */ }

#[test]
fn reset_does_not_reserve_the_window() { /* reset(134_217_728): buffer.capacity() < 1_048_576 */ }

#[test]
fn read_decoded_feeds_the_checksum() { /* push 1000 bytes, read in 7-byte pieces: hash.finish() == XxHash64::oneshot(0, data) */ }
```

- [ ] **Step 2: Run them and see them fail**
  Run: `cargo test --manifest-path vendor/ruzstd/Cargo.toml --lib decode_buffer`. Expected: compile errors.

- [ ] **Step 3: Implement the patch** as designed above, plus the two `FrameDecoder` methods
  (delegating to `state.decoder_scratch.buffer`; `0` when there is no state). Write
  `vendor/PATCHES.md`: what changed, why (the spec's three ruzstd changes, measured numbers), and
  that it is to be offered upstream to KillingSpark/zstd-rs.

- [ ] **Step 4: Run the vendored crate's whole test suite**
  Run: `cargo test --manifest-path vendor/ruzstd/Cargo.toml`. Expected: PASS, including the
  crate's own tests.

- [ ] **Step 5: Commit.**

---

### Task 5: zstd decoder

**Files:**
- Create: `crates/htp-decompress/src/zstd.rs`, `crates/htp-decompress/tests/zstd.rs`
- Modify: `crates/htp-decompress/src/lib.rs` (wire `Format::Zstd`)

**Interfaces:**
- Consumes: Task 1 types, Task 2 support, Task 4 `read_decoded`/`decoded_unread`
- Produces: `pub(crate) struct ZstdDecoder`, `ZstdDecoder::new(limits: Limits) -> ZstdDecoder`, `impl FormatDecoder`

- [ ] **Step 1: Write the failing tests**: the five-test suite from Task 3 with `Format::Zstd`, plus:

```rust
#[test]
fn decodes_at_most_one_block_per_call() {
    let input = support::read("zstd/aaaa-long27.zst");

    let decoded = support::decode(Format::Zstd, &input, input.len(), 1_048_576, Limits::default());

    assert_eq!(decoded.output.len(), 16_777_216);
    assert!(decoded.max_produced_per_call <= 131_072);
}

#[test]
fn rejects_a_window_over_the_limit() { /* aaaa-long28.zst, default limits: Err(MemoryLimit { needed: 268_435_456 }), output empty */ }

#[test]
fn accepts_a_window_equal_to_the_limit() { /* aaaa-long27.zst with Limits { memory: 134_217_728 }: Ok */ }
```

- [ ] **Step 2: Run them and see them fail**
  Run: `cargo test -p htp-decompress --test zstd`. Expected: FAIL.

- [ ] **Step 3: Implement `ZstdDecoder`** in `src/zstd.rs`. Each `step` first hands out pending bytes
  with `read_decoded`, then completes at most one unit from input, taking only the bytes that unit
  needs into a small pending buffer:
  - frame start: 4-byte magic. `0x184D2A50..=0x184D2A5F` is a skippable frame (4-byte length, skip it).
    `0xFD2FB528` is a frame: header length is 4 + 1 + (1 unless Single_Segment) + `[0, 1, 2, 4]`
    [Dictionary_ID_flag] + FCS size (`[if single_segment 1 else 0, 2, 4, 8]`[FCS_flag]); a nonzero
    Dictionary_ID_flag is `Unsupported("zstd dictionary")`, reserved bit 3 is `Corrupt`. Pass exactly
    the header to `FrameDecoder::init`; its window-too-big error becomes `MemoryLimit { needed }`.
    Legacy magics `0xFD2FB522..=0xFD2FB527` are `Unsupported("legacy zstd frame")`. Any other
    magic: `Corrupt("not zstd")` before the first frame, `TrailingData` after one.
  - block: 3-byte header (bit 0 last, bits 1 to 2 type, bits 3 to 23 size; content length is `size`
    for raw and compressed, `1` for RLE; type 3 is `Corrupt`). Pass exactly the header, the content
    and, for the last block of a frame with Content_Checksum_flag, the 4 checksum bytes to
    `decode_blocks(slice, BlockDecodingStrategy::UptoBlocks(1))`.
  - frame end: once `is_finished()` and `decoded_unread() == 0`, compare `get_checksum_from_data()`
    with `get_calculated_checksum()` when present (`Corrupt("zstd checksum mismatch")`), then expect
    the next frame or the end. `FrameDecoder::set_max_window_size(limits.memory)` once at creation.
  - with `end_of_input`: no input at all, or input ending inside a frame, hands out every decoded
    byte and then fails with `Truncated`.

- [ ] **Step 4: Run the tests**
  Run: `cargo test -p htp-decompress --test zstd`. Expected: PASS. The cut test proves output at a
  cut is at least `zstd -dc`'s.

- [ ] **Step 5: Commit.**

---

### Task 6: xz decoder

**Files:**
- Create: `crates/htp-decompress/src/xz.rs`, `crates/htp-decompress/tests/xz.rs`
- Modify: `crates/htp-decompress/src/lib.rs` (wire `Format::Xz`)

**Interfaces:**
- Consumes: Task 1 types, Task 2 support
- Produces: `pub(crate) struct XzDecoder`, `XzDecoder::new(limits: Limits) -> XzDecoder`, `impl FormatDecoder`

- [ ] **Step 1: Write the failing tests**: the five-test suite from Task 3 with `Format::Xz`, except
  that in `cut_prefixes_keep_their_output` the length check is
  `decoded.output.len() + 16_384 >= cut.reference_len` (lzma-rust2 returns a few KiB less than
  `xz -dc` at a cut). Plus:

```rust
#[test]
fn reports_the_dictionary_needed() { /* hex-dict65.xz with Limits { memory: 67_108_864 }: Err(MemoryLimit { needed }) with needed >= 68_157_440 */ }
```

- [ ] **Step 2: Run them and see them fail**
  Run: `cargo test -p htp-decompress --test xz`. Expected: FAIL.

- [ ] **Step 3: Implement `XzDecoder`** in `src/xz.rs` with one `XzStream::new_mem_limit(false, limit_kib)`
  per stream (`limit_kib = ceil(limits.memory / 1024)`, saturating at `u32::MAX - 1`), handling
  concatenation itself: after `Status::StreamEnd`, zero bytes are padding and must come in multiples
  of 4 (otherwise `Corrupt("xz stream padding")`); next comes the xz magic `fd 37 7a 58 5a 00` (a new
  `XzStream`) or, anything else, `TrailingData`. Use `Action::Finish` only with `end_of_input`.
  Error mapping by `std::io::ErrorKind`: `UnexpectedEof` to `Truncated`; `OutOfMemory` to
  `MemoryLimit { needed: memory_usage() KiB * 1024 }`; `Unsupported` to `Unsupported`; an
  `InvalidData` whose message contains `not supported` (multiple filters) to `Unsupported`; the rest
  to `Corrupt`. On an error, `total_out()` before and after the call gives what the failing call
  produced.

- [ ] **Step 4: Run the tests and the whole core suite**
  Run: `cargo test -p htp-decompress && cargo clippy --all-targets -- -D warnings`. Expected: PASS.

- [ ] **Step 5: Commit.**

---

### Task 7: wasm module and its TypeScript binding

**Files:**
- Create: `crates/htp-decompress-wasm/Cargo.toml`, `crates/htp-decompress-wasm/src/lib.rs`
- Modify: `Cargo.toml` (add the crate to `members`)
- Create: `scripts/build-wasm.sh`, `scripts/embed-wasm.mjs`
- Create: `npm/package.json`, `npm/tsconfig.json`, `npm/tsconfig.build.json`, `npm/vitest.config.ts`
- Create: `npm/src/format.ts`, `npm/src/errors.ts`, `npm/src/modules.ts`, `npm/src/wasm-decoder.ts`
- Test: `npm/test/wasm-decoder.test.ts`, `npm/test/fixtures.ts`

**Interfaces:**
- Consumes: `Decoder`, `Limits`, `Error`, `Status` (Task 1)
- Produces, wasm exports (one format per build, chosen by feature `gzip`, `xz` or `zstd`; any other
  number of features is a `compile_error!`), all `#[unsafe(no_mangle)] pub extern "C"`:
  - `htp_alloc(len: usize) -> *mut u8`, `htp_free(ptr: *mut u8, len: usize)`
  - `htp_decoder_new(memory_limit: f64) -> *mut Decoder`, `htp_decoder_free(decoder: *mut Decoder)`
  - `htp_decoder_process(decoder: *mut Decoder, input: *const u8, input_len: usize, output: *mut u8, output_len: usize, end_of_input: u32) -> i32`:
    `0` continue, `1` done, `-1` truncated, `-2` corrupt, `-3` unsupported, `-4` memory-limit, `-5` trailing-data
  - `htp_result() -> *const ResultArea`, `#[repr(C)] struct ResultArea { consumed: u32, produced: u32, message_ptr: u32, message_len: u32, needed: f64 }`
    (byte offsets 0, 4, 8, 12, 16; filled by every process call; the message is the `&'static str`
    of `Corrupt`/`Unsupported`, else empty)
- Produces, TypeScript (used by Tasks 8, 9):
  - `npm/src/format.ts`: `export type DecompressionFormat = 'gzip' | 'xz' | 'zstd';`, `export const FORMATS: readonly DecompressionFormat[]`
  - `npm/src/errors.ts`: `export type HtpDecompressionErrorCode = 'truncated' | 'corrupt' | 'unsupported' | 'memory-limit' | 'trailing-data' | 'internal';`,
    `export class HtpDecompressionError extends TypeError { readonly code: HtpDecompressionErrorCode; constructor(code, message: string, options?: ErrorOptions) }` with `name = 'HtpDecompressionError'`
  - `npm/src/modules.ts`: `export function loadModule(format: DecompressionFormat): Promise<WebAssembly.Module>` (cached per format; a `switch` with three literal `import('./wasm/<format>.js')` calls; base64 decoded with `atob`)
  - `npm/src/wasm-decoder.ts`: `export const OUTPUT_CHUNK_BYTES = 65_536; export const INPUT_SLICE_BYTES = 65_536;`
    `export class WasmDecoder { static create(format: DecompressionFormat, memoryLimit: number): Promise<WasmDecoder>; process(input: Uint8Array, endOfInput: boolean): { consumed: number; produced: number; done: boolean }; takeOutput(): Uint8Array<ArrayBuffer>; memoryBytes(): number; dispose(): void }`
    (`process` passes at most `INPUT_SLICE_BYTES` per call, throws `HtpDecompressionError` for
    negative codes, turns a `WebAssembly.RuntimeError` into code `internal`, and re-reads
    `memory.buffer` after each call since growth detaches old views)

`scripts/build-wasm.sh` builds each format with
`cargo build --release --target wasm32-unknown-unknown -p htp-decompress-wasm --no-default-features --features <format>`
(zstd with `CARGO_PROFILE_RELEASE_OPT_LEVEL=3`, measured 1.5x faster for about 5 KB gzipped more),
copies `target/wasm32-unknown-unknown/release/htp_decompress_wasm.wasm` to `npm/build/<format>.wasm`
after each build, then runs `node scripts/embed-wasm.mjs`, which writes
`npm/src/wasm/<format>.ts` (`export default '<base64>';`) and prints a size table (raw and gzip -9)
for `$GITHUB_STEP_SUMMARY`. `npm/package.json` scripts: `build:wasm` (`bash ../scripts/build-wasm.sh`),
`build` (`npm run build:wasm && tsc -p tsconfig.build.json`), `test` (`vitest run`),
`typecheck` (`tsc -p tsconfig.json --noEmit`). `tsconfig.json`: `target` and `lib` ES2022 plus DOM,
`module` `nodenext`, `strict`. `test/fixtures.ts` (test-only) exports
`interface Fixture { file: string; format: DecompressionFormat; original: Uint8Array; expect: 'done' | HtpDecompressionErrorCode; limit: number | null; cuts: { at: number; referenceLength: number }[] }`,
`fixtures(format: DecompressionFormat): Fixture[]`, `readFixture(file: string): Uint8Array` and
`decodeAll(decoder: WasmDecoder, input: Uint8Array): Uint8Array` (drives `process`/`takeOutput` to
done, rethrowing errors). It reads `../fixtures/manifest.json` with `node:fs` and rebuilds originals
like Task 2's support (repeat sources included).

- [ ] **Step 1: Write the failing tests** in `npm/test/wasm-decoder.test.ts`:

```ts
describe.each(FORMATS)('%s WasmDecoder', (format) => {
  it('decodes every done fixture', async () => { /* drive process/takeOutput to done: output equals original */ });
  it('maps error codes', async () => { /* each error fixture: rejects with HtpDecompressionError whose code equals the manifest's expect */ });
  it('passes at most one input slice per call', async () => { /* process(1 MiB input): consumed <= 65_536 */ });
});

it('keeps zstd memory near the decoded size', async () => {
  const decoder = await WasmDecoder.create('zstd', 134_217_728);
  const before = decoder.memoryBytes();

  const output = decodeAll(decoder, readFixture('zstd/aaaa-long27.zst'));

  expect(output.length).toBe(16_777_216);
  expect(decoder.memoryBytes() - before).toBeLessThan(41_943_040);
});

it('keeps xz memory near the decoded size', async () => { /* same as above with 'xz' and xz/aaaa-dict65.xz (65 MiB dictionary declared): growth < 41_943_040 */ });

it('gives every decoder its own memory', async () => { /* two zstd decoders interleaved on different fixtures both decode correctly; memory objects differ */ });
```

- [ ] **Step 2: Run them and see them fail**
  Run: `cd npm && npm install && npm run build:wasm && npm test`. Expected: the wasm crate fails to
  build or the tests fail.

- [ ] **Step 3: Implement the wasm crate, the scripts and the four TypeScript modules** as specified
  in Interfaces.

- [ ] **Step 4: Build and test**
  Run: `cd npm && npm run build:wasm && npm test && npm run typecheck`. Expected: PASS; the size
  table shows roughly gzip 31 KB, xz 79 KB, zstd 61 KB raw (a large jump means a dependency or
  feature slipped in).

- [ ] **Step 5: Commit.**

---

### Task 8: `HtpDecompressionStream`

**Files:**
- Create: `npm/src/stream.ts`, `npm/src/time-slice.ts`
- Test: `npm/test/stream.test.ts`

**Interfaces:**
- Consumes: `WasmDecoder`, `OUTPUT_CHUNK_BYTES` (Task 7), `HtpDecompressionError` (Task 7), `DecompressionFormat` (Task 7)
- Produces:
  - `export interface HtpDecompressionOptions { memoryLimit?: number }`
  - `export const DEFAULT_MEMORY_LIMIT = 134_217_728;`
  - `export class HtpDecompressionStream { constructor(format: DecompressionFormat, options?: HtpDecompressionOptions); readonly readable: ReadableStream<Uint8Array<ArrayBuffer>>; readonly writable: WritableStream<BufferSource> }`
  - `npm/src/time-slice.ts` (internal): `export const timeSlice = { budgetMs: 8 }; export function yieldIfDue(): Promise<void> | undefined` (a `setTimeout(0)` promise once `budgetMs` has passed since the last yield)

Behaviour is the spec's "npm package" section. The pull algorithm, which the tests alone do not fix:
each `pull(controller)` loads the decoder if needed, then loops: with no queued input and the
writable side open, wait for a write or close; call `process(head chunk from its offset, endOfInput =
writable closed and this is the last queued chunk)`; advance the head chunk by `consumed` and resolve
its `write()` once it is used up; on `produced > 0` enqueue `takeOutput()` and return; on `done` close
the readable side and resolve `close()`; on an error, error the readable side with it, reject
`close()` and pending writes, and dispose; await `yieldIfDue()` between calls. The readable side uses
`highWaterMark: 0`; both sides keep their controllers so `cancel` can error the writable side and
`abort` the readable side. Written chunks are copied into a fresh `Uint8Array` on write.

- [ ] **Step 1: Write the failing tests** in `npm/test/stream.test.ts` (helpers: `pipeFixture(file,
  { chunkBytes })` returning the bytes read and the error; fixture data from Task 7's `test/fixtures.ts`):

```ts
describe.each(FORMATS)('%s stream', (format) => {
  it('decodes through pipeThrough', ...);                 // every done fixture, 16_384-byte writes: bytes equal original, no error
  it('decodes one whole-file write', ...);                // single write of the whole file: equals original
  it('delivers all output, then reports a cut', ...);     // each cut: bytes equal a WasmDecoder run on the same prefix; error.code 'truncated'
  it('delivers output before damage', ...);               // corrupt fixture: original starts with bytes; error.code 'corrupt'
  it('reports trailing data', ...);                       // error.code 'trailing-data', bytes equal original
});

it('throws a TypeError for an unknown format', () => { expect(() => new HtpDecompressionStream('brotli' as never)).toThrow(TypeError); });
it('throws a RangeError for a bad memoryLimit', () => { for (const memoryLimit of [0, -1, Number.NaN, 1.5, Infinity]) expect(() => new HtpDecompressionStream('xz', { memoryLimit })).toThrow(RangeError); });
it('errors both sides on a non-BufferSource write', ...);  // write('text'): write rejects with TypeError, reader rejects with the same error
it('reports the memory limit before any output', ...);     // zstd/aaaa-long27.zst with memoryLimit 67_108_864: 0 bytes, code 'memory-limit'
it('decodes only as far as the reader pulls', ...);        // spy on WasmDecoder.prototype.process; read 65_536 bytes of zstd/aaaa-long27.zst, await 5 macrotasks: calls <= 3
it('reads an exact prefix and stops', ...);                // gzip/aaaa.gz: read until 65_536 bytes, cancel: bytes equal 'a\n' x 32_768
it('passes a readable cancel upstream', ...);              // cancel(reason) mid-decode: pending writer.write rejects with reason
it('stops on an aborted pipeThrough signal', ...);         // pipeThrough(stream, { signal }); abort(): reader rejects with signal.reason
it('passes a writable abort downstream', ...);             // writer.abort(reason): reader rejects with reason
it('runs streams concurrently and after a cancel', ...);   // cancel one xz stream, then two xz streams decode interleaved correctly
it('yields to the event loop', ...);                       // timeSlice.budgetMs = 0; a setTimeout(0) set before reading fires before the last chunk of zstd/aaaa-long27.zst arrives
it('ignores empty chunks', ...);                           // empty writes between and after real ones: equals original, clean end
it('copies written bytes', ...);                           // overwrite each written buffer with zeros after write() resolves: still equals original
it('cancel during module load settles everything', ...);   // cancel before the first pull finishes loading: reader, writer and close promises all settle, a later stream works
it('accepts every buffer source', ...);                    // ArrayBuffer, DataView, Uint16Array view at an even offset over the same bytes: all equal original
```

- [ ] **Step 2: Run them and see them fail**
  Run: `cd npm && npx vitest run test/stream.test.ts`. Expected: FAIL (module not found).

- [ ] **Step 3: Implement `stream.ts` and `time-slice.ts`** per Interfaces and the pull algorithm.
  The constructor throws `TypeError` for a format outside `FORMATS` and `RangeError` unless
  `memoryLimit` is a positive safe integer; it starts no work.

- [ ] **Step 4: Run all package tests**
  Run: `cd npm && npm test && npm run typecheck`. Expected: PASS.

- [ ] **Step 5: Commit.**

---

### Task 9: Format detection, package entry, build output and usage docs

**Files:**
- Create: `npm/src/detect.ts`, `npm/src/index.ts`, `npm/README.md`, `README.md`
- Test: `npm/test/detect.test.ts`, `npm/test/package.test.ts`

**Interfaces:**
- Consumes: Tasks 7 and 8 exports
- Produces: `export const DETECT_FORMAT_BYTES = 6; export function detectFormat(head: Uint8Array): DecompressionFormat | null;`
  and `npm/src/index.ts` exporting exactly `HtpDecompressionStream`, `HtpDecompressionOptions`,
  `DEFAULT_MEMORY_LIMIT`, `HtpDecompressionError`, `HtpDecompressionErrorCode`,
  `DecompressionFormat`, `detectFormat`, `DETECT_FORMAT_BYTES`

- [ ] **Step 1: Write the failing tests**:

```ts
it.each([
  [[0x1f, 0x8b, 0x08], 'gzip'],
  [[0x1f, 0x8b, 0x07], null],
  [[0xfd, 0x37, 0x7a, 0x58, 0x5a, 0x00], 'xz'],
  [[0xfd, 0x37, 0x7a, 0x58, 0x5a], null],
  [[0x28, 0xb5, 0x2f, 0xfd], 'zstd'],
  [[0x5e, 0x2a, 0x4d, 0x18], 'zstd'],
  [[0x60, 0x2a, 0x4d, 0x18], null],
  [[], null],
])('detects %j as %s', (bytes, expected) => {
  expect(detectFormat(new Uint8Array(bytes))).toBe(expected);
});
```

  `package.test.ts`: after `npm run build`, `npm pack --dry-run --json` lists only `package.json`,
  `README.md`, `LICENSE` and `dist/**` (including `dist/wasm/gzip.js`, `xz.js`, `zstd.js` and
  `dist/index.d.ts`); `import('../dist/index.js')` exposes exactly the names in Interfaces.

- [ ] **Step 2: Run them and see them fail**
  Run: `cd npm && npx vitest run test/detect.test.ts test/package.test.ts`. Expected: FAIL.

- [ ] **Step 3: Implement** `detect.ts`, `index.ts`, the `package.json` fields from Global
  Constraints (`exports` with `types` and `default` for `.`, `files: ["dist"]`, copying `LICENSE`
  into `npm/` at build time), `npm/README.md` (install, `pipeThrough` example, options, error codes,
  differences from `DecompressionStream`: no lost output, error codes, multi-member gzip) and the root
  `README.md` (layout, build and test commands, fixture regeneration).

- [ ] **Step 4: Build and run everything**
  Run: `cd npm && npm run build && npm test`. Expected: PASS.

- [ ] **Step 5: Commit.**

---

### Task 10: Fuzz target

**Files:**
- Create: `crates/htp-decompress/fuzz/Cargo.toml`, `crates/htp-decompress/fuzz/fuzz_targets/process.rs`

**Interfaces:**
- Consumes: `Decoder`, `Format`, `Limits` (Task 1)

The target reads, from the fuzz input: byte 0 picks the format (modulo 3), byte 1 seeds the input and
output chunk sizes (1 to 4,096), the rest is the compressed data. It runs `process` to `Done` or
`Err` with `Limits { memory: 16_777_216 }` and panics if a call makes no progress three times in a
row or produces more than `output.len()`. Seed corpus: the fixtures, each prefixed with its format
byte.

- [ ] **Step 1: Install the tooling** (`cargo install cargo-fuzz`, `rustup toolchain install nightly`).
- [ ] **Step 2: Write the target and run it for each format**
  Run: `cd crates/htp-decompress/fuzz && cargo +nightly fuzz run process -- -max_total_time=300`.
  Expected: no crashes. Any crash found gets a regression test in the owning format's test file and
  a fix before going on.
- [ ] **Step 3: Commit.**

---

### Task 11: CI and release workflows

**Files:**
- Create: `.github/workflows/ci.yml`, `.github/workflows/release.yml`, `docs/releasing.md`

**Interfaces:**
- Consumes: the commands from Tasks 1 to 9

`ci.yml` on push and pull request, `ubuntu-latest`, `actions/checkout@v7`,
`actions-rust-lang/setup-rust-toolchain@v2` (reads `rust-toolchain.toml`), `actions/setup-node`
with Node 24: `cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings`,
`cargo test --locked`, `cargo test --manifest-path vendor/ruzstd/Cargo.toml --locked`, then in `npm/`
`npm ci`, `npm run build`, `npm test`, `npm run typecheck`, and the size table appended to
`$GITHUB_STEP_SUMMARY`.

`release.yml` on tags `v*`: the same build and tests; fail unless the tag equals `v` plus the version in
both `npm/package.json` and `crates/htp-decompress/Cargo.toml`; `npm publish --provenance --access public`
from `npm/` (permissions `id-token: write`, `contents: write`; npm trusted publishing, no token
secret); `npm pack` and `softprops/action-gh-release@v3` with the tarball attached.

`docs/releasing.md`: bump both versions; first release by hand (`npm login`, `cd npm && npm run build
&& npm publish --access public`), then on npmjs.com add a trusted publisher for repository
`four-random-common-words/htp-decompress`, workflow `release.yml`; later releases by pushing a tag;
moving the repository to the hashtopolis organization means updating that trusted publisher and the
`repository` field.

- [ ] **Step 1: Write both workflows and `docs/releasing.md`.**
- [ ] **Step 2: Run the CI commands locally in order**
  Run each `run:` line of `ci.yml` from the repository root (and `npm/`). Expected: all pass. If
  `actionlint` is available, `actionlint` reports nothing.
- [ ] **Step 3: Commit.** The owner then creates the GitHub repository, pushes,
  checks the first CI run, and publishes `0.1.0` by hand as in `docs/releasing.md`. The web-ui plan
  starts after that.
