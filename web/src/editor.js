// The code editor: CodeMirror 6 with a Neon language, diagnostics, completion, hover and the semantic
// colors of the language server on top of the lexical ones.
import { EditorState, StateEffect, StateField, Compartment, Prec } from '@codemirror/state';
import {
  EditorView,
  keymap,
  lineNumbers,
  highlightActiveLine,
  highlightActiveLineGutter,
  drawSelection,
  dropCursor,
  rectangularSelection,
  hoverTooltip,
  Decoration,
} from '@codemirror/view';
import { defaultKeymap, history, historyKeymap, indentWithTab, toggleComment } from '@codemirror/commands';
import {
  StreamLanguage,
  syntaxHighlighting,
  indentService,
  indentUnit,
  bracketMatching,
  foldGutter,
  foldKeymap,
  codeFolding,
} from '@codemirror/language';
import { autocompletion, closeBrackets, closeBracketsKeymap, completionKeymap } from '@codemirror/autocomplete';
import { lintGutter, setDiagnostics, lintKeymap } from '@codemirror/lint';
import { searchKeymap, highlightSelectionMatches } from '@codemirror/search';
import { tags, tagHighlighter } from '@lezer/highlight';

// ---- the language -----------------------------------------------------------------------------------------

const KEYWORDS = new Set([
  'let', 'var', 'const', 'fn', 'return', 'if', 'else', 'elif', 'while', 'loop', 'for', 'in', 'break',
  'continue', 'match', 'enum', 'struct', 'trait', 'impl', 'import', 'export', 'as', 'true', 'false', 'self', 'Self',
]);
const PRIMITIVES = new Set(['i8', 'i16', 'i32', 'i64', 'u8', 'u16', 'u32', 'u64', 'f32', 'f64', 'bool', 'char', 'string']);
const OPERATORS = /^(?:->|=>|==|!=|<=|>=|&&|\|\||<<=?|>>=?|\.\.=?|[+\-*/%&|^]=?|[!<>=~?])/;

