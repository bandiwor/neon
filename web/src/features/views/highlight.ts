// Cheap highlighters for the generated listings. They only have to tell comments, strings, numbers and
// words apart, so they are regular expressions that produce HTML-escaped markup.

const C_KEYWORDS = new Set(
  'static void int char const if else for while return goto switch case break default struct union enum typedef unsigned signed long short double float sizeof inline extern do continue'.split(
    ' ',
  ),
);
const C_TYPES = new Set(['uint8_t', 'uint16_t', 'uint32_t', 'uint64_t', 'int8_t', 'int16_t', 'int32_t', 'int64_t', 'size_t', 'bool', 'W', 'Obj', 'Agg', 'Arr', 'Map', 'Str', 'Frame', 'unit_t']);

const LIMIT = 400_000;

export interface ListingClasses {
  readonly keyword: string;
  readonly string: string;
  readonly number: string;
  readonly comment: string;
  readonly type: string;
  readonly label: string;
}

const esc = (s: string): string => s.replace(/&/g, '&amp;').replace(/</g, '&lt;');

function paint(src: string, re: RegExp, classify: (word: string) => string | null): string {
  let out = '';
  let last = 0;
  for (const m of src.matchAll(re)) {
    const word = m[0];
    const at = m.index ?? 0;
    out += esc(src.slice(last, at));
    const cls = classify(word);
    out += cls ? `<span class="${cls}">${esc(word)}</span>` : esc(word);
    last = at + word.length;
  }
  return out + esc(src.slice(last));
}

/** The generated C, as markup; null when it is too long to be worth it. */
export function highlightC(src: string, c: ListingClasses): string | null {
  if (src.length > LIMIT) return null;
  const re = /\/\*[\s\S]*?\*\/|\/\/[^\n]*|"(?:\\.|[^"\\\n])*"|'(?:\\.|[^'\\\n])+'|^#[^\n]*|\b0x[0-9a-fA-F]+[uUlL]*\b|\b\d+(?:\.\d+)?[uUlLfF]*\b|[A-Za-z_]\w*/gm;
  return paint(src, re, (w) => {
    if (w.startsWith('/*') || w.startsWith('//')) return c.comment;
    if (w.startsWith('"') || w.startsWith("'")) return c.string;
    if (w.startsWith('#')) return c.keyword;
    if (/^\d/.test(w)) return c.number;
    if (C_KEYWORDS.has(w)) return c.keyword;
    if (C_TYPES.has(w)) return c.type;
    return null;
  });
}

/** The IR and the bytecode listings: labels, registers, numbers, comments. */
export function highlightListing(src: string, c: ListingClasses): string | null {
  if (src.length > LIMIT) return null;
  const re = /\/\/[^\n]*|#[^\n]*|^f\d+:|^\s*b\d+:|^\s*L\d+:|\bv\d+\b|\br\d+\b|\b(?:return|branch|jump|unreachable|const|intrinsic|call|ret|jmp|loadi|loadk)\b|\b0x[0-9a-fA-F]+\b|\b\d+\b/gm;
  return paint(src, re, (w) => {
    if (w.startsWith('//') || w.startsWith('#')) return c.comment;
    if (/^\s*(?:f|b|L)\d+:$/.test(w)) return c.label;
    if (/^[vr]\d+$/.test(w)) return c.type;
    if (/^[a-z]+$/.test(w)) return c.keyword;
    return c.number;
  });
}
