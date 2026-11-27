# Fixtures

Test inputs for the gzip, xz and zstd decoders. Everything except `generate.py` and
`originals/` is generated; `manifest.json` describes each compressed file (original content,
expected outcome, optional memory limit, and truncation cuts with the reference tool's output length).

Tools used:

- gzip: Apple gzip 487.40.2
- xz: xz (XZ Utils) 5.8.3
- zstd: zstd v1.5.7 (pzstd from the same install)

Regenerate with `gzip`, `xz`, `zstd` and `pzstd` on `PATH`:

    mise run fixtures

Notes:

- The repeat original (`a\n` 8388608 times, 16 MiB) is never stored; the manifest refers to it as `repeat:a\n:8388608`.
- Files marked "from stdin" in the plan are piped so the tools do not know the input size. xz rounds
  `dict=65MiB` up to the next representable size (96 MiB), which still exceeds the 64 MiB test limit.
