import { HtpDecompressionError } from './errors.js';
import type { DecompressionFormat } from './format.js';
import { loadModule } from './modules.js';

export const OUTPUT_CHUNK_BYTES = 65_536;
export const INPUT_SLICE_BYTES = 65_536;

// What htp_decoder_process returns besides 0 (continue).
const DONE = 1;
const TRUNCATED = -1;
const CORRUPT = -2;
const UNSUPPORTED = -3;
const MEMORY_LIMIT = -4;
const TRAILING_DATA = -5;

// The ResultArea: consumed (u32) at 0, produced (u32) at 4, message pointer (u32) at 8, message length (u32) at 12,
// needed (f64) at 16, all little-endian.
const RESULT_BYTES = 24;

/** The exports of crates/htp-decompress-wasm that are used here; pointers are offsets into `memory`. */
interface Exports {
  memory: WebAssembly.Memory;
  htp_alloc(len: number): number;
  htp_decoder_new(memoryLimit: number): number;
  htp_decoder_process(
    decoder: number,
    input: number,
    inputLen: number,
    output: number,
    outputLen: number,
    endOfInput: number
  ): number;
  htp_result(): number;
}

/** One decoder in a wasm instance of its own, so that dropping it frees all of its memory. */
export class WasmDecoder {
  /** Undefined once disposed or trapped. */
  #exports: Exports | undefined;
  #failure: HtpDecompressionError | undefined;
  readonly #memoryLimit: number;
  readonly #decoder: number;
  readonly #input: number;
  readonly #output: number;
  readonly #result: number;
  #produced = 0;

  private constructor(exports: Exports, memoryLimit: number) {
    this.#exports = exports;
    this.#memoryLimit = memoryLimit;
    this.#decoder = exports.htp_decoder_new(memoryLimit);
    this.#input = exports.htp_alloc(INPUT_SLICE_BYTES);
    this.#output = exports.htp_alloc(OUTPUT_CHUNK_BYTES);
    this.#result = exports.htp_result();
  }

  /** Instantiates the format's module for a new decoder; `memoryLimit` is a whole number of bytes. */
  static async create(format: DecompressionFormat, memoryLimit: number): Promise<WasmDecoder> {
    const instance = await WebAssembly.instantiate(await loadModule(format));

    return new WasmDecoder(instance.exports as unknown as Exports, memoryLimit);
  }

  /**
   * Decodes from the start of `input`, at most `INPUT_SLICE_BYTES` of it, into the output buffer.
   *
   * `endOfInput` says that no input follows `input`. Take the output with `takeOutput` before the next call.
   */
  process(input: Uint8Array, endOfInput: boolean): { consumed: number; produced: number; done: boolean } {
    const exports = this.#live();
    const slice = input.subarray(0, INPUT_SLICE_BYTES);
    const last = endOfInput && slice.length === input.length;

    new Uint8Array(exports.memory.buffer, this.#input, slice.length).set(slice);

    let code: number;
    try {
      code = exports.htp_decoder_process(
        this.#decoder,
        this.#input,
        slice.length,
        this.#output,
        OUTPUT_CHUNK_BYTES,
        last ? 1 : 0
      );
    } catch (error) {
      // The module imports nothing, so anything thrown is a trap or another engine failure. The instance may be in
      // any state then, so it is dropped.
      this.#exports = undefined;
      this.#failure = new HtpDecompressionError('internal', `the decoder failed: ${String(error)}`, { cause: error });

      throw this.#failure;
    }

    // Memory growth detaches earlier views of memory.buffer, so views are made after the call.
    const result = new DataView(exports.memory.buffer, this.#result, RESULT_BYTES);

    if (code < 0) {
      throw decodeError(code, result, this.#memoryLimit);
    }

    this.#produced = result.getUint32(4, true);

    return { consumed: result.getUint32(0, true), produced: this.#produced, done: code === DONE };
  }

  /** Copies out what the last `process` call produced; a second call returns nothing. */
  takeOutput(): Uint8Array<ArrayBuffer> {
    const exports = this.#live();
    const output = new Uint8Array(exports.memory.buffer, this.#output, this.#produced).slice();

    this.#produced = 0;

    return output;
  }

  /** The size of the instance's memory, 0 once dropped. */
  memoryBytes(): number {
    return this.#exports?.memory.buffer.byteLength ?? 0;
  }

  /** Drops the instance and with it all of its memory. */
  dispose(): void {
    this.#exports = undefined;
  }

  #live(): Exports {
    if (this.#exports === undefined) {
      throw this.#failure ?? new Error('the decoder is disposed');
    }

    return this.#exports;
  }
}

function decodeError(code: number, result: DataView<ArrayBuffer>, memoryLimit: number): HtpDecompressionError {
  switch (code) {
    case TRUNCATED:
      return new HtpDecompressionError('truncated', 'input ends before the end of the stream');
    case CORRUPT:
      return new HtpDecompressionError('corrupt', `corrupt data: ${message(result)}`);
    case UNSUPPORTED:
      return new HtpDecompressionError('unsupported', `unsupported feature: ${message(result)}`);
    case MEMORY_LIMIT:
      return new HtpDecompressionError(
        'memory-limit',
        `decoding needs ${result.getFloat64(16, true)} bytes, which exceeds the memory limit of ${memoryLimit} bytes`
      );
    case TRAILING_DATA:
      return new HtpDecompressionError('trailing-data', 'unexpected data after the end of the stream');
    default:
      return new HtpDecompressionError('internal', `unknown result code ${code}`);
  }
}

function message(result: DataView<ArrayBuffer>): string {
  return new TextDecoder().decode(new Uint8Array(result.buffer, result.getUint32(8, true), result.getUint32(12, true)));
}
