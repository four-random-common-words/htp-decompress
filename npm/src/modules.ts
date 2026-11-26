import type { DecompressionFormat } from './format.js';

const modules = new Map<DecompressionFormat, Promise<WebAssembly.Module>>();

/** Compiles the format's wasm module on first use; later calls share it, unless loading it failed. */
export function loadModule(format: DecompressionFormat): Promise<WebAssembly.Module> {
  let module = modules.get(format);
  if (module === undefined) {
    module = compile(format);
    modules.set(format, module);

    // Forgotten on failure, so that a later stream tries again: a lazy chunk can fail to load while offline, say.
    module.catch(() => modules.delete(format));
  }

  return module;
}

async function compile(format: DecompressionFormat): Promise<WebAssembly.Module> {
  const { default: base64 } = await importBase64(format);

  return WebAssembly.compile(decodeBase64(base64));
}

// Literal specifiers, so that bundlers split each module into a chunk of its own.
function importBase64(format: DecompressionFormat): Promise<{ default: string }> {
  switch (format) {
    case 'gzip':
      return import('./wasm/gzip.js');
    case 'xz':
      return import('./wasm/xz.js');
    case 'zstd':
      return import('./wasm/zstd.js');
  }
}

function decodeBase64(base64: string): Uint8Array<ArrayBuffer> {
  const binary = atob(base64);

  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) {
    bytes[i] = binary.charCodeAt(i);
  }

  return bytes;
}
