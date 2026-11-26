# htp-decompress

gzip, xz and zstd decompression streams for the browser and Node.js, compiled from Rust to WebAssembly. It works like
`DecompressionStream`, with three differences that matter when a file is damaged or cut off: you get everything that
could be decoded before the error, the error says why, and gzip files with several members decode completely.

ESM only, no runtime dependencies, TypeScript declarations included.

## Install

    npm install htp-decompress

## Usage

```ts
import { HtpDecompressionStream } from 'htp-decompress';

const response = await fetch('/files/wordlist.txt.xz');
const text = response.body!.pipeThrough(new HtpDecompressionStream('xz'));

for await (const chunk of text) {
  // chunk is a Uint8Array of at most 64 KiB
}
```

The format is passed in; it is not guessed. To find it out from a file's first bytes:

```ts
import { DETECT_FORMAT_BYTES, detectFormat } from 'htp-decompress';

const head = new Uint8Array(await file.slice(0, DETECT_FORMAT_BYTES).arrayBuffer());
const format = detectFormat(head); // 'gzip' | 'xz' | 'zstd' | null
```

`detectFormat` checks the gzip magic and method byte (`1f 8b 08`), the xz magic (`fd 37 7a 58 5a 00`) and the zstd
magic (`28 b5 2f fd`, or a skippable frame `5? 2a 4d 18` first). It returns `null` for anything else, and when it gets
too few bytes.

## API

### `new HtpDecompressionStream(format, options?)`

`format` is `'gzip' | 'xz' | 'zstd'`. The stream has a `writable` side taking any `BufferSource` and a `readable` side
of `Uint8Array` chunks. The constructor throws a `TypeError` for an unknown format and a `RangeError` for an invalid
`memoryLimit`.

### Options

| Option        | Default                | Meaning                                                                  |
| ------------- | ---------------------- | ------------------------------------------------------------------------ |
| `memoryLimit` | `134217728` (128 MiB)  | Largest zstd window or xz dictionary a file may declare, in bytes.       |

The limit counts what the file declares, not what is allocated: memory follows the decoded output. `DEFAULT_MEMORY_LIMIT`
exports the default. A limit of 4 GiB or more acts as 4,294,967,295 bytes, the most a WebAssembly decoder can address.

### Errors

Decoding errors are `HtpDecompressionError`, a subclass of `TypeError`, with a `code`:

| Code              | Meaning                                                                           |
| ----------------- | --------------------------------------------------------------------------------- |
| `truncated`       | The input ends inside a stream, member or frame.                                  |
| `corrupt`         | The data is damaged.                                                              |
| `unsupported`     | A feature the decoder does not handle, such as a zstd dictionary.                 |
| `memory-limit`    | The file declares a window or dictionary above `memoryLimit`.                     |
| `trailing-data`   | Bytes follow the last complete stream that do not start another one.              |
| `internal`        | A bug in the decoder; please report it.                                           |

```ts
import { HtpDecompressionError } from 'htp-decompress';

try {
  // read the stream
} catch (error) {
  if (error instanceof HtpDecompressionError && error.code === 'truncated') {
    // everything before the cut-off has been read already
  }
}
```

A consumer that wants to ignore trailing garbage, as hashcat does after gzip data, can treat `trailing-data` as a clean
end.

## Differences from `DecompressionStream`

- No lost output: when the input is cut off or damaged, the reader receives every byte decoded before that point, and
  only then the error. `DecompressionStream` drops what it had queued. For xz this holds up to one decode pass, see
  [Limits](#limits).
- Errors have a `code`, so a caller can tell a cut-off file from a damaged one or one over the memory limit.
- gzip files with several members decode completely. Chrome's `DecompressionStream` stops after the first member.
- Decoding is pull-driven: nothing is decoded ahead of the reader, so a reader that stops stops the work. Long decodes
  yield to the event loop about every 8 ms.
- An abort through a `pipeThrough` signal takes effect after the chunk being written has been decoded and read, because
  a write settles only when its bytes are consumed.
- Also supports xz and zstd, which `DecompressionStream` does not.

## Limits

- Not supported: zstd dictionaries and legacy (pre-1.0) zstd frames (`unsupported`).
- xz accepts at most one BCJ or delta filter in front of LZMA2 per block.
- xz output can fall short of what `xz -dc` gives by up to 64 KiB at a cut-off in highly repetitive data, and before
  damage: the xz decoder underneath decodes in passes of up to 64 KiB and loses the pass that runs into the cut or the
  damage.
- Each format's WebAssembly module is bundled as base64 and loaded on first use, so bundlers split them into lazy chunks
  without extra configuration.

## License

GPL-3.0-only. The WebAssembly modules also contain third-party Rust crates under MIT, Apache-2.0 and other permissive
licenses; their notices are in `THIRD-PARTY-NOTICES.md`.
