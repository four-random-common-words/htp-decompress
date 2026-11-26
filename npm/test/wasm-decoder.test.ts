import { describe, expect, it, vi } from 'vitest';

import { HtpDecompressionError, type HtpDecompressionErrorCode } from '../src/errors.js';
import { type DecompressionFormat, FORMATS } from '../src/format.js';
import { WasmDecoder } from '../src/wasm-decoder.js';
import { decodeAll, decodeSteps, fixtures, readFixture } from './fixtures.js';

const DEFAULT_MEMORY_LIMIT = 134_217_728;

// The detail of the Rust error comes through: its text for corrupt and unsupported, the bytes needed for memory-limit.
const MESSAGES: Record<HtpDecompressionErrorCode, RegExp> = {
  truncated: /^input ends before the end of the stream$/,
  corrupt: /^corrupt data: \w/,
  unsupported: /^unsupported feature: \w/,
  'memory-limit': /^decoding needs [1-9]\d* bytes, which exceeds the memory limit of \d+ bytes$/,
  'trailing-data': /^unexpected data after the end of the stream$/,
  internal: /^the decoder failed: /
};

describe.each(FORMATS)('%s WasmDecoder', (format) => {
  it('decodes every done fixture', async () => {
    for (const fixture of fixtures(format)) {
      if (fixture.expect !== 'done') {
        continue;
      }

      const decoder = await WasmDecoder.create(format, fixture.limit ?? DEFAULT_MEMORY_LIMIT);

      const output = decodeAll(decoder, readFixture(fixture.file));

      expect(sameBytes(output, fixture.original), fixture.file).toBe(true);
    }
  });

  it('maps error codes', async () => {
    for (const fixture of fixtures(format)) {
      if (fixture.expect === 'done') {
        continue;
      }

      const decoder = await WasmDecoder.create(format, fixture.limit ?? DEFAULT_MEMORY_LIMIT);

      const error = thrown(() => decodeAll(decoder, readFixture(fixture.file)));

      expect(error, fixture.file).toBeInstanceOf(HtpDecompressionError);
      expect(error.code, fixture.file).toBe(fixture.expect);
      expect(error.message, fixture.file).toMatch(MESSAGES[fixture.expect]);
    }
  });

  it('passes at most one input slice per call', async () => {
    const decoder = await WasmDecoder.create(format, DEFAULT_MEMORY_LIMIT);

    const { consumed } = decoder.process(skippedInput(format), false);

    expect(consumed).toBeGreaterThan(0);
    expect(consumed).toBeLessThanOrEqual(65_536);
  });
});

it('keeps zstd memory near the decoded size', async () => {
  const decoder = await WasmDecoder.create('zstd', 134_217_728);
  const before = decoder.memoryBytes();

  const output = decodeAll(decoder, readFixture('zstd/aaaa-long27.zst'));

  expect(output.length).toBe(16_777_216);
  expect(decoder.memoryBytes() - before).toBeLessThan(41_943_040);
});

it('keeps xz memory near the decoded size', async () => {
  const decoder = await WasmDecoder.create('xz', 134_217_728);
  const before = decoder.memoryBytes();

  const output = decodeAll(decoder, readFixture('xz/aaaa-dict65.xz'));

  expect(output.length).toBe(16_777_216);
  expect(decoder.memoryBytes() - before).toBeLessThan(41_943_040);
});

it('gives every decoder its own memory', async () => {
  const zstd = fixtures('zstd');
  const hex = zstd.find((fixture) => fixture.file === 'zstd/hex-3.zst')!;
  const frames = zstd.find((fixture) => fixture.file === 'zstd/frames.zst')!;
  const instantiate = vi.spyOn(WebAssembly, 'instantiate');
  const first = await WasmDecoder.create('zstd', DEFAULT_MEMORY_LIMIT);
  const second = await WasmDecoder.create('zstd', DEFAULT_MEMORY_LIMIT);

  const firstSteps = decodeSteps(first, readFixture(hex.file));
  const secondSteps = decodeSteps(second, readFixture(frames.file));
  let firstStep = firstSteps.next();
  let secondStep = secondSteps.next();
  while (!firstStep.done || !secondStep.done) {
    if (!firstStep.done) {
      firstStep = firstSteps.next();
    }

    if (!secondStep.done) {
      secondStep = secondSteps.next();
    }
  }

  const [firstInstance, secondInstance] = await Promise.all(
    instantiate.mock.results.map((result) => result.value as Promise<WebAssembly.Instance>)
  );
  expect(sameBytes(firstStep.value, hex.original)).toBe(true);
  expect(sameBytes(secondStep.value, frames.original)).toBe(true);
  expect(firstInstance.exports.memory).toBeInstanceOf(WebAssembly.Memory);
  expect(firstInstance.exports.memory).not.toBe(secondInstance.exports.memory);
});

it('turns a trap into an internal error and drops the instance', async () => {
  const instantiate = WebAssembly.instantiate;
  vi.spyOn(WebAssembly, 'instantiate').mockImplementationOnce(async (module: WebAssembly.Module) => {
    const instance = await instantiate(module);
    const trap = () => {
      throw new WebAssembly.RuntimeError('unreachable');
    };

    return { exports: { ...instance.exports, htp_decoder_process: trap } };
  });
  const decoder = await WasmDecoder.create('gzip', DEFAULT_MEMORY_LIMIT);

  const error = thrown(() => decoder.process(readFixture('gzip/hex.gz'), true));
  const again = thrown(() => decoder.process(readFixture('gzip/hex.gz'), true));

  expect(error).toBeInstanceOf(HtpDecompressionError);
  expect(error.code).toBe('internal');
  expect(error.message).toMatch(MESSAGES.internal);
  expect(error.cause).toBeInstanceOf(WebAssembly.RuntimeError);
  expect(again).toBe(error);
  expect(decoder.memoryBytes()).toBe(0);
});

/** 1 MiB that produces no output, so a decoder given all of it in one call would consume all of it. */
function skippedInput(format: DecompressionFormat): Uint8Array {
  const input = new Uint8Array(1_048_576);
  switch (format) {
    case 'gzip':
      // A member header whose comment runs to the end.
      input.set([0x1f, 0x8b, 0x08, 0x10, 0, 0, 0, 0, 0, 0xff]);
      input.fill(0x61, 10);
      break;
    case 'xz':
      // An empty stream, then stream padding.
      input.set(readFixture('xz/empty.xz'));
      break;
    case 'zstd':
      // A skippable frame as long as the input.
      new DataView(input.buffer).setUint32(0, 0x184d2a50, true);
      new DataView(input.buffer).setUint32(4, input.length - 8, true);
      break;
  }

  return input;
}

function sameBytes(a: Uint8Array, b: Uint8Array): boolean {
  return Buffer.from(a.buffer, a.byteOffset, a.byteLength).equals(b);
}

function thrown(action: () => unknown): HtpDecompressionError {
  try {
    action();
  } catch (error) {
    return error as HtpDecompressionError;
  }

  throw new Error('nothing was thrown');
}
