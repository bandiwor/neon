// The code editor: CodeMirror 6 with a Neon language, diagnostics, completion, hover and the semantic
// colors of the language server on top of the lexical ones.
import { autocompletion, closeBrackets, closeBracketsKeymap, completionKeymap } from '@codemirror/autocomplete';
import { defaultKeymap, history, historyKeymap, indentWithTab, insertNewlineAndIndent, toggleComment } from '@codemirror/commands';
import { bracketMatching, codeFolding, foldGutter, foldKeymap, indentOnInput, indentUnit, syntaxHighlighting } from '@codemirror/language';
import { lintGutter, lintKeymap, setDiagnostics, type Diagnostic as EditorDiagnostic } from '@codemirror/lint';
import { highlightSelectionMatches, searchKeymap } from '@codemirror/search';
import { Compartment, EditorState, Prec, type Extension } from '@codemirror/state';
import {
  drawSelection,
  dropCursor,
  EditorView,
  highlightActiveLine,
  highlightActiveLineGutter,
  hoverTooltip,
  keymap,
  lineNumbers,
  rectangularSelection,
  scrollPastEnd,
} from '@codemirror/view';
import type { Command } from '@codemirror/view';
import type { lsp as Lsp } from '@/core/lsp/client';
import type { EditorPort, TextChange } from '@/core/editor/port';
import type { Position } from '@/core/lsp/types';
import { completionSource, renderHover } from './completion';
import { toEditorDiagnostics } from './diagnostics';
import { neonIndent } from './indent';
import { makeHighlighter, neonLanguage, type TokenClasses } from './language';
import { offsetToPosition, positionToOffset } from './positions';
import { semanticDecorations, semanticField, setSemantic, type SemanticClasses } from './semantic';

export interface EditorHooks {
  readonly lsp: Pick<typeof Lsp, 'completion' | 'hover'>;
  /** Sends the pending edits to the language server. */
  flush(): Promise<void>;
  onChange(file: string, text: string): void;
  onCursor(line: number, col: number): void;
  onDefinition(): void;
  onRename(): void;
  readonly classes: {
    readonly tokens: TokenClasses;
    readonly semantic: SemanticClasses;
    readonly hover: string;
  };
}

/** The editor settings that can change while it is open. */
export interface EditorOptions {
  readonly wrap: boolean;
  readonly lineNumbers: boolean;
  readonly activeLine: boolean;
  readonly folding: boolean;
  readonly closeBrackets: boolean;
  readonly matchBrackets: boolean;
  readonly tabSize: number;
}

export interface EditorController extends EditorPort {
  readonly view: EditorView;
  /** Shows file `name`; its undo history and caret are kept while it is open. */
  show(name: string, text: string): void;
  focus(): void;
  /** Applies the settings (to the open document and to the ones opened later). */
  configure(options: EditorOptions): void;
  setSemantic(data: readonly number[], legend: readonly string[]): void;
  setDiagnostics(list: readonly EditorDiagnostic[]): void;
  /** Positions of the open document, for converting diagnostics. */
  destroy(): void;
}

const PAIRS: Readonly<Record<string, string>> = { '(': ')', '[': ']', '{': '}' };

/** Enter: between a bracket and its closer it opens a block (closer on its own line), else it keeps the indentation. */
const smartEnter: Command = (view) => {
  const { state } = view;
  const sel = state.selection.main;
  if (state.selection.ranges.length === 1 && sel.empty) {
    const before = state.sliceDoc(sel.head - 1, sel.head);
    if (PAIRS[before] !== undefined && PAIRS[before] === state.sliceDoc(sel.head, sel.head + 1)) {
      const base = /^[ \t]*/.exec(state.doc.lineAt(sel.head).text)?.[0] ?? '';
      const unit = state.facet(indentUnit);
      view.dispatch({
        changes: { from: sel.head, insert: `\n${base}${unit}\n${base}` },
        selection: { anchor: sel.head + 1 + base.length + unit.length },
        scrollIntoView: true,
        userEvent: 'input',
      });
      return true;
    }
  }
  return insertNewlineAndIndent(view);
};

// the compartments hold what the settings can change
const wrapC = new Compartment();
const gutterC = new Compartment();
const activeLineC = new Compartment();
const closeC = new Compartment();
const matchC = new Compartment();
const indentC = new Compartment();

function optionExtensions(o: EditorOptions): Record<'wrap' | 'gutter' | 'activeLine' | 'close' | 'match' | 'indent', Extension> {
  return {
    wrap: o.wrap ? EditorView.lineWrapping : [],
    gutter: [o.lineNumbers ? [lineNumbers(), highlightActiveLineGutter()] : [], o.folding ? [foldGutter({ openText: '▾', closedText: '▸' }), codeFolding()] : [], lintGutter()],
    activeLine: o.activeLine ? highlightActiveLine() : [],
    close: o.closeBrackets ? closeBrackets() : [],
    match: o.matchBrackets ? bracketMatching() : [],
    indent: [indentUnit.of(' '.repeat(o.tabSize)), EditorState.tabSize.of(o.tabSize)],
  };
}

const DEFAULT_OPTIONS: EditorOptions = { wrap: false, lineNumbers: true, activeLine: true, folding: true, closeBrackets: true, matchBrackets: true, tabSize: 4 };

function sameNumbers(a: readonly number[], b: readonly number[]): boolean {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
  return true;
}

