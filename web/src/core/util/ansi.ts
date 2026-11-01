// Terminal output for the console: SGR colors and "clear the screen" are honoured; output that moves the
// cursor around is drawn on a grid (renderScreen); the rest is dropped.

const BASIC = ['black', 'red', 'green', 'yellow', 'blue', 'magenta', 'cyan', 'white'] as const;

/** A foreground color: a named one (the theme decides its shade) or an exact one. */
export type AnsiColor = { readonly name: string } | { readonly rgb: readonly [number, number, number] };

export interface AnsiRun {
  readonly text: string;
  readonly fg?: AnsiColor | undefined;
  readonly bold?: boolean | undefined;
  readonly dim?: boolean | undefined;
  readonly underline?: boolean | undefined;
}

export interface ParsedOutput {
  readonly runs: readonly AnsiRun[];
  /** The text was longer than the limit and its beginning was dropped. */
  readonly truncated: boolean;
}

function color256(n: number): AnsiColor {
  if (n < 16) return { name: `${n < 8 ? '' : 'b-'}${BASIC[n % 8] ?? 'white'}` };
  if (n >= 232) {
    const v = 8 + (n - 232) * 10;
    return { rgb: [v, v, v] };
  }
  const i = n - 16;
  const level = (c: number): number => (c === 0 ? 0 : 55 + c * 40);
  return { rgb: [level(Math.floor(i / 36)), level(Math.floor(i / 6) % 6), level(i % 6)] };
}

type Style = Omit<AnsiRun, 'text'>;

/** The style after an SGR sequence (`ESC [ params m`). */
function sgr(from: Style, params: string): Style {
  let style = from;
  const codes = params === '' ? [0] : params.split(';').map((n) => Number(n) || 0);
  for (let k = 0; k < codes.length; k++) {
    const c = codes[k] ?? 0;
    if (c === 0) style = {};
    else if (c === 1) style = { ...style, bold: true };
    else if (c === 2) style = { ...style, dim: true };
    else if (c === 4) style = { ...style, underline: true };
    else if (c === 22) style = { ...style, bold: false, dim: false };
    else if (c >= 30 && c <= 37) style = { ...style, fg: { name: BASIC[c - 30] ?? 'white' } };
    else if (c >= 90 && c <= 97) style = { ...style, fg: { name: `b-${BASIC[c - 90] ?? 'white'}` } };
    else if (c === 39) style = { ...style, fg: undefined };
    else if (c === 38 && codes[k + 1] === 5) {
      style = { ...style, fg: color256(codes[k + 2] ?? 0) };
      k += 2;
    } else if (c === 38 && codes[k + 1] === 2) {
      style = { ...style, fg: { rgb: [codes[k + 2] ?? 0, codes[k + 3] ?? 0, codes[k + 4] ?? 0] } };
      k += 4;
    }
  }
  return style;
}

interface Cell {
  readonly ch: string;
  readonly style: Style;
}

