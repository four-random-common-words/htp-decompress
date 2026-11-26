import type { DecompressionFormat } from './format.js';

/** How many leading bytes `detectFormat` needs. */
export const DETECT_FORMAT_BYTES = 6;

/** The format a file's first bytes carry, or null when they match none or there are too few of them. */
export function detectFormat(head: Uint8Array): DecompressionFormat | null {
  if (head.length >= 3 && head[0] === 0x1f && head[1] === 0x8b && head[2] === 0x08) {
    return 'gzip';
  }

  if (
    head.length >= 6 &&
    head[0] === 0xfd &&
    head[1] === 0x37 &&
    head[2] === 0x7a &&
    head[3] === 0x58 &&
    head[4] === 0x5a &&
    head[5] === 0x00
  ) {
    return 'xz';
  }

  if (head.length >= 4) {
    const isZstdFrame = head[0] === 0x28 && head[1] === 0xb5 && head[2] === 0x2f && head[3] === 0xfd;
    const isSkippableFrame = (head[0] & 0xf0) === 0x50 && head[1] === 0x2a && head[2] === 0x4d && head[3] === 0x18;

    if (isZstdFrame || isSkippableFrame) {
      return 'zstd';
    }
  }

  return null;
}
