import { readFileSync } from 'node:fs';

import type { HtpDecompressionErrorCode } from '../src/errors.js';
import type { DecompressionFormat } from '../src/format.js';
import type { WasmDecoder } from '../src/wasm-decoder.js';

const ROOT = new URL('../../fixtures/', import.meta.url);

const EXPECTATIONS = ['done', 'truncated', 'corrupt', 'unsupported', 'memory-limit', 'trailing-data'] as const;

export interface Fixture {
  file: string;
  format: DecompressionFormat;
  original: Uint8Array;
  expect: 'done' | HtpDecompressionErrorCode;
  limit: number | null;
  cuts: { at: number; referenceLength: number }[];
}

interface ManifestEntry {
  file: string;
  format: string;
  original: string[];
  expect: string;
  limit: number | null;
  cuts: { at: number; referenceLength: number }[];
}

/** Reads a file relative to the `fixtures` directory. */
export function readFixture(file: string): Uint8Array {
  return readFileSync(new URL(file, ROOT));
}

export function fixtures(format: DecompressionFormat): Fixture[] {
  const manifest = JSON.parse(readFileSync(new URL('manifest.json', ROOT), 'utf8')) as { fixtures: ManifestEntry[] };

  return manifest.fixtures
    .filter((entry) => entry.format === format)
    .map((entry) => ({
      file: entry.file,
      format,
      original: concat(entry.original.map(sourceBytes)),
      expect: expectation(entry.expect),
      limit: entry.limit,
      cuts: entry.cuts
    }));
}

/** Drives `decoder` over the whole of `input` to the end and returns the output; errors are rethrown. */
export function decodeAll(decoder: WasmDecoder, input: Uint8Array): Uint8Array {
  const steps = decodeSteps(decoder, input);
  let step = steps.next();
  while (!step.done) {
    step = steps.next();
  }

  return step.value;
}

/**
 * Like `decodeAll`, one `process` call per step, so that decoders can be interleaved.
 *
 * Throws when the decoder makes no progress three calls in a row.
 */
export function* decodeSteps(decoder: WasmDecoder, input: Uint8Array): Generator<void, Uint8Array, void> {
  const chunks: Uint8Array[] = [];
  let offset = 0;
  let stalled = 0;
  for (;;) {
    const { consumed, produced, done } = decoder.process(input.subarray(offset), true);
    offset += consumed;

    if (produced > 0) {
      chunks.push(decoder.takeOutput());
    }

    if (done) {
      return concat(chunks);
    }

    stalled = consumed === 0 && produced === 0 ? stalled + 1 : 0;

    if (stalled === 3) {
      throw new Error('the decoder made no progress in 3 calls in a row');
    }

    yield;
  }
}

function sourceBytes(source: string): Uint8Array {
  if (!source.startsWith('repeat:')) {
    return readFixture(`originals/${source}`);
  }

  const spec = source.slice('repeat:'.length);
  const separator = spec.lastIndexOf(':');

  return new TextEncoder().encode(spec.slice(0, separator).repeat(Number(spec.slice(separator + 1))));
}

function expectation(name: string): Fixture['expect'] {
  const known = EXPECTATIONS.find((expectation) => expectation === name);

  if (known === undefined) {
    throw new Error(`unknown expectation ${name}`);
  }

  return known;
}

function concat(parts: Uint8Array[]): Uint8Array {
  const whole = new Uint8Array(parts.reduce((length, part) => length + part.length, 0));
  let offset = 0;
  for (const part of parts) {
    whole.set(part, offset);
    offset += part.length;
  }

  return whole;
}