export function createEditor(parent: HTMLElement, hooks: EditorHooks): EditorController {
  let current: string | null = null;
  let lastSemantic: { file: string | null; data: readonly number[] } = { file: null, data: [] };
  const states = new Map<string, EditorState>();

  const hover = hoverTooltip(
    async (view, pos) => {
      if (!current) return null;
      await hooks.flush();
      let h;
      try {
        h = await hooks.lsp.hover(current, offsetToPosition(view.state.doc, pos));
      } catch {
        return null;
      }
      const rendered = h && renderHover(h, hooks.classes.hover);
      if (!rendered) return null;
      const { from, to } = rendered.from(view.state.doc, pos);
      return { pos: from, end: to, above: true, create: () => ({ dom: rendered.dom }) };
    },
    { hoverTime: 350 },
  );

  let options = DEFAULT_OPTIONS;
  const initial = optionExtensions(options);

  const extensions: Extension[] = [
    gutterC.of(initial.gutter),
    history(),
    drawSelection(),
    dropCursor(),
    EditorState.allowMultipleSelections.of(true),
    rectangularSelection(),
    indentC.of(initial.indent),
    neonIndent,
    indentOnInput(),
    matchC.of(initial.match),
    closeC.of(initial.close),
    activeLineC.of(initial.activeLine),
    scrollPastEnd(),
    highlightSelectionMatches(),
    autocompletion({ override: [completionSource(hooks, () => current)], icons: true, closeOnBlur: true, activateOnTypingDelay: 80, maxRenderedOptions: 60 }),
    neonLanguage,
    syntaxHighlighting(makeHighlighter(hooks.classes.tokens)),
    semanticField,
    hover,
    wrapC.of(initial.wrap),
    Prec.highest(
      keymap.of([
        { key: 'F12', run: () => (hooks.onDefinition(), true) },
        { key: 'F2', run: () => (hooks.onRename(), true) },
        { key: 'Mod-/', run: toggleComment },
        { key: 'Enter', run: smartEnter },
      ]),
    ),
    keymap.of([
      ...closeBracketsKeymap,
      ...defaultKeymap,
      ...searchKeymap,
      ...historyKeymap,
      ...foldKeymap,
      ...completionKeymap,
      ...lintKeymap,
      indentWithTab,
    ]),
    EditorView.domEventHandlers({
      mousedown(e, view) {
        if ((e.ctrlKey || e.metaKey) && e.button === 0) {
          const pos = view.posAtCoords({ x: e.clientX, y: e.clientY });
          if (pos !== null) {
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
      if (u.docChanged && current) {
        lastSemantic = { file: null, data: [] };
        hooks.onChange(current, u.state.doc.toString());
      }
      if (u.selectionSet || u.docChanged) {
        const head = u.state.selection.main.head;
        const line = u.state.doc.lineAt(head);
        hooks.onCursor(line.number, head - line.from + 1);
      }
    }),
    EditorView.contentAttributes.of({ 'aria-label': 'Редактор кода', spellcheck: 'false', autocorrect: 'off', autocapitalize: 'off' }),
  ];

  const make = (text: string): EditorState => EditorState.create({ doc: text, extensions });
  const view = new EditorView({ state: make(''), parent });

  const applyOptions = (): void => {
    const o = optionExtensions(options);
    view.dispatch({
      effects: [wrapC.reconfigure(o.wrap), gutterC.reconfigure(o.gutter), activeLineC.reconfigure(o.activeLine), closeC.reconfigure(o.close), matchC.reconfigure(o.match), indentC.reconfigure(o.indent)],
    });
  };

  return {
    view,
    get name() {
      return current;
    },
    text: () => view.state.doc.toString(),
    // the start of a selection: after "jump to the name" the whole name is selected and its end is not on it
    caret: () => (view.state.selection.main.empty ? view.state.selection.main.head : view.state.selection.main.from),
    positionAt: (offset): Position => offsetToPosition(view.state.doc, offset),
    show(name, text) {
      if (current === name && view.state.doc.toString() === text) return;
      if (current) states.set(current, view.state);
      current = null; // no change events while the state is swapped
      let state = states.get(name);
      if (!state || state.doc.toString() !== text) state = make(text);
      view.setState(state);
      applyOptions(); // a stored state may have been made under other settings
      current = name;
      const head = view.state.selection.main.head;
      const line = view.state.doc.lineAt(head);
      hooks.onCursor(line.number, head - line.from + 1);
    },
    forget: (name) => void states.delete(name),
    forgetAll: () => states.clear(),
    focus: () => view.focus(),
    configure(next) {
      options = next;
      applyOptions();
    },
    edit(changes: readonly TextChange[]) {
      view.dispatch({ changes: changes.map((c) => ({ from: c.from, to: c.to, insert: c.insert })), userEvent: 'input' });
    },
    select(from, to = from) {
      const len = view.state.doc.length;
      const a = Math.min(from, len);
      view.dispatch({ selection: { anchor: a, head: Math.min(to, len) }, effects: EditorView.scrollIntoView(a, { y: 'center' }) });
      view.focus();
    },
    setSemantic(data, legend) {
      if (!current) return;
      // nothing new: leave the decorations (and the layout) alone
      if (lastSemantic.file === current && sameNumbers(lastSemantic.data, data)) return;
      lastSemantic = { file: current, data };
      view.dispatch({ effects: setSemantic.of(semanticDecorations(view.state.doc, data, legend, hooks.classes.semantic)) });
    },
    setDiagnostics(list) {
      view.dispatch(setDiagnostics(view.state, list));
    },
    destroy: () => view.destroy(),
  };
}

export { positionToOffset, toEditorDiagnostics };
