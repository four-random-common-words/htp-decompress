# htp-decompress

gzip, xz and zstd decoders written in Rust, compiled to one WebAssembly module per format and wrapped as the npm package
[`htp-decompress`](npm/README.md), a `DecompressionStream` look-alike that never loses decoded output on an error.

## Layout

| Path                      | Contents                                                                         |
| ------------------------- | -------------------------------------------------------------------------------- |
| `crates/htp-decompress`   | The decoder core: gzip (miniz_oxide), xz (lzma-rust2) and zstd (ruzstd).         |
| `crates/htp-decompress-wasm` | The wasm exports, one build per format feature.                               |
| `npm`                     | The npm package: the stream, format detection, tests. See `npm/README.md`.       |
| `fixtures`                | Test files for all three formats, their originals, manifest and generator.       |
| `scripts`                 | `build-wasm.sh`, `embed-wasm.mjs` and `write-license.mjs`.                       |
| `docs/superpowers`        | Design spec and implementation plans.                                            |

## Build and test

Node and Rust are pinned by `mise.toml` and `rust-toolchain.toml`. The build needs no C compiler. Every command is a
[mise](https://mise.jdx.dev) task, and CI runs the same tasks; `mise tasks` lists them all.

    mise run check       # everything CI runs
    mise run lint        # cargo fmt --check, clippy natively and on the wasm crate once per format
    mise run test        # Rust and npm tests
    mise run build       # npm ci, the three wasm modules (prints their sizes) and the npm package
    mise run typecheck
    mise run fmt         # formats the Rust code
    mise run cargo:clean # deletes the cargo build output, also the fuzz crate's (not its corpus)

`mise run build` writes `npm/src/wasm/<format>.ts` (git-ignored), compiles to `npm/dist` and writes `LICENSE` and
`THIRD-PARTY-NOTICES.md` into `npm/` (also git-ignored). The package's `LICENSE` is the GPL text followed by the
third-party notices, because license extractors such as Angular's read only that file. `npm pack` in `npm/` then
produces the package. The npm tests and the typecheck build first, and mise skips the build when none of its inputs
changed; `mise run --force build` reinstalls the npm dependencies and rebuilds anyway.

## Fuzzing

With `cargo-fuzz` and a nightly toolchain installed, this seeds the corpus from the fixtures and fuzzes all three
decoders; arguments after the task name go to libFuzzer:

    mise run fuzz -max_total_time=300

## Regenerating the fixtures

With `gzip`, `xz`, `zstd` and `pzstd` on `PATH`:

    mise run fixtures

The tool versions used are listed in `fixtures/README.md`.

## License

GPL-3.0-only, see `LICENSE`. The WebAssembly modules also contain third-party crates under MIT, Apache-2.0 and other
permissive licenses, listed with their notices in `THIRD-PARTY-NOTICES.md`.
