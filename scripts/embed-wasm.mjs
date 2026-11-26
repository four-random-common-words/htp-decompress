// Writes npm/build/<format>.wasm as base64 to npm/src/wasm/<format>.ts and prints a size table in Markdown,
// which also goes to $GITHUB_STEP_SUMMARY when that is set.
import { appendFileSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { gzipSync } from 'node:zlib';

const NPM = new URL('../npm/', import.meta.url);

const FORMATS = ['gzip', 'xz', 'zstd'];

mkdirSync(new URL('src/wasm/', NPM), { recursive: true });

const rows = FORMATS.map((format) => {
  const wasm = readFileSync(new URL(`build/${format}.wasm`, NPM));

  writeFileSync(new URL(`src/wasm/${format}.ts`, NPM), `const base64: string = '${wasm.toString('base64')}';\n\nexport default base64;\n`);

  return `| ${format} | ${wasm.length} | ${gzipSync(wasm, { level: 9 }).length} |`;
});

const table = ['| Module | Raw bytes | gzip -9 bytes |', '| --- | ---: | ---: |', ...rows, ''].join('\n');

process.stdout.write(table);

if (process.env.GITHUB_STEP_SUMMARY) {
  appendFileSync(process.env.GITHUB_STEP_SUMMARY, `### wasm sizes\n\n${table}`);
}
