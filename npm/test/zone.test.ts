// zone.js replaces the global Promise, as in an Angular app, for this test file only: Vitest runs each file on its own.
import 'zone.js';

import { afterEach, beforeEach, expect, it } from 'vitest';

import { HtpDecompressionError } from '../src/errors.js';
import type { DecompressionFormat } from '../src/format.js';
import { HtpDecompressionStream } from '../src/stream.js';
import { readFixture } from './fixtures.js';

/** zone.js settings and hooks, which it reads from `Zone` under its symbol names. */
const zoneSettings = Zone as unknown as Record<string, unknown>;
const UNHANDLED = Zone.__symbol__('unhandledPromiseRejectionHandler');
const QUIET = Zone.__symbol__('ignoreConsoleErrorUncaughtError');

const cases: { file: string; cut?: number; code: string }[] = [
  { file: 'gzip/hex.gz', cut: 39_252, code: 'truncated' },
  { file: 'xz/hex.xz', cut: 35_758, code: 'truncated' },
  { file: 'zstd/hex-3.zst', cut: 34_831, code: 'truncated' },
  { file: 'gzip/zero.gz', code: 'truncated' },
  { file: 'gzip/corrupt.gz', code: 'corrupt' },
  { file: 'xz/corrupt.xz', code: 'corrupt' },
  { file: 'zstd/corrupt.zst', code: 'corrupt' },
  { file: 'xz/hex-chain.xz', code: 'unsupported' },
  { file: 'zstd/dict.zst', code: 'unsupported' },
  { file: 'gzip/trailing.gz', code: 'trailing-data' }
];

let unhandled: unknown[] = [];

beforeEach(() => {
  unhandled = [];
  zoneSettings[UNHANDLED] = (error: unknown) => unhandled.push(error);
  zoneSettings[QUIET] = true;
});

afterEach(() => {
  delete zoneSettings[UNHANDLED];
  delete zoneSettings[QUIET];
});

it.each(cases)('reports no unhandled rejection under zone.js for $file', async ({ file, cut, code }) => {
  const format = file.slice(0, file.indexOf('/')) as DecompressionFormat;
  const input = new Uint8Array(readFixture(file).subarray(0, cut));

  // As the Hashtopolis file preview does it: write, close, then read until the end or an error.
  const stream = new HtpDecompressionStream(format);
  const writer = stream.writable.getWriter();
  const write = writer.write(input).catch((reason: unknown) => reason);
  const close = writer.close().catch((reason: unknown) => reason);
  const reader = stream.readable.getReader();

  let error: unknown;
  try {
    while (!(await reader.read()).done) {
      // Only the error matters here.
    }
  } catch (reason) {
    error = reason;
  }

  // zone.js reports a rejection that has no handler at the end of the microtask drain it happens in.
  await new Promise((resolve) => setTimeout(resolve, 0));

  expect(error).toBeInstanceOf(HtpDecompressionError);
  expect(error).toHaveProperty('code', code);
  expect(await close).toBe(error);
  expect([undefined, error]).toContain(await write);
  expect(unhandled).toEqual([]);
});

it('reports no unhandled rejection under zone.js for a bad write', async () => {
  const stream = new HtpDecompressionStream('gzip');
  const writer = stream.writable.getWriter();
  const reader = stream.readable.getReader();
  const read = reader.read().catch((reason: unknown) => reason);

  const write = writer.write('text' as never).catch((reason: unknown) => reason);
  await new Promise((resolve) => setTimeout(resolve, 0));

  expect(await write).toBeInstanceOf(TypeError);
  expect(await read).toBe(await write);
  expect(unhandled).toEqual([]);
});

it('sees zone.js report a rejection that gets its handler too late', async () => {
  const late = Promise.reject(new Error('late'));

  await new Promise((resolve) => setTimeout(resolve, 0));
  late.catch(() => {});

  expect(Promise).not.toBe(globalThis[Zone.__symbol__('Promise') as keyof typeof globalThis]);
  expect(unhandled).toHaveLength(1);
});
