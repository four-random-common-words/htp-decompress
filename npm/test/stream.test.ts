import { describe, expect, it, vi } from 'vitest';

import { HtpDecompressionError } from '../src/errors.js';
import { type DecompressionFormat, FORMATS } from '../src/format.js';
import { loadModule } from '../src/modules.js';
import { DEFAULT_MEMORY_LIMIT, HtpDecompressionStream } from '../src/stream.js';
import { timeSlice, yieldIfDue } from '../src/time-slice.js';
import { WasmDecoder } from '../src/wasm-decoder.js';
import { decodeUntilError, fixtures, readFixture } from './fixtures.js';

/** What a reader got before the end or an error, and the error. */
interface Read {
  bytes: Uint8Array;
  error: unknown;
}

interface PipeOptions {
  chunkBytes?: number;
  memoryLimit?: number;
}

describe.each(FORMATS)('%s stream', (format) => {
  const done = fixtures(format).filter((fixture) => fixture.expect === 'done');

  it('decodes through pipeThrough', async () => {
    expect(done.length).toBeGreaterThan(0);

    for (const fixture of done) {
      const { bytes, error } = await pipeFixture(fixture.file, { chunkBytes: 16_384 });

      expect(error, fixture.file).toBeUndefined();
      expect(sameBytes(bytes, fixture.original), fixture.file).toBe(true);
    }
  });

  it('decodes one whole-file write', async () => {
    for (const fixture of done) {
      const { bytes, error } = await pipeFixture(fixture.file);

      expect(error, fixture.file).toBeUndefined();
      expect(sameBytes(bytes, fixture.original), fixture.file).toBe(true);
    }
  });

  it('delivers all output, then reports a cut', async () => {
    const cuts = done.flatMap((fixture) => fixture.cuts.map(({ at }) => ({ file: fixture.file, at })));

    expect(cuts.length).toBeGreaterThan(0);

    for (const { file, at } of cuts) {
      const prefix = readFixture(file).subarray(0, at);
      const reference = decodeUntilError(await WasmDecoder.create(format, DEFAULT_MEMORY_LIMIT), prefix);

      const { bytes, error } = await pipe(new HtpDecompressionStream(format), split(prefix, 16_384));

      const label = `${file} cut at ${at}`;
      expect(error, label).toBeInstanceOf(HtpDecompressionError);
      expect(error, label).toHaveProperty('code', 'truncated');
      expect(sameBytes(bytes, reference.output), label).toBe(true);
    }
  });

  it('delivers output before damage', async () => {
    const fixture = fixtures(format).find((candidate) => candidate.file.startsWith(`${format}/corrupt.`))!;
    const decoder = await WasmDecoder.create(format, DEFAULT_MEMORY_LIMIT);
    const reference = decodeUntilError(decoder, readFixture(fixture.file));

    // One write, as the reference gets it: how far xz decodes before it sees damage depends on the input chunks.
    const { bytes, error } = await pipeFixture(fixture.file);

    // A flipped byte is caught at the next checksum, so only the first 64 KiB must match the original (ruling R6b).
    expect(error).toBeInstanceOf(HtpDecompressionError);
    expect(error).toHaveProperty('code', 'corrupt');
    expect(sameBytes(bytes, reference.output)).toBe(true);
    expect(bytes.length).toBeGreaterThanOrEqual(65_536);
    expect(sameBytes(bytes.subarray(0, 65_536), fixture.original.subarray(0, 65_536))).toBe(true);
  });

  it('reports trailing data', async () => {
    const fixture = fixtures(format).find((candidate) => candidate.expect === 'trailing-data')!;
    const decoders = watchDecoders();

    const { bytes, error } = await pipeFixture(fixture.file, { chunkBytes: 16_384 });

    expect(error).toBeInstanceOf(HtpDecompressionError);
    expect(error).toHaveProperty('code', 'trailing-data');
    expect(sameBytes(bytes, fixture.original)).toBe(true);
    expect(await memoryOf(decoders)).toEqual([0]);
  });
});

