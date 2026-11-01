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
import { createEditor, toEditorDiagnostics, type EditorController } from './createEditor';
import s from './Editor.module.scss';

const CLASSES = {
  tokens: { keyword: s.tokKeyword, string: s.tokString, number: s.tokNumber, comment: s.tokComment, type: s.tokType, function: s.tokFunction, operator: s.tokOperator, punct: s.tokPunct },
  semantic: { type: s.semType, param: s.semParam, variable: s.semVariable, property: s.semProperty, variant: s.semVariant, function: s.semFunction },
  hover: s.hover,
} as const;

/** The editor of the open file. */
export function Editor(): JSX.Element {
  const host = useRef<HTMLDivElement>(null);
  const controller = useRef<EditorController | null>(null);
  const fixes = useRef<{ key: string; actions: readonly (readonly CodeAction[])[] }>({ key: '', actions: [] });
  const active = useStore(project, (p) => p.active);
  const text = useStore(project, (p) => getFile(p.active)?.text ?? '');
  const wrap = useStore(settings, (st) => st.wrap);
  const problems = useStore(runtime, (r) => r.problems[active]);
  const jump = useStore(runtime, (r) => r.jump);

  useEffect(() => {
    const parent = host.current;
    if (!parent) return undefined;
    const ed = createEditor(parent, {
      lsp,
      flush,
      onChange: (file, value) => {
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
    // semantic colors follow every change the server has seen
    const off = onSynced((file) => {
      if (file !== ed.name) return;
      lsp
        .semanticTokens(file)
        .then((tokens) => {
          if (tokens && ed.name === file) ed.setSemantic(tokens.data, lsp.legend());
        })
        .catch(() => undefined /* the server restarted: the next change repaints */);
    });
    return () => {
      off();
      ed.destroy();
      controller.current = null;
      editorPort.current = null;
    };
  }, []);

  useEffect(() => {
    // the store, not the (possibly stale) render's value: typing must never be overwritten by its own echo
    controller.current?.show(active, getFile(active)?.text ?? '');
  }, [active, text]);

  useEffect(() => controller.current?.setWrap(wrap), [wrap]);

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
