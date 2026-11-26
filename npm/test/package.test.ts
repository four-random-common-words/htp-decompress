import { execFileSync } from 'node:child_process';
import { readdirSync, statSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

import type { DecompressionFormat, HtpDecompressionErrorCode, HtpDecompressionOptions } from '../src/index.js';

// A missing type export fails `npm run typecheck`.
const format = 'gzip' satisfies DecompressionFormat;
const code = 'corrupt' satisfies HtpDecompressionErrorCode;
const options = { memoryLimit: 1 } satisfies HtpDecompressionOptions;

describe('package', () => {
  it('packs only package.json, README.md, LICENSE and dist', () => {
    const output = execFileSync('npm', ['pack', '--dry-run', '--json'], { encoding: 'utf8' });
    const files = (JSON.parse(output) as { files: { path: string }[] }[])[0].files.map((file) => file.path);
    const allowed = files.filter((path) => ['package.json', 'README.md', 'LICENSE'].includes(path) || path.startsWith('dist/'));

    expect(files).toEqual(allowed);
    expect(files).toEqual(
      expect.arrayContaining([
        'package.json',
        'README.md',
        'LICENSE',
        'dist/index.js',
        'dist/index.d.ts',
        'dist/wasm/gzip.js',
        'dist/wasm/xz.js',
        'dist/wasm/zstd.js',
      ]),
    );
  });

  it('keeps the generated declaration files small', () => {
    const declarations = readdirSync(new URL('../dist/wasm/', import.meta.url)).filter((name) => name.endsWith('.d.ts'));

    expect(declarations.length).toBe(3);

    for (const name of declarations) {
      expect(statSync(new URL(`../dist/wasm/${name}`, import.meta.url)).size, name).toBeLessThan(1024);
    }
  });

  it('exposes exactly the public value exports', async () => {
    const exports = await import('../dist/index.js');

    expect(Object.keys(exports).sort()).toEqual(
      ['DETECT_FORMAT_BYTES', 'DEFAULT_MEMORY_LIMIT', 'HtpDecompressionError', 'HtpDecompressionStream', 'detectFormat'].sort(),
    );
    expect([format, code, options]).toHaveLength(3);
  });
});
