# Vendored crates

## ruzstd 0.9.1

`vendor/ruzstd` is ruzstd 0.9.1 as published on crates.io
(`https://static.crates.io/crates/ruzstd/ruzstd-0.9.1.crate`, SHA-256
`69e4ce4a039f42bf25920b95667e4d789e98749ab11bbb8d2c2936f788421bb5`), unpacked so its files sit
directly in `vendor/ruzstd/`, plus the patch below. The root `Cargo.toml` points
`[patch.crates-io]` at it. The first commit that adds the directory holds the crate unchanged, so
`git diff <that commit> -- vendor/ruzstd` shows exactly the patch.

The changes are to be offered upstream to KillingSpark/zstd-rs. Once a release contains them, drop
the vendored copy and the `[patch.crates-io]` entry.

### Why

ruzstd's public API hands out decoded bytes only once they are older than the window while a frame
is unfinished (`DecodeBuffer::read` drains up to `can_drain_to_window_size`). For a stream preview
that reads a cut-off prefix of a file, that loses output and wastes memory:

- No output lag: reading 16 MiB from a file with a 37.8 MiB window grew wasm memory by 148 MiB,
  because 54 MiB had to be decoded first and the ring buffer doubled past it.
- Everything out at a cut-off: a 1 MiB prefix of a `zstd -3` file gave 0 bytes, where `zstd -dc`
  gives 1.97 MB.
- No window reserved up front: `DecodeBuffer::reset` reserved the whole declared window, so every
  frame after the first allocated it before producing any output (the first frame goes through
  `DecodeBuffer::new`, which does not). The ring buffer already grows on demand.

### What changed

`src/decoding/decode_buffer.rs`

- `DecodeBuffer` has a new private field `emitted`: how many leading buffer bytes have already been
  handed out by `read_decoded`.
- New `DecodeBuffer::read_decoded(&mut self, target: &mut [u8]) -> usize`: copies bytes from
  position `emitted` (across the ring buffer's wraparound), feeds them to `hash` (feature `hash`),
  adds them to `emitted`, then drops `min(emitted, len - window_size)` leading bytes when
  `len > window_size` and lowers `emitted` by the same amount. The window stays in the buffer for
  later back-references; handed out bytes beyond it are released.
- New `DecodeBuffer::decoded_unread(&self) -> usize`: `len - emitted`.
- `reset` sets `emitted = 0` instead of calling `self.buffer.reserve(self.window_size)`.
- The existing drain paths are otherwise unchanged; they lower `emitted` by what they drop
  (saturating): `drain` resets it with the buffer, and `drain_to` (behind `read`, `read_all`,
  `drain_to_window_size`, `drain_to_window_size_writer` and `drain_to_writer`) does it in its
  `DrainGuard`, so partial writes and errors keep it right.
- Seven unit tests for the above.

`src/decoding/frame_decoder.rs`

- New `FrameDecoder::read_decoded(&mut self, target: &mut [u8]) -> usize` and
  `FrameDecoder::decoded_unread(&self) -> usize`, delegating to the frame's `DecodeBuffer` (`0`
  without a frame).

`src/decoding/ringbuffer.rs`

- New `RingBuffer::capacity`, compiled for tests only, so a test can check that `reset` does not
  reserve the window.

`src/lib.rs`

- `mod tests;` is commented out: those tests read `decodecorpus_files`, `dict_tests` and
  `fuzz_decodecorpus`, which the published crate excludes, so the lib test target does not compile
  as published. The module only holds tests.

### Using it

Read a frame's output either with `read_decoded` or with the collect and read functions
(`collect`, `collect_to_writer`, `Read::read`, `decode_from_to`, `decode_all`), not both: each hands
out and hashes the bytes it takes, so mixing them repeats bytes in the output and in the checksum.
`read_decoded` hands out everything decoded so far, so feed `decode_blocks` one block at a time
(`BlockDecodingStrategy::UptoBlocks(1)`) and call it after every block to bound the output per call
and keep the buffer near the window size.

### Testing

`cargo test --locked --manifest-path vendor/ruzstd/Cargo.toml` runs the crate's remaining tests
(53 unit tests including the new ones, 7 doc tests) with the crate's own `Cargo.lock`. ruzstd's
dev-dependencies include the `zstd` crate, whose `zstd-sys` builds C code, so this needs a C
compiler. Building htp-decompress itself does not: dev-dependencies of a patched crate are not part
of its build.