/** Output that moves the cursor around is a picture of a screen, not a stream: draw it on a grid. */
const ADDRESSING = /\x1b\[(?:\d*;\d*[Hf]|\d*[KABCD]|[01]?J|\?1049[hl])/;

/**
 * The screen a stream of text and escape sequences leaves: cursor positioning (`H`, `A`-`D`), erasing
 * (`J`, `K`), colors, and the alternate screen. A newline goes to the start of the next row.
 */
function renderScreen(text: string): AnsiRun[] {
  const rows: Cell[][] = [[]];
  let r = 0;
  let c = 0;
  let style: Style = {};
  const blank: Cell = { ch: ' ', style: {} };
  const row = (n: number): Cell[] => {
    while (rows.length <= n) rows.push([]);
    return rows[n] ?? [];
  };
  const put = (ch: string): void => {
    const cells = row(r);
    while (cells.length < c) cells.push(blank);
    cells[c] = { ch, style };
    c++;
  };
  const re = /\x1b\[([0-9;?]*)([A-Za-z])/y;
  let i = 0;
  while (i < text.length) {
    const ch = text[i] ?? '';
    if (ch === '\x1b') {
      re.lastIndex = i;
      const m = re.exec(text);
      if (!m) {
        i++;
        continue;
      }
      i += m[0].length;
      const params = m[1] ?? '';
      const n = (k: number, fallback: number): number => {
        const v = params.split(';')[k];
        return v === undefined || v === '' || Number.isNaN(Number(v)) ? fallback : Number(v);
      };
      switch (m[2]) {
        case 'm':
          style = sgr(style, params);
          break;
        case 'H':
        case 'f':
          r = Math.max(0, n(0, 1) - 1);
          c = Math.max(0, n(1, 1) - 1);
          break;
        case 'A':
          r = Math.max(0, r - n(0, 1));
          break;
        case 'B':
          r += n(0, 1);
          break;
        case 'C':
          c += n(0, 1);
          break;
        case 'D':
          c = Math.max(0, c - n(0, 1));
          break;
        case 'K': {
          const cells = row(r);
          const mode = n(0, 0);
          if (mode === 0) cells.length = Math.min(cells.length, c);
          else if (mode === 1) for (let k = 0; k <= c && k < cells.length; k++) cells[k] = blank;
          else cells.length = 0;
          break;
        }
        case 'J': {
          const mode = n(0, 0);
          if (mode === 0) {
            row(r).length = Math.min(row(r).length, c);
            rows.length = r + 1;
          } else if (mode === 1) {
            for (let k = 0; k < r; k++) row(k).length = 0;
            const cells = row(r);
            for (let k = 0; k <= c && k < cells.length; k++) cells[k] = blank;
          } else {
            rows.length = 1;
            rows[0] = [];
            r = 0;
            c = 0;
          }
          break;
        }
        case 'h':
        case 'l':
          // the alternate screen starts clean
          if (params === '?1049' && m[2] === 'h') {
            rows.length = 1;
            rows[0] = [];
            r = 0;
            c = 0;
          }
          break;
        default:
          break;
      }
      continue;
    }
    if (ch === '\n') {
      r++;
      c = 0;
      row(r);
    } else if (ch === '\r') {
      c = 0;
    } else if (ch === '\t') {
      do put(' ');
      while (c % 8 !== 0);
    } else {
      // a character outside the BMP is one cell
      const cp = text.codePointAt(i) ?? 0;
      const glyph = String.fromCodePoint(cp);
      i += glyph.length - 1;
      put(glyph);
    }
    i++;
  }
  // the grid as runs: cells of one style together, rows apart; blank rows at the end are not output
  while (rows.length > 1 && (rows[rows.length - 1]?.length ?? 0) === 0) rows.pop();
  const runs: AnsiRun[] = [];
  const same = (a: Style, b: Style): boolean =>
    JSON.stringify(a) === JSON.stringify(b);
  let last: { style: Style; text: string } | null = null;
  const emit = (text: string, st: Style): void => {
    if (last && same(last.style, st)) last.text += text;
    else {
      if (last) runs.push({ text: last.text, ...last.style });
      last = { style: st, text };
    }
  };
  rows.forEach((cells, k) => {
    let end = cells.length;
    while (end > 0 && cells[end - 1]?.ch === ' ' && Object.keys(cells[end - 1]?.style ?? {}).length === 0) end--;
    for (let j = 0; j < end; j++) {
      const cell = cells[j] ?? blank;
      emit(cell.ch, cell.style);
    }
    if (k < rows.length - 1) emit('\n', {});
  });
  if (last) runs.push({ text: (last as { style: Style; text: string }).text, ...(last as { style: Style; text: string }).style });
  return runs;
}

/** `text` as styled runs. A clear-screen sequence drops what came before. */
export function parseAnsi(input: string, limit = 400_000): ParsedOutput {
  let text = input;
  let truncated = false;
  if (text.length > limit) {
    text = text.slice(text.length - limit);
    truncated = true;
  }
  if (ADDRESSING.test(text)) return { runs: renderScreen(text), truncated };
  const runs: AnsiRun[] = [];
  let style: Style = {};
  let buf = '';
  const flush = (): void => {
    if (buf) runs.push({ text: buf, ...style });
    buf = '';
  };
  const re = /\x1b\[([0-9;?]*)([A-Za-z])/y;
  let i = 0;
  while (i < text.length) {
    const ch = text[i];
    if (ch === '\x1b') {
      re.lastIndex = i;
      const m = re.exec(text);
      if (!m) {
        i++;
        continue;
      }
      i += m[0].length;
      const params = m[1] ?? '';
      if (m[2] === 'J' && (params === '2' || params === '3')) {
        buf = '';
        runs.length = 0;
        truncated = false;
      } else if (m[2] === 'm') {
        flush();
        style = sgr(style, params);
      }
      continue;
    }
    if (ch === '\r' && text[i + 1] !== '\n') {
      i++;
      continue;
    }
    buf += ch;
    i++;
  }
  flush();
  return { runs, truncated };
}
