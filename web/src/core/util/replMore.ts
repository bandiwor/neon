/**
 * Whether REPL input ends inside an open bracket, string or block comment, so Enter should continue the
 * input instead of running it. A port of `repl::needs_more` (a test compares the two), so the prompt can
 * decide at once, without waiting for the module.
 */
export function needsMore(text: string): boolean {
  const c = [...text];
  let depth = 0;
  for (let i = 0; i < c.length; i++) {
    const ch = c[i];
    if (ch === '(' || ch === '[' || ch === '{') depth++;
    else if (ch === ')' || ch === ']' || ch === '}') depth--;
    else if (ch === '"') {
      let closed = false;
      for (i++; i < c.length; i++) {
        if (c[i] === '\\') i++;
        else if (c[i] === '"') {
          closed = true;
          break;
        }
      }
      if (!closed) return true;
    } else if (ch === "'") {
      // a char literal like 'a' or '\n'; a lone quote is left to the lexer to reject
      const esc = c[i + 1] === '\\';
      const from = i + 2 + (esc ? 1 : 0);
      const pos = c.slice(from).indexOf("'");
      if (pos >= 0 && pos < 12) i += (esc ? 1 : 0) + pos + 2;
    } else if (ch === '/') {
      if (c[i + 1] === '/') {
        while (i < c.length && c[i] !== '\n') i++;
      } else if (c[i + 1] === '*') {
        i += 2;
        let prev = ' ';
        let closed = false;
        for (; i < c.length; i++) {
          if (prev === '*' && c[i] === '/') {
            closed = true;
            break;
          }
          prev = c[i] ?? ' ';
        }
        if (!closed) return true;
      }
    }
  }
  return depth > 0;
}
