// Every text the interface asks for must exist, and none may be left unused. (The compiler checks the
// literal keys; this finds the dictionary entries nobody asks for.)
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { LOCALES, ru, type Locale } from '../../src/locales';
import { plural, count } from '../../src/core/i18n';

const src = resolve(import.meta.dirname, '../../src');

function sources(dir: string): string[] {
  return readdirSync(dir).flatMap((f) => {
    const p = join(dir, f);
    if (statSync(p).isDirectory()) return f === 'locales' ? [] : sources(p);
    return /\.tsx?$/.test(f) ? [p] : [];
  });
}

const code = sources(src).map((p) => readFileSync(p, 'utf8')).join('\n');
// Any quoted 'namespace.key' counts as a use: t('a.b'), label('a.b', ...), cmd('x', 'a.b', ...), lists of keys.
const used = new Set([...code.matchAll(/'([a-z_]+\.[a-z_0-9-]+)'/g)].flatMap((m) => (m[1] ? [m[1]] : [])));
// keys built from a variable: t(`panel.${id}`)
const prefixes = [...code.matchAll(/`([a-z_]+\.[a-z_]*)\$\{/g)].flatMap((m) => (m[1] ? [m[1]] : []));

describe('texts', () => {
  it('has no unused entries', () => {
    const unused = Object.keys(ru).filter((k) => !used.has(k) && !prefixes.some((p) => k.startsWith(p)));
    expect(unused).toEqual([]);
  });

  it('every language has every key', () => {
    for (const [name, dictionary] of Object.entries(LOCALES) as [Locale, Record<string, string>][]) {
      expect(Object.keys(dictionary).sort(), name).toEqual(Object.keys(ru).sort());
    }
  });

  it('has well formed placeholders', () => {
    for (const [k, v] of Object.entries(ru)) {
      expect(/\{[^}a-z]/.test(v) || /\{\}/.test(v), `${k}: ${v}`).toBe(false);
    }
  });

  it('forms Russian plurals', () => {
    const forms = ['файл', 'файла', 'файлов'] as const;
    expect([1, 2, 5, 11, 12, 21, 22, 25, 101, 111].map((n) => plural(n, forms))).toEqual([
      'файл', 'файла', 'файлов', 'файлов', 'файлов', 'файл', 'файла', 'файлов', 'файл', 'файлов',
    ]);
    expect(count(3, forms)).toBe('3 файла');
  });
});
