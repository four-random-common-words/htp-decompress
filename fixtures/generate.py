"""Generates the test fixtures and manifest.json.

Run from the repository root: python3 -I fixtures/generate.py
Needs gzip, xz, zstd and pzstd on PATH.
"""

import hashlib
import json
import struct
import subprocess
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parent
REPEAT = "repeat:a\n:8388608"
MEMORY_LIMIT = 67_108_864
TOOLS = {"gzip": ["gzip", "-dc"], "xz": ["xz", "-dc"], "zstd": ["zstd", "-dc"]}

manifest = []


def run(command, data=None):
    return subprocess.run(command, input=data, stdout=subprocess.PIPE, check=True).stdout


def write(file, data):
    path = ROOT / file
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)


def original(name):
    return (ROOT / "originals" / name).read_bytes()


def repeated():
    return b"a\n" * 8_388_608


def flip(data, index):
    out = bytearray(data)
    out[index] ^= 0xFF
    return bytes(out)


def reference_length(format, prefix):
    result = subprocess.run(TOOLS[format], input=prefix, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    return len(result.stdout)


def cuts_for(format, data):
    if len(data) <= 64:
        return []

    cuts = []
    for percent in (10, 25, 50, 90):
        at = len(data) * percent // 100
        cuts.append({"at": at, "referenceLength": reference_length(format, data[:at])})

    return cuts


def add(file, format, sources, data, expect, limit=None):
    write(file, data)
    cuts = cuts_for(format, data) if expect == "done" else []
    manifest.append(
        {
            "file": file,
            "format": format,
            "original": sources,
            "expect": expect,
            "limit": limit,
            "cuts": cuts,
        }
    )


def make_originals():
    write("originals/numbered.txt", "".join(f"line {n}\n" for n in range(1, 1001)).encode())
    write("originals/hex.txt", "".join(hashlib.md5(str(i).encode()).hexdigest() + "\n" for i in range(8192)).encode())
    write("originals/empty.txt", b"")


def gzip_headers(data):
    header = b"\x1f\x8b\x08" + bytes([0x02 | 0x04 | 0x08 | 0x10]) + struct.pack("<IBB", 0, 0, 3)
    header += struct.pack("<H", 4) + b"\x01\x02\x03\x04"
    header += b"hex.txt\x00"
    header += b"fixture\x00"
    header += struct.pack("<H", zlib.crc32(header) & 0xFFFF)

    compressor = zlib.compressobj(9, zlib.DEFLATED, -15)
    body = compressor.compress(data) + compressor.flush()
    trailer = struct.pack("<II", zlib.crc32(data), len(data) & 0xFFFFFFFF)

    return header + body + trailer


def make_gzip():
    def gz(args, data):
        return run(["gzip", *args, "-n", "-c"], data)

    hex_data = original("hex.txt")
    hex_gz = gz(["-9"], hex_data)

    reserved = bytearray(hex_gz)
    reserved[3] |= 0x20

    add("gzip/hex.gz", "gzip", ["hex.txt"], hex_gz, "done")
    add("gzip/hex-headers.gz", "gzip", ["hex.txt"], gzip_headers(hex_data), "done")
    add("gzip/members.gz", "gzip", ["numbered.txt", "hex.txt"], gz([], original("numbered.txt")) + gz([], hex_data), "done")
    add("gzip/empty.gz", "gzip", ["empty.txt"], gz([], b""), "done")
    add("gzip/aaaa.gz", "gzip", [REPEAT], gz(["-1"], repeated()), "done")
    add("gzip/corrupt.gz", "gzip", ["hex.txt"], flip(hex_gz, len(hex_gz) // 2), "corrupt")
    add("gzip/bad-crc.gz", "gzip", ["hex.txt"], flip(hex_gz, len(hex_gz) - 8), "corrupt")
    add("gzip/reserved-flag.gz", "gzip", ["hex.txt"], bytes(reserved), "corrupt")
    add("gzip/trailing.gz", "gzip", ["hex.txt"], hex_gz + b"garbage!", "trailing-data")
    add("gzip/zero.gz", "gzip", ["empty.txt"], b"", "truncated")


def make_xz():
    def xz(args, data):
        return run(["xz", *args, "-c"], data)

    hex_data = original("hex.txt")
    hex_xz = xz(["-6"], hex_data)
    numbered_xz = xz(["-6"], original("numbered.txt"))
    dict65 = ["--lzma2=preset=6,dict=65MiB"]
    dict65_xz = xz(dict65, hex_data)

    add("xz/hex.xz", "xz", ["hex.txt"], hex_xz, "done")
    add("xz/hex-none.xz", "xz", ["hex.txt"], xz(["-6", "--check=none"], hex_data), "done")
    add("xz/hex-crc32.xz", "xz", ["hex.txt"], xz(["-6", "--check=crc32"], hex_data), "done")
    add("xz/hex-sha256.xz", "xz", ["hex.txt"], xz(["-6", "--check=sha256"], hex_data), "done")
    add("xz/hex-x86.xz", "xz", ["hex.txt"], xz(["--x86", "--lzma2=preset=6"], hex_data), "done")
    add("xz/hex-delta.xz", "xz", ["hex.txt"], xz(["--delta=dist=4", "--lzma2=preset=6"], hex_data), "done")
    add("xz/hex-blocks.xz", "xz", ["hex.txt"], xz(["-6", "-T2", "--block-size=16KiB"], hex_data), "done")
    add("xz/concat.xz", "xz", ["numbered.txt", "hex.txt"], numbered_xz + b"\x00" * 4 + hex_xz, "done")
    add("xz/empty.xz", "xz", ["empty.txt"], xz(["-6"], b""), "done")
    add("xz/hex-dict65.xz", "xz", ["hex.txt"], dict65_xz, "done")
    add("xz/hex-dict65.xz", "xz", ["hex.txt"], dict65_xz, "memory-limit", MEMORY_LIMIT)
    add("xz/aaaa-dict65.xz", "xz", [REPEAT], xz(dict65, repeated()), "done")
    add("xz/hex-chain.xz", "xz", ["hex.txt"], xz(["--x86", "--delta=dist=4", "--lzma2=preset=6"], hex_data), "unsupported")
    add("xz/corrupt.xz", "xz", ["hex.txt"], flip(hex_xz, len(hex_xz) // 2), "corrupt")
    add("xz/bad-padding.xz", "xz", ["hex.txt"], hex_xz + b"\x00\x00", "corrupt")
    add("xz/trailing.xz", "xz", ["hex.txt"], hex_xz + b"garbage!", "trailing-data")
    add("xz/zero.xz", "xz", ["empty.txt"], b"", "truncated")


def make_zstd():
    def zstd(args, data):
        return run(["zstd", *args, "-c", "-q"], data)

    hex_data = original("hex.txt")
    hex3 = zstd(["-3"], hex_data)
    dict_frame = bytes.fromhex("28b52ffd" "21" "07" "00" "010000")
    legacy = bytes.fromhex("27b52ffd") + b"\x00" * 16

    add("zstd/hex-3.zst", "zstd", ["hex.txt"], hex3, "done")
    add("zstd/hex-19.zst", "zstd", ["hex.txt"], zstd(["-19"], hex_data), "done")
    add("zstd/hex-nocheck.zst", "zstd", ["hex.txt"], zstd(["-3", "--no-check"], hex_data), "done")
    add("zstd/hex-pzstd.zst", "zstd", ["hex.txt"], run(["pzstd", "-p", "2", "-3", "-c", "-q"], hex_data), "done")
    add("zstd/frames.zst", "zstd", ["numbered.txt", "hex.txt"], zstd(["-3"], original("numbered.txt")) + hex3, "done")
    add("zstd/empty.zst", "zstd", ["empty.txt"], zstd(["-3"], b""), "done")
    add("zstd/aaaa-long27.zst", "zstd", [REPEAT], zstd(["-3", "--long=27"], repeated()), "done")
    add("zstd/aaaa-long28.zst", "zstd", [REPEAT], zstd(["-3", "--long=28"], repeated()), "memory-limit")
    add("zstd/dict.zst", "zstd", ["empty.txt"], dict_frame, "unsupported")
    add("zstd/legacy.zst", "zstd", ["empty.txt"], legacy, "unsupported")
    add("zstd/corrupt.zst", "zstd", ["hex.txt"], flip(hex3, len(hex3) // 2), "corrupt")
    add("zstd/bad-checksum.zst", "zstd", ["hex.txt"], flip(hex3, len(hex3) - 1), "corrupt")
    add("zstd/trailing.zst", "zstd", ["hex.txt"], hex3 + b"garbage!", "trailing-data")
    add("zstd/zero.zst", "zstd", ["empty.txt"], b"", "truncated")


make_originals()
make_gzip()
make_xz()
make_zstd()

(ROOT / "manifest.json").write_text(json.dumps({"fixtures": manifest}, indent=2) + "\n")
