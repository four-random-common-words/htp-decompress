import { HtpDecompressionError } from './errors.js';
import { type DecompressionFormat, FORMATS } from './format.js';
import { yieldIfDue } from './time-slice.js';
import { WasmDecoder } from './wasm-decoder.js';

export interface HtpDecompressionOptions {
  /** Largest zstd window or xz dictionary a file may declare, in bytes. Default 128 MiB. */
  memoryLimit?: number;
}

export const DEFAULT_MEMORY_LIMIT = 134_217_728;

/** How a pending `write()` or `close()` of the writable side settles. */
interface Pending {
  resolve(): void;
  reject(reason: unknown): void;
}

/** A written chunk with the bytes the decoder has not taken yet. */
interface Input extends Pending {
  bytes: Uint8Array;
}

/**
 * Decompresses gzip, xz or zstd like `DecompressionStream`, except that on an error the reader first gets everything
 * decoded before it, and the error is an `HtpDecompressionError` with a code.
 *
 * The readable side decodes only when it is read from, one output chunk per pull.
 */
export class HtpDecompressionStream {
  readonly readable: ReadableStream<Uint8Array<ArrayBuffer>>;
  readonly writable: WritableStream<BufferSource>;
  readonly #format: DecompressionFormat;
  readonly #memoryLimit: number;
  #readableController!: ReadableStreamDefaultController<Uint8Array<ArrayBuffer>>;
  #writableController!: WritableStreamDefaultController;
  /** Started by the first write or pull. */
  #loading: Promise<void> | undefined;
  /** Undefined until loaded, and again once the stream has ended. */
  #decoder: WasmDecoder | undefined;
  /** The chunk being decoded; the writable side hands over one chunk at a time. */
  #input: Input | undefined;
  /** Set once the writable side is closing: no input follows `#input`. */
  #closing: Pending | undefined;
  /** Lets a pull that waits for a write or close go on. */
  #waiting: (() => void) | undefined;
  /** Whether the last decoder call made no progress: only then can the decoder hold no output that it could hand out. */
  #drained = true;
  #ended = false;

  constructor(format: DecompressionFormat, options?: HtpDecompressionOptions) {
    if (!FORMATS.includes(format)) {
      throw new TypeError(`unknown decompression format ${String(format)}`);
    }

    const memoryLimit = options?.memoryLimit ?? DEFAULT_MEMORY_LIMIT;

    if (!Number.isSafeInteger(memoryLimit) || memoryLimit <= 0) {
      throw new RangeError(`memoryLimit must be a positive whole number of bytes, not ${String(memoryLimit)}`);
    }

    this.#format = format;
    this.#memoryLimit = memoryLimit;
    this.readable = new ReadableStream(
      {
        start: (controller) => {
          this.#readableController = controller;
        },
        pull: () => this.#pull(),
        cancel: (reason) => this.#cancel(reason)
      },
      { highWaterMark: 0 }
    );
    this.writable = new WritableStream({
      start: (controller) => {
        this.#writableController = controller;

        // Signalled at once, also while a write is in flight; the sink's abort is called only once that write settles.
        controller.signal.addEventListener('abort', () => this.#abort(controller.signal.reason));
      },
      write: (chunk) => this.#write(chunk),
      close: () => this.#close(),
      abort: (reason) => this.#abort(reason)
    });
  }

  async #pull(): Promise<void> {
    await this.#load();

    try {
      for (;;) {
        // Without input, the decoder is called with none until it has nothing left to hand out; only then does the pull
        // wait. The decoder is undefined once the stream has ended, by a cancel or abort while this pull waited, say.
        while (this.#decoder !== undefined && this.#input === undefined && this.#closing === undefined && this.#drained) {
          await new Promise<void>((resolve) => {
            this.#waiting = resolve;
          });
        }

        if (this.#decoder === undefined) {
          return;
        }

        const pause = yieldIfDue();

        if (pause !== undefined) {
          await pause;
        }

        if (this.#decoder === undefined || this.#step(this.#decoder)) {
          return;
        }
      }
    } catch (error) {
      this.#fail(decompressionError(error, 'the decoder failed'));
    }
  }

