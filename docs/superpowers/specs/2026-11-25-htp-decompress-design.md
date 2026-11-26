# htp-decompress design

Date: 2026-11-25. Status: draft, waiting for approval.

## Why

The Hashtopolis web-ui file preview (hashtopolis/web-ui PR 906 for gzip, branch
`2644-enhancement-preview-xz-and-zstd-files` for xz and zstd) fetches the first 1 MiB of a
compressed file (`PEEK_BYTES`) and decompresses at most 16 MiB of it (`MAX_DECOMPRESSED_BYTES`).
A cut-off prefix is the normal case, not an error. gzip uses the browser's `DecompressionStream`.
An experiment on the second branch, not meant to be merged, added xz with xz-decompress 0.2.3 and
zstd with fzstd 0.1.1. It showed that those libraries force workarounds and leave gaps:

- xz-decompress: a module-wide mutex allows one xz stream at a time, and an unfinished,
  uncancelled stream blocks every later preview. One shared wasm instance keeps its memory for the
  life of the tab (about 73 MiB after one `xz -9` preview). An abort during the first-use setup leaks
  a context. SHA-256 checks, BCJ and delta filters, and dictionaries over 64 MiB fail before any
  output with an opaque "get_next_output failed with error code N". It is CommonJS
  (`allowedCommonJsDependencies`) and has no options.
