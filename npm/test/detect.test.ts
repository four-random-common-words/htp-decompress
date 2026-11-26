import { describe, expect, it } from 'vitest';

import { DETECT_FORMAT_BYTES, detectFormat } from '../src/detect.js';

describe('detectFormat', () => {
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

  it('needs at most DETECT_FORMAT_BYTES bytes and ignores what follows', () => {
    const xz = new Uint8Array([0xfd, 0x37, 0x7a, 0x58, 0x5a, 0x00, 0x01, 0x02]);

    expect(DETECT_FORMAT_BYTES).toBe(6);
    expect(detectFormat(xz.subarray(0, DETECT_FORMAT_BYTES))).toBe('xz');
    expect(detectFormat(xz)).toBe('xz');
  });
});
