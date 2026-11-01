// The code editor: CodeMirror 6 with a Neon language, diagnostics, completion, hover and the semantic
// colors of the language server on top of the lexical ones.
import { autocompletion, closeBrackets, closeBracketsKeymap, completionKeymap } from '@codemirror/autocomplete';
import { defaultKeymap, history, historyKeymap, indentWithTab, toggleComment } from '@codemirror/commands';
import { bracketMatching, codeFolding, foldGutter, foldKeymap, indentUnit, syntaxHighlighting } from '@codemirror/language';
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
} from '@codemirror/view';
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

export interface EditorController extends EditorPort {
  readonly view: EditorView;
  /** Shows file `name`; its undo history and caret are kept while it is open. */
  show(name: string, text: string): void;
  focus(): void;
  setWrap(on: boolean): void;
  setSemantic(data: readonly number[], legend: readonly string[]): void;
  setDiagnostics(list: readonly EditorDiagnostic[]): void;
  /** Positions of the open document, for converting diagnostics. */
  destroy(): void;
}

const wrapCompartment = new Compartment();

export function createEditor(parent: HTMLElement, hooks: EditorHooks): EditorController {
  let current: string | null = null;
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

  const extensions: Extension[] = [
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
    neonIndent,
    bracketMatching(),
    closeBrackets(),
    highlightActiveLine(),
    highlightSelectionMatches(),
    autocompletion({ override: [completionSource(hooks, () => current)], icons: true, closeOnBlur: true }),
    neonLanguage,
    syntaxHighlighting(makeHighlighter(hooks.classes.tokens)),
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
      if (u.docChanged && current) hooks.onChange(current, u.state.doc.toString());
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
      current = name;
      const head = view.state.selection.main.head;
      const line = view.state.doc.lineAt(head);
      hooks.onCursor(line.number, head - line.from + 1);
    },
    forget: (name) => void states.delete(name),
    forgetAll: () => states.clear(),
    focus: () => view.focus(),
    setWrap(on) {
      view.dispatch({ effects: wrapCompartment.reconfigure(on ? EditorView.lineWrapping : []) });
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
      view.dispatch({ effects: setSemantic.of(semanticDecorations(view.state.doc, data, legend, hooks.classes.semantic)) });
    },
    setDiagnostics(list) {
      view.dispatch(setDiagnostics(view.state, list));
    },
    destroy: () => view.destroy(),
  };
}

export { positionToOffset, toEditorDiagnostics };
