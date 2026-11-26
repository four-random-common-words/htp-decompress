#!/bin/sh
# Writes every fixture listed in fixtures/manifest.json, prefixed with its format byte
# (gzip 0, xz 1, zstd 2) and a chunk seed byte, into the git-ignored corpus directory.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
fixtures="$here/../../../fixtures"
corpus="$here/corpus/process"

mkdir -p "$corpus"

python3 -I - "$fixtures" "$corpus" <<'PY'
import json
import os
import sys

fixtures, corpus = sys.argv[1:3]
prefixes = {"gzip": 0, "xz": 1, "zstd": 2}

with open(os.path.join(fixtures, "manifest.json")) as manifest:
    entries = json.load(manifest)["fixtures"]

for entry in entries:
    with open(os.path.join(fixtures, entry["file"]), "rb") as source:
        data = source.read()

    name = entry["file"].replace("/", "-")

    with open(os.path.join(corpus, name), "wb") as target:
        target.write(bytes([prefixes[entry["format"]], 0]) + data)

print(f"wrote {len(entries)} seeds to {corpus}")
PY
