// Terminal output for the console: SGR colors and "clear the screen" are honoured, the rest is dropped.

const BASIC = ['black', 'red', 'green', 'yellow', 'blue', 'magenta', 'cyan', 'white'];

function color256(n) {
  if (n < 16) return { cls: `${n < 8 ? '' : 'b-'}${BASIC[n % 8]}` };
  if (n >= 232) {
    const v = 8 + (n - 232) * 10;
    return { rgb: [v, v, v] };
  }
  const i = n - 16;
  const level = (c) => (c === 0 ? 0 : 55 + c * 40);
  return { rgb: [level(Math.floor(i / 36)), level(Math.floor(i / 6) % 6), level(i % 6)] };
}

/** `text` as styled runs: [{text, fg, bold, dim, underline}]. A clear-screen sequence drops what came before. */
export function parseAnsi(text, limit = 400_000) {
  let truncated = false;
  if (text.length > limit) {
    text = text.slice(text.length - limit);
    truncated = true;
  }
  const runs = [];
  let style = {};
  let buf = '';
  const flush = () => {
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
      const params = m[1];
      if (m[2] === 'J' && (params === '2' || params === '3')) {
        buf = '';
        runs.length = 0;
        truncated = false;
      } else if (m[2] === 'm') {
        flush();
        const codes = params === '' ? [0] : params.split(';').map((n) => Number(n) || 0);
        for (let k = 0; k < codes.length; k++) {
          const c = codes[k];
          if (c === 0) style = {};
          else if (c === 1) style = { ...style, bold: true };
          else if (c === 2) style = { ...style, dim: true };
          else if (c === 4) style = { ...style, underline: true };
          else if (c === 22) style = { ...style, bold: false, dim: false };
          else if (c >= 30 && c <= 37) style = { ...style, fg: { cls: BASIC[c - 30] } };
          else if (c >= 90 && c <= 97) style = { ...style, fg: { cls: `b-${BASIC[c - 90]}` } };
          else if (c === 39) style = { ...style, fg: undefined };
          else if (c === 38 && codes[k + 1] === 5) {
            style = { ...style, fg: color256(codes[k + 2] ?? 0) };
            k += 2;
          } else if (c === 38 && codes[k + 1] === 2) {
            style = { ...style, fg: { rgb: [codes[k + 2] ?? 0, codes[k + 3] ?? 0, codes[k + 4] ?? 0] } };
            k += 4;
          }
        }
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