function scanString(stream, st) {
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

export const neonLanguage = StreamLanguage.define({
  name: 'neon',
  // str: inside a string ('plain' or 'f' for an f-string); interp: brace depth inside `{...}` of an f-string
  startState: () => ({ comment: 0, str: null, interp: 0 }),
  copyState: (s) => ({ ...s }),
  token(stream, st) {
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
      return this.token(stream, st);
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
  },
  languageData: {
    commentTokens: { line: '//', block: { open: '/*', close: '*/' } },
    closeBrackets: { brackets: ['(', '[', '{', '"'] },
  },
});

const highlighter = tagHighlighter([
  { tag: tags.keyword, class: 'tok-keyword' },
  { tag: [tags.string, tags.character], class: 'tok-string' },
  { tag: tags.number, class: 'tok-number' },
  { tag: tags.comment, class: 'tok-comment' },
  { tag: tags.typeName, class: 'tok-type' },
  { tag: tags.function(tags.variableName), class: 'tok-function' },
  { tag: tags.operator, class: 'tok-operator' },
  { tag: [tags.punctuation, tags.bracket], class: 'tok-punct' },
]);

// indentation: that of the line above, one level deeper after an opening bracket, one less for a closer
const indent = indentService.of((context, pos) => {
  const line = context.state.doc.lineAt(pos);
  const text = line.text;
  let prevNo = line.number - 1;
  while (prevNo > 0 && context.state.doc.line(prevNo).text.trim() === '') prevNo--;
  if (prevNo < 1) return 0;
  const prev = context.state.doc.line(prevNo).text;
  let n = prev.match(/^ */)[0].length;
  const trimmed = prev.trimEnd();
  if (/[{(\[]$/.test(trimmed)) n += 4;
  if (/^\s*[})\]]/.test(text)) n = Math.max(0, n - 4);
  return n;
});

// ---- semantic colors ----------------------------------------------------------------------------------------

const setSemantic = StateEffect.define();

const SEMANTIC_CLASS = {
  namespace: 'sem-type',
  type: 'sem-type',
  struct: 'sem-type',
  enum: 'sem-type',
  interface: 'sem-type',
  typeParameter: 'sem-type',
  parameter: 'sem-param',
  variable: 'sem-variable',
  property: 'sem-property',
  enumMember: 'sem-variant',
  function: 'sem-function',
  method: 'sem-function',
};

const semanticField = StateField.define({
  create: () => Decoration.none,
  update(value, tr) {
    value = value.map(tr.changes);
    for (const e of tr.effects) if (e.is(setSemantic)) value = e.value;
    return value;
  },
  provide: (f) => EditorView.decorations.from(f),
});

/** Decorations for the LSP's encoded semantic tokens (positions are line / UTF-16 column). */
function semanticDecorations(doc, data, legend) {
  const ranges = [];
  let line = 0;
  let col = 0;
  for (let i = 0; i + 4 < data.length; i += 5) {
    line += data[i];
    col = data[i] === 0 ? col + data[i + 1] : data[i + 1];
    const cls = SEMANTIC_CLASS[legend[data[i + 3]]];
    if (!cls || line + 1 > doc.lines) continue;
    const from = doc.line(line + 1).from + col;
    const to = from + data[i + 2];
    if (to <= doc.length) ranges.push(Decoration.mark({ class: cls }).range(from, to));
  }
  return Decoration.set(ranges, true);
}

// ---- positions -----------------------------------------------------------------------------------------------

export function offsetToPosition(doc, offset) {
  const line = doc.lineAt(offset);
  return { line: line.number - 1, character: offset - line.from };
}

export function positionToOffset(doc, pos) {
  if (pos.line + 1 > doc.lines) return doc.length;
  const line = doc.line(pos.line + 1);
  return Math.min(line.from + pos.character, line.to);
}

// ---- hover ----------------------------------------------------------------------------------------------------

function renderMarkdown(value) {
  const dom = document.createElement('div');
  dom.className = 'hover-doc';
  const parts = value.split(/```[a-z]*\n?/);
  parts.forEach((part, i) => {
    const text = part.replace(/\n+$/, '');
    if (!text.trim()) return;
    const el = document.createElement(i % 2 === 1 ? 'pre' : 'p');
    el.textContent = text.trim();
    dom.append(el);
  });
  return dom;
}

// ---- the editor ---------------------------------------------------------------------------------------------------

const wrapCompartment = new Compartment();

/**
 * Creates the editor in `parent`. `hooks`: {onChange(name, text), onCursor(line, col), lsp, flush(), onDefinition(),
 * onRename(), onRun(), onFormat()}.
 */
export function createEditor(parent, hooks) {
  let currentName = null;
  const states = new Map();

  const completionSource = async (ctx) => {
    const word = ctx.matchBefore(/[A-Za-z_\u0080-\uffff][A-Za-z0-9_\u0080-\uffff]*/);
    const before = ctx.state.sliceDoc(Math.max(0, ctx.pos - 2), ctx.pos);
    if (!word && !ctx.explicit && !before.endsWith('.') && !before.endsWith('::')) return null;
    await hooks.flush();
    let result;
    try {
      result = await hooks.lsp.completion(currentName, offsetToPosition(ctx.state.doc, ctx.pos));
    } catch {
      return null;
    }
    if (ctx.aborted) return null;
    const items = Array.isArray(result) ? result : (result?.items ?? []);
    const KIND = { 2: 'method', 3: 'function', 5: 'property', 6: 'variable', 7: 'class', 8: 'interface', 9: 'namespace', 13: 'enum', 14: 'keyword', 20: 'constant', 21: 'constant', 22: 'class' };
    return {
      from: word ? word.from : ctx.pos,
      options: items.map((it) => ({
        label: it.label,
        detail: it.detail && it.detail.length < 36 ? it.detail : undefined,
        info: it.detail && it.detail.length >= 36 ? it.detail : undefined,
        type: KIND[it.kind] ?? 'text',
        sortText: it.sortText,
        boost: it.sortText ? -(it.sortText.charCodeAt(0) - 48) : 0,
      })),
      validFor: /^[A-Za-z0-9_\u0080-\uffff]*$/,
    };
  };

  const hover = hoverTooltip(
    async (view, pos) => {
      if (!currentName) return null;
      await hooks.flush();
      let h;
      try {
        h = await hooks.lsp.hover(currentName, offsetToPosition(view.state.doc, pos));
      } catch {
        return null;
      }
      const value = h?.contents?.value ?? (typeof h?.contents === 'string' ? h.contents : null);
      if (!value) return null;
      let from = pos;
      let to = pos;
      if (h.range) {
        from = positionToOffset(view.state.doc, h.range.start);
        to = positionToOffset(view.state.doc, h.range.end);
      }
      return { pos: from, end: to, above: true, create: () => ({ dom: renderMarkdown(value) }) };
    },
    { hoverTime: 350 },
  );

  const extensions = [
    lineNumbers(),
    highlightActiveLineGutter(),
    foldGutter({ openText: '▾', closedText: '▸' }),
    codeFolding(),
    lintGutter(),
    history(),
    drawSelection(),
    dropCursor(),
    EditorState.allowMultipleSelections.of(true),
    rectangularSelection(),
    indentUnit.of('    '),
    indent,
    bracketMatching(),
    closeBrackets(),
    highlightActiveLine(),
    highlightSelectionMatches(),
    autocompletion({ override: [completionSource], icons: true, closeOnBlur: true, defaultKeymap: true }),
    neonLanguage,
    syntaxHighlighting(highlighter),
    semanticField,
    hover,
    wrapCompartment.of([]),
    Prec.highest(
      keymap.of([
        { key: 'F12', run: () => (hooks.onDefinition(), true) },
        { key: 'F2', run: () => (hooks.onRename(), true) },
        { key: 'Mod-/', run: toggleComment },
      ]),
    ),
    keymap.of([...closeBracketsKeymap, ...defaultKeymap, ...searchKeymap, ...historyKeymap, ...foldKeymap, ...completionKeymap, ...lintKeymap, indentWithTab]),
    EditorView.domEventHandlers({
      mousedown(e, view) {
        if ((e.ctrlKey || e.metaKey) && e.button === 0) {
          const pos = view.posAtCoords({ x: e.clientX, y: e.clientY });
          if (pos != null) {
            view.dispatch({ selection: { anchor: pos } });
            hooks.onDefinition();
            e.preventDefault();
            return true;
          }
        }
        return false;
      },
    }),
    EditorView.updateListener.of((u) => {
      if (u.docChanged && currentName) hooks.onChange(currentName, u.state.doc.toString());
      if (u.selectionSet || u.docChanged) {
        const head = u.state.selection.main.head;
        const line = u.state.doc.lineAt(head);
        hooks.onCursor(line.number, head - line.from + 1);
      }
    }),
    EditorView.contentAttributes.of({ 'aria-label': 'Редактор кода', spellcheck: 'false', autocorrect: 'off', autocapitalize: 'off' }),
  ];

  const view = new EditorView({ state: EditorState.create({ doc: '', extensions }), parent });

  const make = (text) => EditorState.create({ doc: text, extensions });

  return {
    view,
    get name() {
      return currentName;
    },
    /** Shows file `name`; its undo history and cursor are kept while it is open. */
    show(name, text) {
      if (currentName === name && view.state.doc.toString() === text) return;
      if (currentName) states.set(currentName, view.state);
      currentName = null; // no change events while the state is swapped
      let state = states.get(name);
      if (!state || state.doc.toString() !== text) state = make(text);
      view.setState(state);
      currentName = name;
      const head = view.state.selection.main.head;
      const line = view.state.doc.lineAt(head);
      hooks.onCursor(line.number, head - line.from + 1);
    },
    forget(name) {
      states.delete(name);
    },
    forgetAll() {
      states.clear();
    },
    focus: () => view.focus(),
    setWrap(on) {
      view.dispatch({ effects: wrapCompartment.reconfigure(on ? EditorView.lineWrapping : []) });
    },
    /** Replaces the text of the open file (formatting, rename, quick fixes) as one undoable step. */
    edit(changes) {
      view.dispatch({ changes, userEvent: 'input' });
    },
    select(from, to = from) {
      const len = view.state.doc.length;
      view.dispatch({ selection: { anchor: Math.min(from, len), head: Math.min(to, len) }, effects: EditorView.scrollIntoView(Math.min(from, len), { y: 'center' }) });
      view.focus();
    },
    setSemantic(data, legend) {
      if (!currentName) return;
      view.dispatch({ effects: setSemantic.of(semanticDecorations(view.state.doc, data, legend)) });
    },
    setDiagnostics(list) {
      view.dispatch(setDiagnostics(view.state, list));
    },
    // the start of a selection: after "jump to the name" the whole name is selected and its end is not on it
    cursorOffset: () => (view.state.selection.main.empty ? view.state.selection.main.head : view.state.selection.main.from),
    destroy() {
      view.destroy();
    },
  };
}
