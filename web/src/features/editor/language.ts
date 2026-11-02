// The Neon language for CodeMirror: a stream tokenizer (what the lexer sees) and the mapping of its
// tokens to class names (the editor's module provides the names).
import { StreamLanguage, type StringStream } from '@codemirror/language';
import { tagHighlighter, tags } from '@lezer/highlight';

const KEYWORDS = new Set([
  'let', 'var', 'const', 'fn', 'return', 'if', 'else', 'elif', 'while', 'loop', 'for', 'in', 'break',
  'continue', 'match', 'enum', 'struct', 'trait', 'impl', 'import', 'export', 'extern', 'as', 'true', 'false', 'self', 'Self',
]);
const PRIMITIVES = new Set(['i8', 'i16', 'i32', 'i64', 'u8', 'u16', 'u32', 'u64', 'f32', 'f64', 'bool', 'char', 'string']);
const OPERATORS = /^(?:->|=>|==|!=|<=|>=|&&|\|\||<<=?|>>=?|\.\.=?|[+\-*/%&|^]=?|[!<>=~?])/;

interface LexState {
  /** Depth of nested block comments. */
  comment: number;
  /** Inside a string: a plain one or an f-string. */
  str: null | 'plain' | 'f';
  /** Brace depth inside `{...}` of an f-string. */
  interp: number;
}

function scanString(stream: StringStream, st: LexState): string {
  while (!stream.eol()) {
    const c = stream.next();
    if (c === '\\') stream.next();
    else if (c === '"') {
      st.str = null;
      return 'string';
    } else if (c === '{' && st.str === 'f') {
      if (stream.peek() === '{') {
        stream.next();
        continue;
      }
      stream.backUp(1);
      return 'string';
    }
  }
  return 'string';
}

function token(stream: StringStream, st: LexState): string | null {
  if (st.comment > 0) {
    while (!stream.eol()) {
      if (stream.match('*/')) {
        if (--st.comment === 0) break;
      } else if (stream.match('/*')) st.comment++;
      else stream.next();
    }
    return 'comment';
  }
  if (st.str) {
    if (st.str === 'f' && stream.peek() === '{' && !stream.match('{{', false)) {
      stream.next();
      st.str = null;
      st.interp = 1;
      return 'bracket';
    }
    return scanString(stream, st);
  }
  if (stream.eatSpace()) return null;
  if (stream.match('//')) {
    stream.skipToEnd();
    return 'comment';
  }
  if (stream.match('/*')) {
    st.comment = 1;
    return token(stream, st);
  }
  if (stream.match('f"')) {
    st.str = 'f';
    return scanString(stream, st);
  }
  if (stream.match('"')) {
    st.str = 'plain';
    return scanString(stream, st);
  }
  if (stream.match(/^'(?:\\.|[^'\\])'/)) return 'string';
  if (stream.match(/^(?:0[xX][0-9a-fA-F_]+|0[bB][01_]+|\d[\d_]*(?:\.\d[\d_]*)?(?:[eE][+-]?\d+)?)[A-Za-z0-9]*/)) return 'number';
  if (stream.match(/^[A-Za-z_\u0080-\uffff][A-Za-z0-9_\u0080-\uffff]*/)) {
    const w = stream.current();
    if (KEYWORDS.has(w)) return 'keyword';
    if (PRIMITIVES.has(w) || /^[A-Z]/.test(w)) return 'typeName';
    if (stream.match(/^\s*\(/, false)) return 'variableName.function';
    return 'variableName';
  }
  if (stream.match('::')) return 'punctuation';
  if (stream.match(OPERATORS)) return 'operator';
  const ch = stream.next();
  if (ch === undefined) return null;
  if (ch === '{' && st.interp > 0) st.interp++;
  if (ch === '}' && st.interp > 0) {
    if (st.interp === 1) {
      st.interp = 0;
      st.str = 'f';
    } else st.interp--;
  }
  if (ch === ':' && st.interp === 1) {
    // a format specifier: `{x:>5.2f}`
    while (!stream.eol() && stream.peek() !== '}' && stream.peek() !== '"') stream.next();
    return 'string';
  }
  if ('()[]{}'.includes(ch)) return 'bracket';
  if ('.,;:'.includes(ch)) return 'punctuation';
  return null;
}

export const neonLanguage = StreamLanguage.define<LexState>({
  name: 'neon',
  startState: () => ({ comment: 0, str: null, interp: 0 }),
  copyState: (s) => ({ ...s }),
  token,
  languageData: {
    commentTokens: { line: '//', block: { open: '/*', close: '*/' } },
    closeBrackets: { brackets: ['(', '[', '{', '"', "'"] },
    indentOnInput: /^\s*[})\]]$/,
  },
});

/** The class names the highlighter assigns (the editor's CSS module provides them). */
export interface TokenClasses {
  readonly keyword: string;
  readonly string: string;
  readonly number: string;
  readonly comment: string;
  readonly type: string;
  readonly function: string;
  readonly operator: string;
  readonly punct: string;
}

export function makeHighlighter(c: TokenClasses) {
  return tagHighlighter([
    { tag: tags.keyword, class: c.keyword },
    { tag: [tags.string, tags.character], class: c.string },
    { tag: tags.number, class: c.number },
    { tag: tags.comment, class: c.comment },
    { tag: tags.typeName, class: c.type },
    { tag: tags.function(tags.variableName), class: c.function },
    { tag: tags.operator, class: c.operator },
    { tag: [tags.punctuation, tags.bracket], class: c.punct },
  ]);
}
