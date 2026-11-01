import { useEffect, useRef } from 'preact/hooks';
import { createEditor, offsetToPosition, positionToOffset } from '../editor.js';
import { getFile, project, runtime, setFileText, settings, useStore } from '../store.js';
import { lsp } from '../lsp.js';
import * as sync from '../sync.js';
import { applyWorkspaceEdit, editorRef, goToDefinition, startRename } from '../actions.js';

const SEVERITY = { 1: 'error', 2: 'warning', 3: 'info', 4: 'info' };

/** LSP diagnostics of one file as CodeMirror diagnostics (non-empty ranges, quick fixes attached). */
function toEditorDiagnostics(doc, list, actions) {
  return list.map((d, i) => {
    let from = positionToOffset(doc, d.range.start);
    let to = positionToOffset(doc, d.range.end);
    if (to <= from) to = Math.min(doc.length, from + 1);
    if (to <= from) from = Math.max(0, to - 1);
    return {
      from,
      to,
      severity: SEVERITY[d.severity] ?? 'error',
      message: d.code ? `${d.message}  [${d.code}]` : d.message,
      source: d.source,
      actions: (actions[i] ?? []).map((a) => ({
        name: a.title,
        apply: () => a.edit && applyWorkspaceEdit(a.edit),
      })),
    };
  });
}

export function Editor() {
  const host = useRef();
  const active = useStore(project, (p) => p.active);
  const text = useStore(project, (p) => getFile(p.active)?.text ?? '');
  const wrap = useStore(settings, (s) => s.wrap);
  const problems = useStore(runtime, (s) => s.problems[active]);
  const jump = useStore(runtime, (s) => s.jump);
  const edRef = useRef();
  const quickFixes = useRef({ key: '', actions: [] });

  useEffect(() => {
    const ed = createEditor(host.current, {
      lsp,
      flush: () => sync.flush(),
      onChange: (name, value) => {
        setFileText(name, value);
        sync.scheduleChange(name, value);
      },
      onCursor: (line, col) => runtime.set({ cursor: { line, col } }),
      onDefinition: goToDefinition,
      onRename: startRename,
    });
    edRef.current = ed;
    editorRef.current = ed;
    // semantic colors follow every change the server has seen
    const off = sync.onSynced(async (name) => {
      if (name !== ed.name) return;
      try {
        const tokens = await lsp.semanticTokens(name);
        if (tokens?.data && ed.name === name) ed.setSemantic(tokens.data, lsp.legend());
      } catch {
        /* the server restarted: the next change repaints */
      }
    });
    return () => {
      off();
      ed.destroy();
      editorRef.current = null;
    };
  }, []);

  useEffect(() => {
    // the store, not the (possibly stale) render's value: typing must never be overwritten by its own echo
    edRef.current.show(active, getFile(active)?.text ?? '');
  }, [active, text]);

  useEffect(() => {
    edRef.current.setWrap(wrap);
  }, [wrap]);

  // problems -> squiggles, with the quick fixes the server offers
  useEffect(() => {
    const ed = edRef.current;
    if (!ed || ed.name !== active) return;
    const doc = ed.view.state.doc;
    const list = problems ?? [];
    ed.setDiagnostics(toEditorDiagnostics(doc, list, quickFixes.current.key === JSON.stringify(list) ? quickFixes.current.actions : []));
    let cancelled = false;
    (async () => {
      const key = JSON.stringify(list);
      if (list.length === 0 || quickFixes.current.key === key) return;
      const actions = [];
      for (const d of list.slice(0, 12)) {
        try {
          const a = await lsp.codeActions(active, d.range, [d]);
          actions.push(Array.isArray(a) ? a.filter((x) => x.edit) : []);
        } catch {
          actions.push([]);
        }
      }
      if (cancelled || !actions.some((a) => a.length)) return;
      quickFixes.current = { key, actions };
      if (edRef.current?.name === active) edRef.current.setDiagnostics(toEditorDiagnostics(edRef.current.view.state.doc, list, actions));
    })();
    return () => {
      cancelled = true;
    };
  }, [problems, active]);

  useEffect(() => {
    if (jump && jump.file === project.get().active) {
      // the file may have just been switched: wait for the state to be shown
      setTimeout(() => edRef.current?.select(jump.from, jump.to), 0);
    }
  }, [jump]);

  return <div class="editor-host" ref={host} />;
}
