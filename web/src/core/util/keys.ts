// A key press of the browser as the bytes a terminal would send for it: the program that is running in the
// page reads them from its input exactly as it would read a terminal (see src/term.rs for the names).

export interface KeyLike {
  readonly key: string;
  readonly ctrlKey: boolean;
  readonly altKey: boolean;
  readonly shiftKey: boolean;
  readonly metaKey: boolean;
}

export interface TerminalKey {
  /** What goes into the input. */
  readonly bytes: string;
  /** The name the program will see: `"a"`, `"up"`, `"ctrl+c"`. */
  readonly name: string;
}

const MODIFIER_KEYS = new Set(['Shift', 'Control', 'Alt', 'Meta', 'AltGraph', 'CapsLock', 'NumLock', 'ScrollLock', 'Dead', 'Process', 'Unidentified']);

const CURSOR: Readonly<Record<string, readonly [letter: string, name: string]>> = {
  ArrowUp: ['A', 'up'],
  ArrowDown: ['B', 'down'],
  ArrowRight: ['C', 'right'],
  ArrowLeft: ['D', 'left'],
  Home: ['H', 'home'],
  End: ['F', 'end'],
};

const TILDE: Readonly<Record<string, readonly [code: number, name: string]>> = {
  Insert: [2, 'insert'],
  Delete: [3, 'delete'],
  PageUp: [5, 'pageup'],
  PageDown: [6, 'pagedown'],
};

const FUNCTION_CODES: Readonly<Record<number, number>> = { 5: 15, 6: 17, 7: 18, 8: 19, 9: 20, 10: 21, 11: 23, 12: 24 };

const ESC = '\u001b';

/** The terminal's key for a browser key, or null for one that means nothing to a program (a bare Shift). */
export function terminalKey(e: KeyLike): TerminalKey | null {
  if (MODIFIER_KEYS.has(e.key) || e.metaKey) return null;
  const mod = 1 + (e.shiftKey ? 1 : 0) + (e.altKey ? 2 : 0) + (e.ctrlKey ? 4 : 0);
  const prefix = `${e.ctrlKey ? 'ctrl+' : ''}${e.altKey ? 'alt+' : ''}${e.shiftKey ? 'shift+' : ''}`;
  const cursor = CURSOR[e.key];
  if (cursor) {
    const [letter, name] = cursor;
    return { bytes: mod > 1 ? `${ESC}[1;${mod}${letter}` : `${ESC}[${letter}`, name: prefix + name };
  }
  const tilde = TILDE[e.key];
  if (tilde) {
    const [code, name] = tilde;
    return { bytes: mod > 1 ? `${ESC}[${code};${mod}~` : `${ESC}[${code}~`, name: prefix + name };
  }
  const f = /^F(\d{1,2})$/.exec(e.key);
  if (f) {
    const n = Number(f[1]);
    if (n >= 1 && n <= 4) {
      const letter = 'PQRS'[n - 1] ?? 'P';
      return { bytes: mod > 1 ? `${ESC}[1;${mod}${letter}` : `${ESC}O${letter}`, name: `${prefix}f${n}` };
    }
    const code = FUNCTION_CODES[n];
    if (code !== undefined) return { bytes: mod > 1 ? `${ESC}[${code};${mod}~` : `${ESC}[${code}~`, name: `${prefix}f${n}` };
    return null;
  }
  switch (e.key) {
    case 'Enter':
      return { bytes: '\r', name: 'enter' };
    case 'Backspace':
      return { bytes: '\u007f', name: 'backspace' };
    case 'Escape':
      return { bytes: ESC, name: 'esc' };
    case 'Tab':
      return e.shiftKey ? { bytes: `${ESC}[Z`, name: 'shift+tab' } : { bytes: '\t', name: 'tab' };
  }
  if ([...e.key].length !== 1) return null;
  if (e.ctrlKey) {
    if (e.key === ' ') return { bytes: '\u0000', name: 'ctrl+space' };
    const letter = e.key.toLowerCase();
    if (/^[a-z]$/.test(letter)) {
      return { bytes: String.fromCharCode(letter.charCodeAt(0) - 96), name: `ctrl+${letter}` };
    }
    return null;
  }
  if (e.altKey) return { bytes: ESC + e.key, name: `alt+${e.key}` };
  return { bytes: e.key, name: e.key };
}