- fzstd: a synchronous push API, so the preview yields by hand every 128 KiB and cannot interrupt a
  push. It allocates the whole window (or the whole content size of a single-segment frame) before
  decoding, hence the preview's 32 MiB refusal and hand-written frame-header parser. It cannot be told
  that a prefix ended, does not verify checksums, and is unmaintained (open issues #22 silent
  corruption, #24 a crafted frame spins the CPU).
- Chrome's gzip `DecompressionStream` decodes only the first member of a multi-member file and then
  errors; hashcat reads all members. Any erroring web stream clears its queue, so output not yet read
  is lost (Chrome delivered 128 KiB of a 2.4 MB member).
- The preview has three decode paths for three formats.

This repository replaces all of that with one package: decoders written in Rust on maintained crates
(ruzstd, lzma-rust2, miniz_oxide), compiled to WebAssembly, behind a stream class that works like
`DecompressionStream`. The owner expects the Rust crates to be better maintained than the JS
libraries over time.

## Goals

- `new HtpDecompressionStream('gzip' | 'xz' | 'zstd', options)` returns a `{ readable, writable }`
  pair that works with `pipeThrough`, respects backpressure and passes cancellation through, like
  `DecompressionStream`.
- Never lose decoded output: every byte decoded before a cut-off or damage reaches the reader before
  the stream errors.
- Errors say why: cut off, damaged, unsupported, over the memory limit, or trailing data.
- Decode only as far as the reader pulls, in small bounded steps, so a reader that stops stops the
  work, and a cancel lands within milliseconds.
- No global state: streams run concurrently, and each stream's wasm memory is released when it ends.
- A configurable memory limit for the zstd window and the xz dictionary, with a clear error.
- hashcat parity for what it reads: gzip with several members; xz with CRC32, CRC64, SHA-256 or no
  check, BCJ and delta filters, large dictionaries, several streams and stream padding; zstd with
  several frames, skippable frames (pzstd), checksum verification and large windows within the limit.
- Zero configuration in the consuming app: no bundler loader, no asset copying, no CommonJS
  allowance, no C toolchain anywhere.

## Non-goals

- Compression.
- Formats other than gzip, xz and zstd (deflate, deflate-raw, brotli, bzip2, lz4, zip, `.lzma`,
  lzip).
- zstd dictionaries and legacy (pre-1.0) zstd frames: reported as unsupported.
- Running in a Web Worker. The API does not rule it out later.
- Node.js stream (`stream.Transform`) APIs, random access or seeking.
- Publishing the Rust crate to crates.io, for now.

## Anti-goals

The work fails, even if it decodes, if it brings back any of these:

- a lock or shared instance that lets one stream block or leak into another;
- output lost at a cut-off or damage point;
- wasm memory that stays allocated after a stream ends, or memory reserved for a declared window or
  dictionary before the output needs it;
- configuration the consuming app has to add (angular.json loaders, assets, CSP changes beyond
  `'wasm-unsafe-eval'` if a `script-src` is ever set);
- a decode step whose work is not bounded (one call expanding a slice into hundreds of MiB).

## Constraints

- Pure Rust: no C code, so no clang with a wasm32 backend (Apple's clang has none).
- Target `wasm32-unknown-unknown`, stable Rust, pinned in `rust-toolchain.toml`.
- Runtime targets: the browsers in web-ui's `.browserslistrc` (last Chrome, last Firefox, Firefox
  ESR, last two Edge, Safari and iOS) and Node 24 (web-ui's Vitest tests).
- ESM only, TypeScript types included, no runtime dependencies.
- License GPL-3.0 like the other Hashtopolis repositories. ruzstd (MIT), lzma-rust2 (Apache-2.0),
  miniz_oxide (MIT/Apache-2.0/Zlib) and crc32fast (MIT/Apache-2.0) are compatible.
- Follow the workspace code style (blank-line rules in the Hashtopolis CLAUDE.md).

## Approaches

**A. Stream class like `DecompressionStream`, decoding in bounded, time-sliced steps on the calling
thread (picked).** One wasm instance per stream, one wasm module per format loaded on first use,
wasm inlined in the package, hand-written TypeScript over a few raw wasm exports. A cancel lands
between steps (a few ms apart), not inside one.

**B. The same class with decoding in a Web Worker.** Gives a hard cancel through `terminate()` and
keeps decode work off the main thread, but the consuming app must bundle a worker file (or the
package needs a `blob:` worker and a CSP change), every chunk crosses `postMessage`, and there is
more code to maintain. A 16 MiB preview decodes in 0.1 to 0.5 s, so it buys little.

Dropped: a one-shot `decode(prefix)` function shaped like the preview's current interface (the
owner wants the `DecompressionStream` shape); wasm-bindgen's generated loader (its separate `.wasm`
file 404s under esbuild, evanw/esbuild#795, and it needs a pinned CLI in CI).

The owner picked A.

## Design

### Repository layout

```
Cargo.toml              workspace
rust-toolchain.toml     Rust 1.100.0 + wasm32-unknown-unknown
crates/htp-decompress/  core library: Decoder for all three formats, features gzip/xz/zstd
crates/htp-decompress-wasm/  cdylib: raw wasm exports over the core, built once per format
vendor/ruzstd/          patched ruzstd until the patch is upstream (see zstd below)
npm/                    the npm package: TypeScript sources, generated wasm modules, tests
scripts/                build script: wasm per format -> base64 modules -> tsc -> npm pack
fixtures/               compressed test files, the generator script and a README
.github/workflows/      ci.yml, release.yml
```

### Core crate (`htp-decompress`)

One push decoder for all formats, with no I/O of its own:

```rust
pub enum Format {
    Gzip,
    Xz,
    Zstd,
}

pub struct Limits {
    /// Largest zstd window or xz dictionary a file may declare, in bytes.
    pub memory: u64,
}

impl Decoder {
    pub fn new(format: Format, limits: Limits) -> Decoder;

    /// Decodes from `input` into `output`.
    pub fn process(
        &mut self,
        input: &[u8],
        output: &mut [u8],
        end_of_input: bool,
    ) -> Result<Progress, Error>;
}

pub struct Progress {
    pub consumed: usize,
    pub produced: usize,
    pub status: Status,
}

pub enum Status {
    /// More input, more output room, or another call is needed.
    Continue,
    /// `end_of_input` was set and every stream, member or frame ended cleanly.
    Done,
}

pub enum Error {
    Truncated,
    Corrupt(&'static str),
    Unsupported(&'static str),
    MemoryLimit { needed: u64 },
    TrailingData,
}
```

Contract, the same for every format:

- `process` never writes past `output`, and does a bounded amount of work per call: at most one
  zstd block, or at most `output.len()` bytes of output otherwise. The caller loops.
- An error never discards output. While decoded bytes are still pending, `process` returns them
  with `Ok`; the `Err` comes on a later call that has nothing left to hand over.
- After an `Err`, every later call returns the same `Err`.
- Several streams, members or frames are decoded in sequence. Between them, without
  `end_of_input`, the status is `Continue`, since more may follow.
- With `end_of_input`, input that ends inside a stream, member or frame gives `Truncated` (after
  the output). Bytes after the last complete one that do not start a new one give `TrailingData`,
  except xz stream padding (zero bytes in multiples of four), which is valid.

#### gzip (miniz_oxide)

- Parse the member header ourselves: magic `1f 8b`, method `08`, flags `FEXTRA`, `FNAME`,
  `FCOMMENT`, `FHCRC`; reserved flag bits are `Corrupt`.
- Inflate with `miniz_oxide::inflate::stream` (raw deflate), bounded by the output slice.
- Check the trailer: CRC32 (crc32fast) and ISIZE (length modulo 2^32).
- Then expect another member or the end.

Measured: miniz_oxide inflate alone builds to 31 KB raw, 13 KB gzipped.

#### xz (lzma-rust2 0.25, `default-features = false, features = ["std", "xz"]`)

- One `XzStream::new_mem_limit(false, limit_kib)` per stream. The decoder handles concatenation
  itself: after a stream, zero bytes are padding in multiples of four, then comes the xz magic (a
  new stream) or trailing data. Checks (CRC32, CRC64, SHA-256) are verified, BCJ and delta filters
  are supported, and the dictionary grows with the output instead of being allocated up front.
- `limit_kib` is the memory limit in KiB plus the fixed 40 KiB of working memory lzma-rust2 0.25.0
  counts on top of the dictionary, so the limit compares exactly against the declared dictionary.
- `Action::Run` while input may follow, `Action::Finish` with `end_of_input`.
- Map its errors: unexpected end of input to `Truncated`, its memory-limit error to `MemoryLimit`,
  unsupported filter chains to `Unsupported`, everything else to `Corrupt`. After a failing call,
  `total_out()` tells how much that call produced.
- Known limit: `XzStream` accepts at most one BCJ or delta filter in front of LZMA2 per block;
  longer chains are `Unsupported`. It decodes a pass of up to 64 KiB before it hands any of it out,
  and loses the pass that runs into a cut or damage, so the output falls short of `xz -dc` by up to
  one 64 KiB decode pass, at a cut in highly repetitive data and before damage (measured: 63,609
  bytes at a cut of `aaaa-dict65.xz`, at most 38 bytes at cuts of `hex.xz`). Each `XzStream` call
  gets at most 4 KiB of input, which keeps the pass lost before damage to what 4 KiB decodes to
  (measured on `hex.xz` with flipped bytes: at most 7.7 KB, against 65 KB with 64 KiB of input per
  call) at no measurable cost in speed.

#### zstd (ruzstd 0.9, `default-features = false, features = ["std", "hash"]`)

- One `FrameDecoder` reused per frame, `set_max_window_size(limits.memory)`; a larger declared
  window is `MemoryLimit`.
- Skippable frames (`5? 2a 4d 18`) are skipped. A frame with a dictionary id is `Unsupported`.
- Feed ruzstd one block at a time: read the 3-byte block header (last flag, type, size) and pass
  exactly that block, so one call decodes at most one block (at most 128 KiB of output). Without
  this, `decode_from_to` decodes every block in the slice it gets, and a highly repetitive 16 KiB
  slice can expand to hundreds of MiB in one call.
- Verify the content checksum when the frame has one: compare `get_checksum_from_data()` with
  `get_calculated_checksum()`. ruzstd does not do this itself.
- Watch out: when a frame's last block is decoded but fewer than 4 checksum bytes are available,
  `decode_from_to` reports 4 bytes consumed anyway. Pass the checksum bytes only when all 4 are
  there.

ruzstd needs a patch with three behaviour changes:

1. **No output lag.** Its public API hands out only bytes older than the window while a frame is
   unfinished (`DecodeBuffer::read` drains up to `can_drain_to_window_size`). Decoded bytes must be
   handed out as soon as their block is decoded, while the window stays in the buffer for later
   back-references. This needs the buffer to copy bytes out without removing them (an "emitted up
   to" position) and to keep the checksum hash correct. Measured cost of the lag: reading 16 MiB
   from a file with a 37.8 MiB window grew wasm memory by 148 MiB, because 54 MiB had to be decoded
   first and the ring buffer doubled past it.
2. **Everything out at a cut-off.** Measured: a 1 MiB prefix of a `zstd -3` file gave 0 bytes,
   where `zstd -dc` gives 1.97 MB. The research verified an 8-line
   `FrameDecoder::drain_unfinished(&mut self, target: &mut [u8]) -> usize` that calls the buffer's
   `read_all`; with it, prefix output matches `zstd -dc` byte for byte. Change 1 may make this
   unnecessary.
3. **No window reserved up front.** `DecodeBuffer::reset` calls `self.buffer.reserve(window_size)`,
   so every frame after the first reserves its whole declared window before any output (the first
   frame goes through `DecodeBuffer::new`, which does not). The ring buffer already grows on demand,
   so removing that reserve is enough; tested on single-frame, multi-frame (pzstd) and 37.8 MiB
   window files, all still decoding correctly.

With these, zstd memory follows the decoded output (up to the window), as xz's already does. Keep the
patched copy in `vendor/ruzstd` through `[patch.crates-io]`, and offer the changes upstream
(KillingSpark/zstd-rs).

### wasm module (`htp-decompress-wasm`)

- A `cdylib` built three times, once per format feature, giving `gzip.wasm`, `xz.wasm` and
  `zstd.wasm`. Separate modules keep a gzip preview from loading the xz and zstd code.
- Release profile: `lto = true`, `codegen-units = 1`, `panic = "abort"`, `strip = true`.
- Raw `extern "C"` exports, no wasm-bindgen. Roughly: `memory`, buffer allocate and free, decoder
  create (format fixed by the build, memory limit as a parameter), process (input and output
  pointers and lengths, end-of-input flag; consumed, produced and status written to a small result
  area in linear memory), and an error code.
- A panic or allocation failure traps (`WebAssembly.RuntimeError`); the TypeScript side turns that
  into an `internal` error. Since every stream has its own instance, a trapped instance is simply
  dropped.

### npm package (`htp-decompress`)

Public API:

```ts
export type DecompressionFormat = 'gzip' | 'xz' | 'zstd';

export interface HtpDecompressionOptions {
  /** Largest zstd window or xz dictionary a file may declare, in bytes. Default 128 MiB. */
  memoryLimit?: number;
}

export class HtpDecompressionStream {
  constructor(format: DecompressionFormat, options?: HtpDecompressionOptions);
  readonly readable: ReadableStream<Uint8Array<ArrayBuffer>>;
  readonly writable: WritableStream<BufferSource>;
}

export type HtpDecompressionErrorCode =
  | 'truncated'
  | 'corrupt'
  | 'unsupported'
  | 'memory-limit'
  | 'trailing-data'
  | 'internal';

export class HtpDecompressionError extends TypeError {
  readonly code: HtpDecompressionErrorCode;
}

/** The format a file's first bytes carry, or null. */
export function detectFormat(head: Uint8Array): DecompressionFormat | null;

/** How many leading bytes detectFormat needs. */
export const DETECT_FORMAT_BYTES: number;
```

Behaviour, matching `DecompressionStream` except where noted:

- The constructor is synchronous. An unknown format throws a `TypeError`; an invalid
  `memoryLimit` throws a `RangeError`.
- Writing anything other than a `BufferSource` errors the stream with a `TypeError`. Written bytes
  are copied, so callers may reuse their buffers. A write settles once the decoder has consumed
  its bytes, so backpressure reaches the source.
- The readable side is pull-driven with `highWaterMark: 0`: each `pull()` runs decode steps until
  it has one output chunk (64 KiB unless the builder finds a better size) or needs more input.
  Nothing is decoded ahead of the reader.
- Clean end: after the writable side closes and the decoder reports `Done`, the readable side
  closes.
- Errors (different from `DecompressionStream`, which drops queued output): the readable side
  errors only from inside a `pull()` with nothing left to deliver, so the reader receives every
  decoded byte first. The error is an `HtpDecompressionError` with its `code`. `writer.close()`
  settles once all input is decoded and rejects with the same error.
- `readable.cancel(reason)` stops decoding, errors the writable side with `reason` (so an upstream
  pipe stops) and drops the instance. `writable.abort(reason)` errors the readable side with
  `reason` and drops the instance. `pipeThrough(stream, { signal })` works through these.
- Time slicing: inside a `pull()`, yield to the event loop (a macrotask) whenever about 8 ms of
  decoding has passed since the last yield, so clicks and aborts get through.
- Instances: the first write or pull loads the format's module with a dynamic
  `import('./wasm/<format>.js')`, compiles it once per page (a cached `Promise<WebAssembly.Module>`)
  and instantiates it for this stream. The instance, and with it its memory, is dropped on close,
  error, cancel and abort.
- Loading: each `wasm/<format>.js` is generated at build time and exports the module bytes as
  base64. Bundlers split the dynamic imports into lazy chunks on their own; no loader or asset
  configuration is needed, and Vitest on Node 24 runs it as is.
- `detectFormat` uses the rules web-ui uses today: gzip `1f 8b 08` (method byte checked, as hashcat
  does); xz `fd 37 7a 58 5a 00`; zstd `28 b5 2f fd`, or a skippable frame `5? 2a 4d 18` first. Too
  few bytes means `null`.

Package shape: `"type": "module"`, an `exports` map, `"sideEffects": false`, ESM JavaScript plus
`.d.ts` from `tsc`, no runtime dependencies.

### Distribution

- The repository lives in the owner's personal GitHub account for now,
  `github.com/four-random-common-words/htp-decompress`, not in the hashtopolis organization. It may
  move there later; GitHub redirects the old URL, but the npm trusted-publisher setting and the
  `repository` field in `package.json` then have to be updated by hand.
- Published to the public npm registry as `htp-decompress` (unscoped; the name was free on
  2026-11-25). web-ui depends on it like any other package, so its Docker build and CI need nothing
  extra.
- The release workflow stages a version on `v*` tags with npm provenance (`npm stage publish`);
  it goes live only when the owner approves it with their npm second factor, so nobody else can
  release. The first version is published by hand from the owner's npm account; after that, the
  package is set up for npm trusted publishing from `release.yml` with direct `npm publish` not
  allowed, so CI needs no long-lived npm token.
- Local development before a release: web-ui points at the built package by path
  (`"htp-decompress": "file:../htp-decompress/npm"`). This works for `ng serve` and Vitest on a
  developer machine, not in web-ui's Docker build or CI, and is never committed in web-ui.
- Not a git dependency: npm cannot install a package from a subdirectory of a git repository, and
  it would run the build (needing Rust) or need the built files committed.

### Testing

Rust (`cargo test`):

- Fixtures made with the reference tools and committed with their generator script and a README:
  gzip (single member, several members, empty, with FNAME/FEXTRA/FHCRC); xz (`-6`, `-9`,
  `--check=none|crc32|crc64|sha256`, `--x86`, `--delta`, `--lzma2=dict=65MiB`, multi-block `-T`,
  two concatenated streams with padding, empty); zstd (`-3`, `-19`, `--long=27`, `--ultra -22`,
  with and without checksum, pzstd skippable frames, several frames, a dictionary frame, empty).
- For each: the whole file decodes to the original, ending in `Done`; cuts at about 10, 25, 50
  and 90 % give a prefix of the original and then `Truncated`, at least as long as what
  `gzip -dc`/`zstd -dc` give from the same cut (xz: within 64 KiB of `xz -dc`); a flipped byte
  gives a prefix of the original and then `Corrupt`; random input and output chunk sizes give the
  same result.
- Bounded work: with 16 KiB input slices of a file of `a\n` repeated millions of times, no call
  produces more than 128 KiB.
- A cargo-fuzz target on `Decoder::process` with arbitrary chunking: no panics, and every call
  either makes progress or asks for input.

TypeScript (Vitest on Node 24, against the built wasm):

- Stream semantics: `pipeThrough`, clean end, truncated, damaged, trailing data, unknown format,
  non-`BufferSource` writes, `memoryLimit` exceeded before any output.
- No lost output: on a cut-off or damage, the bytes read before the error equal what the Rust core
  produced.
- Backpressure: a reader that stops after N bytes leaves at most one step of extra decoding.
- Memory: reading 16 MiB from a 1 MiB prefix and from the whole of a large-window file
  (`zstd --long=27`, `xz -9`) grows the stream's wasm memory by no more than about twice the bytes
  decoded (amortized buffer growth), not by the declared window or dictionary. Measured on
  2026-11-26, xz reaches about four times: lzma-rust2 doubles its dictionary with `realloc`, and
  dlmalloc, Rust's wasm32 allocator, grows a block in place only into free space it already has, so
  each doubling moves the dictionary to new memory above the freed blocks, which never fit the next
  doubling (`xz -9`: 65.6 MiB at 16 MiB of output; `xz -6` levels off at 17.5 MiB). Accepted: a
  wrapper around the allocator that made room first halved it, but was not worth its `unsafe` code.
- Cancel and abort: `readable.cancel()` mid-decode, `pipeThrough` with an aborted signal,
  `writable.abort()`; afterwards a new stream works, and two streams decode concurrently.
- The web-ui preview's cases, as far as they apply to the package: whole file, file spanning many
  16 KiB slices, several streams/members/frames, cut-off prefix, `a\n` x 4,194,304 read up to
  exactly 65,536 bytes, empty stream, cancel before start and while running, pzstd skippable frame
  first, detection rules.

### CI and release

- `ci.yml` on pushes and pull requests: rustfmt, clippy, `cargo test --locked`, wasm builds,
  `npm ci` and the Vitest suite, and a report of the three wasm sizes.
- `release.yml` on `v*` tags, like htp-agent-rust: build and test as above, `npm stage publish`
  with provenance (the owner approves the staged version), and create the GitHub release (with
  the `npm pack` tarball attached). The Cargo
  workspace and the npm package share one version, and the tag must match it.

### web-ui integration (the last phase, a web-ui pull request)

Order:

1. PR 906 (gzip through the built-in `DecompressionStream`) merges into web-ui as it is.
2. The package is built here and its first version is published to npm by the owner, by hand.
3. A new web-ui branch off master (with 906 in it) switches the preview to the package.

The branch `2644-enhancement-preview-xz-and-zstd-files` was an experiment with fzstd and
xz-decompress and will not be merged, so those libraries never reach master. Reuse from it: the
spec cases in `file-preview-formats.spec.ts`, the xz fixtures with their README and generator, the
detection rules, and the findings listed under "Why".

The new branch:

- Adds `htp-decompress` as a dependency.
- Decodes all three formats, gzip included (which fixes Chrome's first-member limit), through
  `input stream .pipeThrough(new HtpDecompressionStream(format, { memoryLimit }))`; `reachedEnd` is
  "the readable side closed cleanly and the input was the whole file".
- Uses `detectFormat` for detection, replacing the gzip-only check from 906.
- Brings over the experiment's spec cases, adapted to the package.
- The input no longer needs slicing to bound overshoot, since decode steps are bounded; slicing can
  stay if it keeps the code simpler.
- Because memory follows the decoded output (lzma-rust2 as is, ruzstd with the patch) and the
  preview stops at 16 MiB, the preview can pass a higher `memoryLimit` than the default, so files
  with large declared windows or dictionaries still preview.

## Measured during research (for reference)

wasm sizes, `opt-level = "s"`, LTO, no wasm-opt, no glue:

| Module | Raw | gzip -9 |
|---|---|---|
| ruzstd decoder | 56 KB | 22 KB |
| lzma-rust2 xz decoder | 79 KB | 35 KB |
| miniz_oxide inflate | 31 KB | 13 KB |

`opt-level = 3` adds about 5 KB gzipped per module and makes zstd about 1.5x faster. Decode speed
in Deno (V8): zstd 135 to 293 MiB/s, xz 35 to 107 MiB/s (about 1.3x the time of native `xz`).

## Left to the builder

- The exact wasm ABI, how 64-bit values cross it, and the result area layout.
- Base64 decoding, the yield mechanism and its time budget, the output chunk size.
- `opt-level` per module and whether wasm-opt is worth adding.
- The shape of the ruzstd patch (within the required behaviour above) and the upstream pull request.
- Exact error messages, the mapping of lzma-rust2's error kinds, fixture generation details.

## Calls made without the owner (please check)

1. Names: package `htp-decompress`, class `HtpDecompressionStream`, error `HtpDecompressionError`
   with a `code`.
2. Errors stay `TypeError`s (a subclass), as in `DecompressionStream`.
3. Trailing data after the last member/stream/frame is an error (`trailing-data`), as in
   `DecompressionStream`. hashcat (through zlib) ignores trailing garbage after gzip data; a consumer
   can treat this code as a clean end if it wants that.
4. `memoryLimit` defaults to 128 MiB (zstd's own default limit; covers `xz -9`) and counts what the
   file declares, not what is allocated.
5. One wasm module per format, loaded on first use, inlined as base64.
6. `detectFormat` lives in the package, so format knowledge stays in one place.
7. No Worker, no deflate or deflate-raw, no crates.io publishing for now.
8. Distribution: public npm registry (owner confirmed), first publish by hand, then trusted
   publishing with provenance from CI as staged versions the owner approves with 2FA (owner
   confirmed); a `file:` path only for local development.
9. Licensed GPL-3.0. Hosting is the owner's decision: a personal repository for now (see
   Distribution).
10. ruzstd must hand out output right after each block and must not reserve the window up front;
    both go into the vendored patch alongside the tested drain at a cut-off.
11. The web-ui pull request is the last phase of this plan, starting after the owner's first npm
    publish. It switches gzip to the package too, not only xz and zstd.
