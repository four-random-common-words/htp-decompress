# htp-decompress

gzip, xz and zstd decoders written in Rust, compiled to one WebAssembly module per format and wrapped as the npm package
[`htp-decompress`](npm/README.md), a `DecompressionStream` look-alike that never loses decoded output on an error.

## Layout

| Path                      | Contents                                                                         |
| ------------------------- | -------------------------------------------------------------------------------- |
| `crates/htp-decompress`   | The decoder core: gzip (miniz_oxide), xz (lzma-rust2) and zstd (ruzstd).         |
| `crates/htp-decompress-wasm` | The wasm exports, one build per format feature.                               |
| `vendor/ruzstd`           | The patched ruzstd the zstd decoder uses.                                        |
| `npm`                     | The npm package: the stream, format detection, tests. See `npm/README.md`.       |
| `fixtures`                | Test files for all three formats, their originals, manifest and generator.       |
| `scripts`                 | `build-wasm.sh`, `embed-wasm.mjs` and `write-license.mjs`.                       |
| `docs/superpowers`        | Design spec and implementation plans.                                            |

## Build and test

Node and Rust are pinned by `mise.toml` and `rust-toolchain.toml`. The build needs no C compiler.

    # Rust
    cargo fmt --check
    cargo clippy --all-targets -- -D warnings
    cargo test --locked

    # npm package (builds the three wasm modules first and prints their sizes)
    cd npm
    npm ci
    npm run build
    npm test
    npm run typecheck

`npm run build` writes `npm/src/wasm/<format>.ts` (git-ignored), compiles to `npm/dist` and writes `LICENSE` and
`THIRD-PARTY-NOTICES.md` into `npm/` (also git-ignored). The package's `LICENSE` is the GPL text followed by the
third-party notices, because license extractors such as Angular's read only that file. `npm pack` then produces the
package.

## Regenerating the fixtures

With `gzip`, `xz`, `zstd` and `pzstd` on `PATH`, from the repository root:

    python3 -I fixtures/generate.py

The tool versions used are listed in `fixtures/README.md`.

## License

GPL-3.0-only, see `LICENSE`. The WebAssembly modules also contain third-party crates under MIT, Apache-2.0 and other
permissive licenses, listed with their notices in `THIRD-PARTY-NOTICES.md`.
