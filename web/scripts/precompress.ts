// Writes a gzip and a Brotli copy next to every text asset of dist/ (the static server sends them as is).
import { brotliCompress, constants, gzip } from 'node:zlib';
import { readdir, readFile, stat, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { promisify } from 'node:util';

const brotli = promisify(brotliCompress);
const gz = promisify(gzip);

const COMPRESSIBLE = /\.(?:html|js|css|json|svg|wasm|txt|map)$/;
const MIN_BYTES = 1024;

async function* walk(dir: string): AsyncGenerator<string> {
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) yield* walk(path);
    else yield path;
  }
}

const dist = resolve(import.meta.dirname, '../dist');
let files = 0;
let before = 0;
let after = 0;
for await (const file of walk(dist)) {
  if (!COMPRESSIBLE.test(file)) continue;
  const size = (await stat(file)).size;
  if (size < MIN_BYTES) continue;
  const data = await readFile(file);
  const [b, g] = await Promise.all([
    brotli(data, { params: { [constants.BROTLI_PARAM_QUALITY]: 11, [constants.BROTLI_PARAM_SIZE_HINT]: data.length } }),
    gz(data, { level: 9 }),
  ]);
  await Promise.all([writeFile(`${file}.br`, b), writeFile(`${file}.gz`, g)]);
  files++;
  before += size;
  after += b.length;
}
console.log(`precompressed ${files} files: ${(before / 1024).toFixed(0)} KB -> ${(after / 1024).toFixed(0)} KB (brotli)`);
