import type { JSX } from 'preact';
import { useEffect, useRef } from 'preact/hooks';
import { editorPort } from '@/core/editor/port';
import { lsp } from '@/core/lsp/client';
import { onSynced, flush, scheduleChange } from '@/core/lsp/sync';
import type { CodeAction } from '@/core/lsp/types';
import { getFile, setFileText } from '@/core/project/files';
import { project, runtime, settings } from '@/core/state/stores';
import { useStore } from '@/core/state/useStore';
import { goToDefinition, startRename } from '@/app/actions/navigation';
import { applyWorkspaceEdit } from '@/app/actions/edits';
import { createEditor, toEditorDiagnostics, type EditorController, type EditorOptions } from './createEditor';
import s from './Editor.module.scss';

const CLASSES = {
  tokens: { keyword: s.tokKeyword, string: s.tokString, number: s.tokNumber, comment: s.tokComment, type: s.tokType, function: s.tokFunction, operator: s.tokOperator, punct: s.tokPunct },
  semantic: { type: s.semType, param: s.semParam, variable: s.semVariable, property: s.semProperty, variant: s.semVariant, function: s.semFunction },
  hover: s.hover,
} as const;

/** How long typing must pause before the semantic colors are repainted. */
const SETTLE = 250;

/** The text the editor itself last reported: the store's echo of it needs no round trip. */
let lastTyped = '';
let lastEdit = 0;

const editorOptions = (): EditorOptions => {
  const { wrap, lineNumbers, activeLine, folding, closeBrackets, matchBrackets, tabSize } = settings.get();
  return { wrap, lineNumbers, activeLine, folding, closeBrackets, matchBrackets, tabSize };
};

/** The editor of the open file. */
export function Editor(): JSX.Element {
  const host = useRef<HTMLDivElement>(null);
  const controller = useRef<EditorController | null>(null);
  const fixes = useRef<{ key: string; actions: readonly (readonly CodeAction[])[] }>({ key: '', actions: [] });
  const active = useStore(project, (p) => p.active);
  const wrap = useStore(settings, (st) => st.wrap);
  const lineNumbers = useStore(settings, (st) => st.lineNumbers);
  const activeLine = useStore(settings, (st) => st.activeLine);
  const folding = useStore(settings, (st) => st.folding);
  const closeBrackets = useStore(settings, (st) => st.closeBrackets);
  const matchBrackets = useStore(settings, (st) => st.matchBrackets);
  const tabSize = useStore(settings, (st) => st.tabSize);
  const problems = useStore(runtime, (r) => r.problems[active]);
  const jump = useStore(runtime, (r) => r.jump);

  useEffect(() => {
    const parent = host.current;
    if (!parent) return undefined;
    const ed = createEditor(parent, {
      lsp,
      flush,
      onChange: (file, value) => {
        lastTyped = value;
        lastEdit = performance.now();
        setFileText(file, value);
        scheduleChange(file, value);
      },
      onCursor: (line, col) => runtime.set({ cursor: { line, col } }),
      onDefinition: () => void goToDefinition(),
      onRename: () => void startRename(),
      classes: CLASSES,
    });
    controller.current = ed;
    editorPort.current = ed;
    ed.configure(editorOptions());
    // semantic colors follow the changes the server has seen, once typing pauses (repainting while the
    // fingers move is what makes typing feel uneven)
    let repaint: ReturnType<typeof setTimeout> | undefined;
    const paint = (file: string): void => {
      lsp
        .semanticTokens(file)
        .then((tokens) => {
          if (tokens && ed.name === file) ed.setSemantic(tokens.data, lsp.legend());
        })
        .catch(() => undefined /* the server restarted: the next change repaints */);
    };
    const off = onSynced((file) => {
      if (file !== ed.name) return;
      clearTimeout(repaint);
      const quiet = performance.now() - lastEdit;
      repaint = setTimeout(() => paint(file), Math.max(0, SETTLE - quiet));
    });
    // changes that did not come from typing (format, rename, an example, undo of a file operation)
    const offProject = project.subscribe(() => {
      const name = project.get().active;
      const text = getFile(name)?.text ?? '';
      if (ed.name === name && text === lastTyped) return; // our own echo
      ed.show(name, text);
    });
    return () => {
      clearTimeout(repaint);
      offProject();
      off();
      ed.destroy();
      controller.current = null;
      editorPort.current = null;
    };
  }, []);

  useEffect(() => {
    // the store, not the (possibly stale) render's value: typing must never be overwritten by its own echo
    controller.current?.show(active, getFile(active)?.text ?? '');
  }, [active]);

  useEffect(() => controller.current?.configure(editorOptions()), [wrap, lineNumbers, activeLine, folding, closeBrackets, matchBrackets, tabSize]);

  // problems -> squiggles, with the quick fixes the server offers
  useEffect(() => {
    const ed = controller.current;
    if (!ed || ed.name !== active) return undefined;
    const list = problems ?? [];
    const key = JSON.stringify(list);
    const known = fixes.current.key === key ? fixes.current.actions : [];
    ed.setDiagnostics(toEditorDiagnostics(ed.view.state.doc, list, known, (a) => a.edit && applyWorkspaceEdit(a.edit)));
    if (list.length === 0 || fixes.current.key === key) return undefined;
    let cancelled = false;
    void (async () => {
      const actions: (readonly CodeAction[])[] = [];
      for (const d of list.slice(0, 12)) {
        try {
          const found = await lsp.codeActions(active, d.range, [d]);
          actions.push((found ?? []).filter((a) => a.edit));
        } catch {
          actions.push([]);
        }
      }
      if (cancelled || !actions.some((a) => a.length > 0)) return;
      fixes.current = { key, actions };
      const now = controller.current;
      if (now?.name === active) now.setDiagnostics(toEditorDiagnostics(now.view.state.doc, list, actions, (a) => a.edit && applyWorkspaceEdit(a.edit)));
    })();
    return () => {
      cancelled = true;
    };
  }, [problems, active]);

  useEffect(() => {
    if (!jump || jump.file !== project.get().active) return undefined;
    // the file may have just been switched: wait for its state to be shown
    const id = setTimeout(() => controller.current?.select(jump.from, jump.to), 0);
    return () => clearTimeout(id);
  }, [jump]);

  return <div class={s.host} ref={host} />;
}