  /** Runs one decoder call; true once this pull is over, with a chunk delivered or the stream closed. */
  #step(decoder: WasmDecoder): boolean {
    const input = this.#input;
    const { consumed, produced, done } = decoder.process(input?.bytes ?? new Uint8Array(), this.#closing !== undefined);

    this.#drained = consumed === 0 && produced === 0;

    if (input !== undefined) {
      input.bytes = input.bytes.subarray(consumed);

      if (input.bytes.length === 0) {
        this.#input = undefined;
        input.resolve();
      }
    }

    if (produced > 0) {
      this.#readableController.enqueue(decoder.takeOutput());
    }

    if (done) {
      this.#finish();
    }

    return produced > 0 || done;
  }

  #write(chunk: unknown): Promise<void> {
    let bytes: Uint8Array;
    try {
      bytes = copyBufferSource(chunk);
    } catch (error) {
      this.#fail(error);

      return Promise.reject(error);
    }

    if (bytes.length === 0) {
      return Promise.resolve();
    }

    void this.#load();

    // Settles once the decoder has taken all of the bytes, so that backpressure reaches the source.
    return new Promise((resolve, reject) => {
      this.#input = { bytes, resolve, reject };
      this.#wake();
    });
  }

  /** Settles once all input is decoded. */
  #close(): Promise<void> {
    return new Promise((resolve, reject) => {
      this.#closing = { resolve, reject };
      this.#wake();
    });
  }

  /** Loads the decoder on the first write or pull; settles once it is there or the stream has ended. */
  #load(): Promise<void> {
    this.#loading ??= WasmDecoder.create(this.#format, this.#memoryLimit).then(
      (decoder) => {
        if (this.#ended) {
          decoder.dispose();
        } else {
          this.#decoder = decoder;
        }
      },
      (error: unknown) => {
        this.#fail(decompressionError(error, `loading the ${this.#format} decoder failed`));
      }
    );

    return this.#loading;
  }

  /** The clean end: the writable side closed and the decoder reached the end of the data. */
  #finish(): void {
    if (this.#stop()) {
      this.#readableController.close();
      this.#closing?.resolve();
      this.#closing = undefined;
    }
  }

  /** A decoder error, a failed load or a bad write errors both sides. */
  #fail(error: unknown): void {
    if (this.#stop()) {
      this.#readableController.error(error);
      this.#writableController.error(error);
      this.#rejectPending(error);
    }
  }

  /** `readable.cancel(reason)` errors the writable side, so that an upstream pipe stops. */
  #cancel(reason: unknown): void {
    if (this.#stop()) {
      this.#writableController.error(reason);
      this.#rejectPending(reason);
    }
  }

  /** `writable.abort(reason)` errors the readable side. */
  #abort(reason: unknown): void {
    if (this.#stop()) {
      this.#readableController.error(reason);
      this.#rejectPending(reason);
    }
  }

  /** Marks the stream ended, drops the decoder and lets a waiting pull return; false if it had already ended. */
  #stop(): boolean {
    if (this.#ended) {
      return false;
    }

    this.#ended = true;
    this.#decoder?.dispose();
    this.#decoder = undefined;
    this.#wake();

    return true;
  }

  #rejectPending(reason: unknown): void {
    this.#input?.reject(reason);
    this.#closing?.reject(reason);
    this.#input = undefined;
    this.#closing = undefined;
  }

  #wake(): void {
    this.#waiting?.();
    this.#waiting = undefined;
  }
}

/** A copy of the bytes of an `ArrayBuffer` or `ArrayBufferView`; a `TypeError` for anything else. */
function copyBufferSource(chunk: unknown): Uint8Array {
  if (chunk instanceof ArrayBuffer) {
    return new Uint8Array(chunk.slice(0));
  }

  if (ArrayBuffer.isView(chunk)) {
    return new Uint8Array(chunk.buffer, chunk.byteOffset, chunk.byteLength).slice();
  }

  throw new TypeError(`a written chunk must be an ArrayBuffer or ArrayBufferView, not ${typeof chunk}`);
}

/** `error` itself when it is an `HtpDecompressionError`, else an internal error caused by it. */
function decompressionError(error: unknown, what: string): HtpDecompressionError {
  return error instanceof HtpDecompressionError
    ? error
    : new HtpDecompressionError('internal', `${what}: ${String(error)}`, { cause: error });
}