it('throws a TypeError for an unknown format', () => {
  expect(() => new HtpDecompressionStream('brotli' as never)).toThrow(TypeError);
});

it('throws a RangeError for a bad memoryLimit', () => {
  for (const memoryLimit of [0, -1, Number.NaN, 1.5, Infinity]) {
    expect(() => new HtpDecompressionStream('xz', { memoryLimit })).toThrow(RangeError);
  }
});

it('errors both sides on a non-BufferSource write', async () => {
  const stream = new HtpDecompressionStream('gzip');
  const reader = stream.readable.getReader();
  const read = handled(reader.read());

  const error: unknown = await stream.writable
    .getWriter()
    .write('text' as never)
    .catch((reason: unknown) => reason);

  expect(error).toBeInstanceOf(TypeError);
  await expect(read).rejects.toBe(error);
});

it('reports the memory limit before any output', async () => {
  const { bytes, error } = await pipeFixture('zstd/aaaa-long27.zst', { memoryLimit: 67_108_864 });

  expect(bytes.length).toBe(0);
  expect(error).toBeInstanceOf(HtpDecompressionError);
  expect(error).toHaveProperty('code', 'memory-limit');
});

it('decodes only as far as the reader pulls', async () => {
  const process = vi.spyOn(WasmDecoder.prototype, 'process');
  const reader = source(split(readFixture('zstd/aaaa-long27.zst')))
    .pipeThrough(new HtpDecompressionStream('zstd'))
    .getReader();

  const bytes = await readAtLeast(reader, 65_536);

  for (let i = 0; i < 5; i++) {
    await macrotask();
  }

  await reader.cancel();

  expect(bytes.length).toBe(65_536);
  expect(process.mock.calls.length).toBeLessThanOrEqual(3);
});

it('reads an exact prefix and stops', async () => {
  const reader = source(split(readFixture('gzip/aaaa.gz')))
    .pipeThrough(new HtpDecompressionStream('gzip'))
    .getReader();

  const bytes = await readAtLeast(reader, 65_536);
  await reader.cancel();

  expect(sameBytes(bytes, new TextEncoder().encode('a\n'.repeat(32_768)))).toBe(true);
});

it('passes a readable cancel upstream', async () => {
  const decoders = watchDecoders();
  const stream = new HtpDecompressionStream('gzip');
  const writer = stream.writable.getWriter();
  const reader = stream.readable.getReader();
  const write = handled(writer.write(fixtureBytes('gzip/aaaa.gz')));
  const reason = new Error('enough');

  await reader.read();
  await reader.cancel(reason);

  await expect(write).rejects.toBe(reason);
  await expect(writer.closed).rejects.toBe(reason);
  expect(await memoryOf(decoders)).toEqual([0]);
});

it('stops on an aborted pipeThrough signal', async () => {
  const controller = new AbortController();
  const reader = source(split(readFixture('gzip/aaaa.gz'), 16_384))
    .pipeThrough(new HtpDecompressionStream('gzip'), { signal: controller.signal })
    .getReader();

  await reader.read();
  controller.abort(new Error('stop'));
  const { bytes, error } = await readToEnd(reader);

  expect(error).toBe(controller.signal.reason);
  expect(bytes.length).toBeLessThan(16_777_216);
});

it('passes a writable abort downstream', async () => {
  const decoders = watchDecoders();
  const stream = new HtpDecompressionStream('gzip');
  const writer = stream.writable.getWriter();
  const reader = stream.readable.getReader();
  const write = handled(writer.write(fixtureBytes('gzip/aaaa.gz')));
  const reason = new Error('abort');

  await reader.read();
  const abort = writer.abort(reason);

  await expect(reader.read()).rejects.toBe(reason);
  await expect(write).rejects.toBe(reason);
  await expect(abort).resolves.toBeUndefined();
  expect(await memoryOf(decoders)).toEqual([0]);
});

