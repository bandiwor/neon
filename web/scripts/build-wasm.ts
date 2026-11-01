// Builds the Rust module (wasm/) and puts the result where the site imports it from: generated/neon.wasm.
import { spawnSync } from 'node:child_process';
import { copyFileSync, mkdirSync, statSync } from 'node:fs';
import { resolve } from 'node:path';

const root = resolve(import.meta.dirname, '..');
const manifest = resolve(root, 'wasm/Cargo.toml');
const built = resolve(root, 'wasm/target/wasm32-unknown-unknown/release/neon_web.wasm');
export const output = resolve(root, 'generated/neon.wasm');

export function buildWasm(): void {
  console.log('cargo build --release --target wasm32-unknown-unknown');
  const r = spawnSync('cargo', ['build', '--release', '--target', 'wasm32-unknown-unknown', '--manifest-path', manifest], {
    stdio: 'inherit',
  });
  if (r.error) throw new Error(`cannot run cargo: ${r.error.message} (is Rust installed? rustup target add wasm32-unknown-unknown)`);
  if (r.status !== 0) process.exit(r.status ?? 1);
  mkdirSync(resolve(root, 'generated'), { recursive: true });
  copyFileSync(built, output);
  console.log(`generated/neon.wasm  ${(statSync(output).size / 1024 / 1024).toFixed(2)} MB`);
}

if (import.meta.main) buildWasm();
