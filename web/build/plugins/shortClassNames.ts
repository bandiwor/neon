// Production class names of one or two letters. Lightning CSS generates unique names for the CSS modules
// (`[hash]_[local]`, e.g. `fMTMOW_btn`); once everything is bundled and minified this plugin renames every
// one of them to the shortest names there are (`a`, `b`, ... `Z`, `aa`, ...), in the stylesheet and in the
// scripts that use them. The renaming is exact: only the names the modules actually export are touched,
// and the order is by name, so the same sources give the same output.
import type { OutputAsset, OutputChunk } from 'rollup';
import type { Plugin } from 'vite';

const FIRST = 'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ';
const REST = `${FIRST}0123456789`;

/** The n-th short name: a..Z, then aa..aZ, a0..a9, ba... (never starts with a digit). */
export function shortName(n: number): string {
  let name = FIRST[n % FIRST.length] ?? 'a';
  let rest = Math.floor(n / FIRST.length);
  while (rest > 0) {
    rest -= 1;
    name += REST[rest % REST.length] ?? 'a';
    rest = Math.floor(rest / REST.length);
  }
  return name;
}

const escapeRegExp = (s: string): string => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');

/** Replaces every whole occurrence of the keys of `names` (a name ends where an identifier character would not continue it). */
export function renameTokens(text: string, names: ReadonlyMap<string, string>): string {
  if (names.size === 0) return text;
  // longest first, so a name that is the start of another is not taken for it
  const alternatives = [...names.keys()].sort((a, b) => b.length - a.length).map(escapeRegExp).join('|');
  const re = new RegExp(`(?<![\\w-])(?:${alternatives})(?![\\w-])`, 'g');
  return text.replace(re, (match) => names.get(match) ?? match);
}

/** The class names a CSS module's JavaScript exports: `export const btn = "fMTMOW_btn";`. */
export function exportedNames(code: string): string[] {
  return [...code.matchAll(/export const [\w$]+ = "([^"\\]+)";/g)].flatMap((m) => (m[1] ? [m[1]] : []));
}

export function shortClassNames(): Plugin {
  const generated = new Set<string>();
  return {
    name: 'neon:short-class-names',
    apply: 'build',
    enforce: 'post',
    transform(code, id) {
      if (/\.module\.s?css(\?|$)/.test(id)) for (const name of exportedNames(code)) generated.add(name);
      return null;
    },
    generateBundle(_options, bundle) {
      const names = new Map<string, string>();
      [...generated].sort().forEach((name, i) => names.set(name, shortName(i)));
      for (const file of Object.values(bundle)) {
        if (file.type === 'chunk') {
          const chunk: OutputChunk = file;
          chunk.code = renameTokens(chunk.code, names);
        } else if (file.fileName.endsWith('.css') && typeof file.source === 'string') {
          const asset: OutputAsset = file;
          asset.source = renameTokens(String(asset.source), names);
        }
      }
    },
  };
}