it('runs streams concurrently and after a cancel', async () => {
  const xz = fixtures('xz');
  const hex = xz.find((fixture) => fixture.file === 'xz/hex.xz')!;
  const concat = xz.find((fixture) => fixture.file === 'xz/concat.xz')!;
  const cancelled = source(split(readFixture(hex.file), 16_384))
    .pipeThrough(new HtpDecompressionStream('xz'))
    .getReader();

  await cancelled.read();
  await cancelled.cancel();

  const [first, second] = await readInTurn(
    [hex, concat].map((fixture) =>
      source(split(readFixture(fixture.file), 16_384))
        .pipeThrough(new HtpDecompressionStream('xz'))
        .getReader()
    )
  );

  expect(sameBytes(first, hex.original)).toBe(true);
  expect(sameBytes(second, concat.original)).toBe(true);
});

it('yields to the event loop', async () => {
  const budgetMs = timeSlice.budgetMs;

  // Loaded beforehand, so that only the stream's yields can let the timer run.
  await loadModule('zstd');
  timeSlice.budgetMs = 0;

  try {
    let fired = false;
    setTimeout(() => {
      fired = true;
    }, 0);

    const reader = source(split(readFixture('zstd/aaaa-long27.zst')))
      .pipeThrough(new HtpDecompressionStream('zstd'))
      .getReader();

    let firedBeforeLast = false;
    for (let chunk = await reader.read(); !chunk.done; chunk = await reader.read()) {
      firedBeforeLast = fired;
    }

    expect(firedBeforeLast).toBe(true);
  } finally {
    timeSlice.budgetMs = budgetMs;
  }
});

it('yields with scheduler.yield, else a MessageChannel, else setTimeout', async () => {
  const budgetMs = timeSlice.budgetMs;
  const schedulerYield = vi.fn(() => Promise.resolve());
  const postMessage = vi.spyOn(MessagePort.prototype, 'postMessage');
  const setTimeoutSpy = vi.spyOn(globalThis, 'setTimeout');
  timeSlice.budgetMs = 0;

  try {
    vi.stubGlobal('scheduler', { yield: schedulerYield });
    await yieldIfDue();
    vi.unstubAllGlobals();
    await yieldIfDue();
    vi.stubGlobal('MessageChannel', undefined);
    await yieldIfDue();
  } finally {
    vi.unstubAllGlobals();
    timeSlice.budgetMs = budgetMs;
  }

  expect(schedulerYield).toHaveBeenCalledOnce();
  expect(postMessage).toHaveBeenCalledOnce();
  expect(setTimeoutSpy).toHaveBeenCalledOnce();
});

it('ignores empty chunks', async () => {
  const fixture = fixtures('gzip').find((candidate) => candidate.file === 'gzip/members.gz')!;
  const input = fixtureBytes(fixture.file);
  const empty = new Uint8Array(0);
  const decoders = watchDecoders();
  const stream = new HtpDecompressionStream('gzip');
  const writer = stream.writable.getWriter();
  const output = readAll(stream.readable);

  for (const chunk of [empty, input.subarray(0, 1_000), empty, empty, input.subarray(1_000), empty, empty]) {
    await writer.write(chunk);
  }

  await writer.close();
  const { bytes, error } = await output;

  expect(error).toBeUndefined();
  expect(sameBytes(bytes, fixture.original)).toBe(true);
  expect(await memoryOf(decoders)).toEqual([0]);
});

it('copies written bytes', async () => {
  const fixture = fixtures('gzip').find((candidate) => candidate.file === 'gzip/hex.gz')!;
  const stream = new HtpDecompressionStream('gzip');
  const writer = stream.writable.getWriter();
  const output = readAll(stream.readable);

  // The stream is idle at each write, so the sink takes the chunk at once; it must not keep the caller's buffer.
  await writer.ready;

  for (const chunk of split(readFixture(fixture.file), 16_384)) {
    const written = writer.write(chunk);
    chunk.fill(0);
    await written;
  }

  await writer.close();
  const { bytes, error } = await output;

  expect(error).toBeUndefined();
  expect(sameBytes(bytes, fixture.original)).toBe(true);
});

