// Every text the interface asks for must exist in the Russian dictionary, and none may be left unused.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';
import ru from '../src/locales/ru.js';

function sources(dir) {
  return readdirSync(dir).flatMap((f) => {
    const p = join(dir, f);
    if (statSync(p).isDirectory()) return f === 'locales' ? [] : sources(p);
    return /\.(jsx?|mjs)$/.test(f) ? [p] : [];
  });
}

const code = sources(new URL('../src', import.meta.url).pathname).map((p) => readFileSync(p, 'utf8')).join('\n');
const used = new Set([...code.matchAll(/\bt\('([a-z_]+\.[a-z_0-9-]+)'/g)].map((m) => m[1]));
// keys built from a variable: `t(`panel.${id}`)`
const prefixes = [...code.matchAll(/\bt\(`([a-z_]+)\.\$\{/g)].map((m) => `${m[1]}.`);

test('every key used in the code has a Russian text', () => {
  const missing = [...used].filter((k) => !(k in ru));
  assert.deepEqual(missing, []);
});

test('no Russian text is unused', () => {
  const unused = Object.keys(ru).filter((k) => !used.has(k) && !prefixes.some((p) => k.startsWith(p)));
  assert.deepEqual(unused, []);
});

test('placeholders are well formed', () => {
  for (const [k, v] of Object.entries(ru)) {
    assert.ok(!/\{[^}a-z]/.test(v) && !/\{\}/.test(v), `${k}: ${v}`);
  }
});
