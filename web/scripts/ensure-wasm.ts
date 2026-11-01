// For `npm run dev`: builds the module only when it is missing or older than the Rust sources.
import { existsSync, readdirSync, statSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { buildWasm, output } from './build-wasm';

const repo = resolve(import.meta.dirname, '../..');

function newestSource(dir: string): number {
  let newest = 0;
  for (const name of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, name.name);
    if (name.isDirectory()) newest = Math.max(newest, newestSource(path));
    else if (name.name.endsWith('.rs') || name.name === 'Cargo.toml') newest = Math.max(newest, statSync(path).mtimeMs);
  }
  return newest;
}

const sources = Math.max(newestSource(join(repo, 'src')), newestSource(join(repo, 'web/wasm/src')), statSync(join(repo, 'Cargo.toml')).mtimeMs);
if (!existsSync(output) || statSync(output).mtimeMs < sources) buildWasm();
else console.log('generated/neon.wasm is up to date');