it('cancel during module load settles everything', async () => {
  const fixture = fixtures('xz').find((candidate) => candidate.file === 'xz/hex.xz')!;
  const decoder = await WasmDecoder.create('xz', DEFAULT_MEMORY_LIMIT);

  let finishLoad = () => {};
  const create = vi.spyOn(WasmDecoder, 'create').mockReturnValueOnce(
    new Promise((resolve) => {
      finishLoad = () => resolve(decoder);
    })
  );

  const stream = new HtpDecompressionStream('xz');
  const writer = stream.writable.getWriter();
  const reader = stream.readable.getReader();
  const write = handled(writer.write(fixtureBytes(fixture.file)));
  const close = handled(writer.close());
  const read = reader.read();
  const reason = new Error('cancelled');

  // The load cannot finish before finishLoad, so the stream is still loading at the cancel.
  await macrotask();
  const cancel = reader.cancel(reason);
  finishLoad();

  expect(create).toHaveBeenCalledOnce();
  await expect(read).resolves.toEqual({ done: true, value: undefined });
  await expect(cancel).resolves.toBeUndefined();
  await expect(write).rejects.toBe(reason);
  await expect(close).rejects.toBe(reason);
  expect(decoder.memoryBytes()).toBe(0);

  const later = await pipeFixture(fixture.file);

  expect(later.error).toBeUndefined();
  expect(sameBytes(later.bytes, fixture.original)).toBe(true);
});

it('accepts every buffer source', async () => {
  const fixture = fixtures('xz').find((candidate) => candidate.file === 'xz/hex.xz')!;
  const input = readFixture(fixture.file);

  const buffer = new ArrayBuffer(input.length + 4);
  new Uint8Array(buffer, 2, input.length).set(input);

  const chunks: BufferSource[] = [
    new Uint8Array(input).buffer,
    new DataView(buffer, 2, input.length),
    new Uint16Array(buffer, 2, input.length / 2)
  ];

  expect(input.length % 2).toBe(0);

  for (const chunk of chunks) {
    const { bytes, error } = await pipe(new HtpDecompressionStream('xz'), [chunk]);

    expect(error, chunk.constructor.name).toBeUndefined();
    expect(sameBytes(bytes, fixture.original), chunk.constructor.name).toBe(true);
  }
});

it('reports a failed load as an internal error', async () => {
  const cause = new Error('out of memory');
  vi.spyOn(WasmDecoder, 'create').mockRejectedValueOnce(cause);

  const { bytes, error } = await pipeFixture('gzip/hex.gz');

  expect(bytes.length).toBe(0);
  expect(error).toBeInstanceOf(HtpDecompressionError);
  expect(error).toHaveProperty('code', 'internal');
  expect((error as Error).cause).toBe(cause);
});

it('passes on a decompression error from the load', async () => {
  const failure = new HtpDecompressionError('memory-limit', 'no room');
  vi.spyOn(WasmDecoder, 'create').mockRejectedValueOnce(failure);

  const { error } = await pipeFixture('gzip/hex.gz');

  expect(error).toBe(failure);
});

it('loads a module again after a failed load', async () => {
  const fixture = fixtures('gzip').find((candidate) => candidate.file === 'gzip/hex.gz')!;

  // A fresh module graph, so that no earlier test has compiled the gzip module yet.
  vi.resetModules();
  const fresh = await import('../src/stream.js');
  const { HtpDecompressionError: FreshError } = await import('../src/errors.js');
  const cause = new Error('offline');
  vi.spyOn(WebAssembly, 'compile').mockRejectedValueOnce(cause);

  const failed = await pipe(new fresh.HtpDecompressionStream('gzip'), split(readFixture(fixture.file)));
  const retried = await pipe(new fresh.HtpDecompressionStream('gzip'), split(readFixture(fixture.file)));

  expect(failed.error).toBeInstanceOf(FreshError);
  expect(failed.error).toHaveProperty('code', 'internal');
  expect((failed.error as Error).cause).toBe(cause);
  expect(retried.error).toBeUndefined();
  expect(sameBytes(retried.bytes, fixture.original)).toBe(true);
});

