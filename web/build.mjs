// Builds the site into dist/: the WebAssembly module (cargo), the bundles (esbuild), the examples.
//
//   node build.mjs            everything, minified
//   node build.mjs wasm       only the module
//   node build.mjs --watch    rebuild the bundles when a source file changes (the module is built once)
//   node build.mjs --no-wasm  everything but the module (it must already be in dist/)
import { build, context } from 'esbuild';
import { spawnSync } from 'node:child_process';
import { copyFileSync, existsSync, mkdirSync, readFileSync, writeFileSync, rmSync, statSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = dirname(fileURLToPath(import.meta.url));
const repo = resolve(root, '..');
const dist = join(root, 'dist');
const args = process.argv.slice(2);
const watch = args.includes('--watch');
const only = args.find((a) => !a.startsWith('--'));

function buildWasm() {
  const manifest = join(root, 'wasm', 'Cargo.toml');
  console.log('cargo build (wasm32-unknown-unknown, release)');
  const r = spawnSync(
    'cargo',
    ['build', '--release', '--target', 'wasm32-unknown-unknown', '--manifest-path', manifest],
    { stdio: 'inherit' },
  );
  if (r.status !== 0) process.exit(r.status ?? 1);
  mkdirSync(dist, { recursive: true });
  copyFileSync(join(root, 'wasm/target/wasm32-unknown-unknown/release/neon_web.wasm'), join(dist, 'neon.wasm'));
  console.log(`neon.wasm ${(statSync(join(dist, 'neon.wasm')).size / 1024 / 1024).toFixed(2)} MB`);
}

function examples() {
  const manifest = JSON.parse(readFileSync(join(root, 'examples.json'), 'utf8'));
  const out = manifest.map((e) => {
    const file = join(repo, e.source);
    if (!existsSync(file)) throw new Error(`example source missing: ${e.source}`);
    return { id: e.id, group: e.group, title: e.title, description: e.description, interactive: !!e.interactive, files: [{ name: 'main.neon', text: readFileSync(file, 'utf8') }] };
  });
  writeFileSync(join(dist, 'examples.json'), JSON.stringify(out));
  console.log(`examples.json: ${out.length} programs`);
}

function page(version) {
  const html = readFileSync(join(root, 'src/index.html'), 'utf8').replaceAll('__VERSION__', version);
  writeFileSync(join(dist, 'index.html'), html);
  copyFileSync(join(root, 'src/favicon.svg'), join(dist, 'favicon.svg'));
  // GitHub Pages: no Jekyll processing
  writeFileSync(join(dist, '.nojekyll'), '');
}

const common = { bundle: true, sourcemap: false, logLevel: 'info', target: ['es2022'], legalComments: 'none' };

async function bundles() {
  const minify = !watch;
  const jobs = [
    { entryPoints: [join(root, 'src/main.jsx')], outfile: join(dist, 'app.js'), format: 'esm', jsx: 'automatic', jsxImportSource: 'preact', loader: { '.svg': 'text' } },
    { entryPoints: [join(root, 'src/wasm-worker.js')], outfile: join(dist, 'worker.js'), format: 'iife' },
    { entryPoints: [join(root, 'src/styles.css')], outfile: join(dist, 'app.css') },
  ];
  if (watch) {
    for (const j of jobs) {
      const ctx = await context({ ...common, ...j, minify });
      await ctx.watch();
    }
    console.log('watching src/ ...');
    return;
  }
  for (const j of jobs) await build({ ...common, ...j, minify });
}

if (only === 'wasm') {
  buildWasm();
} else {
  if (!args.includes('--no-wasm') && !watch) buildWasm();
  mkdirSync(dist, { recursive: true });
  examples();
  const version = createHash('sha1').update(String(Date.now())).digest('hex').slice(0, 8);
  page(version);
  await bundles();
}