/** Decodes a fixture through `pipeThrough`, written in pieces of `chunkBytes` (default: all at once). */
function pipeFixture(file: string, { chunkBytes, memoryLimit }: PipeOptions = {}): Promise<Read> {
  const format = file.slice(0, file.indexOf('/')) as DecompressionFormat;

  return pipe(new HtpDecompressionStream(format, { memoryLimit }), split(readFixture(file), chunkBytes));
}

function pipe(stream: HtpDecompressionStream, chunks: BufferSource[]): Promise<Read> {
  return readAll(source(chunks).pipeThrough(stream));
}

/** Copies of the consecutive pieces of `input`, each `chunkBytes` long but the last. */
function split(input: Uint8Array, chunkBytes = Infinity): Uint8Array<ArrayBuffer>[] {
  const chunks: Uint8Array<ArrayBuffer>[] = [];
  for (let offset = 0; offset < input.length; offset += chunkBytes) {
    chunks.push(new Uint8Array(input.subarray(offset, offset + chunkBytes)));
  }

  return chunks;
}

/** A source that hands out one chunk per pull, like a file being read. */
function source(chunks: BufferSource[]): ReadableStream<BufferSource> {
  const queue = [...chunks];

  return new ReadableStream<BufferSource>(
    {
      pull(controller) {
        const chunk = queue.shift();

        if (chunk === undefined) {
          controller.close();
        } else {
          controller.enqueue(chunk);
        }
      }
    },
    { highWaterMark: 0 }
  );
}

function readAll(readable: ReadableStream<Uint8Array>): Promise<Read> {
  return readToEnd(readable.getReader());
}

async function readToEnd(reader: ReadableStreamDefaultReader<Uint8Array>): Promise<Read> {
  const chunks: Uint8Array[] = [];
  try {
    for (let chunk = await reader.read(); !chunk.done; chunk = await reader.read()) {
      chunks.push(chunk.value);
    }
  } catch (error) {
    return { bytes: Buffer.concat(chunks), error };
  }

  return { bytes: Buffer.concat(chunks), error: undefined };
}

/** Reads until at least `length` bytes have come, or the end. */
async function readAtLeast(reader: ReadableStreamDefaultReader<Uint8Array>, length: number): Promise<Uint8Array> {
  const chunks: Uint8Array[] = [];
  let read = 0;
  while (read < length) {
    const chunk = await reader.read();

    if (chunk.done) {
      break;
    }

    chunks.push(chunk.value);
    read += chunk.value.length;
  }

  return Buffer.concat(chunks);
}

/** Reads the readers to their ends, one chunk from each in turn. */
async function readInTurn(readers: ReadableStreamDefaultReader<Uint8Array>[]): Promise<Uint8Array[]> {
  const chunks = readers.map((): Uint8Array[] => []);
  const open = new Set(readers.keys());
  while (open.size > 0) {
    for (const index of open) {
      const chunk = await readers[index].read();

      if (chunk.done) {
        open.delete(index);
      } else {
        chunks[index].push(chunk.value);
      }
    }
  }

  return chunks.map((parts) => Buffer.concat(parts));
}

/** Spies on `WasmDecoder.create`; the result gives the decoders made since. */
function watchDecoders(): () => Promise<WasmDecoder[]> {
  const create = vi.spyOn(WasmDecoder, 'create');

  return () => Promise.all(create.mock.results.map((result) => result.value as Promise<WasmDecoder>));
}

/** The memory of each decoder made, 0 for a dropped one. */
async function memoryOf(decoders: () => Promise<WasmDecoder[]>): Promise<number[]> {
  return (await decoders()).map((decoder) => decoder.memoryBytes());
}

function fixtureBytes(file: string): Uint8Array<ArrayBuffer> {
  return new Uint8Array(readFixture(file));
}

/** Marks a promise as handled, so that a rejection checked later is not reported as unhandled. */
function handled<T>(promise: Promise<T>): Promise<T> {
  promise.catch(() => {});

  return promise;
}

function macrotask(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

function sameBytes(a: Uint8Array, b: Uint8Array): boolean {
  return Buffer.from(a.buffer, a.byteOffset, a.byteLength).equals(b);
}
