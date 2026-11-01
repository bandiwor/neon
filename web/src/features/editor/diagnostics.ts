import type { Diagnostic as EditorDiagnostic } from '@codemirror/lint';
import type { Text } from '@codemirror/state';
import type { CodeAction, Diagnostic, DiagnosticSeverity } from '@/core/lsp/types';
import { positionToOffset } from './positions';

const SEVERITY: Readonly<Record<DiagnosticSeverity, 'error' | 'warning' | 'info'>> = {
  1: 'error',
  2: 'warning',
  3: 'info',
  4: 'info',
};

/** LSP diagnostics of one file as CodeMirror diagnostics (non-empty ranges, quick fixes attached). */
export function toEditorDiagnostics(
  doc: Text,
  list: readonly Diagnostic[],
  fixes: readonly (readonly CodeAction[])[],
  apply: (action: CodeAction) => void,
): EditorDiagnostic[] {
  return list.map((d, i) => {
    let from = positionToOffset(doc, d.range.start);
    let to = positionToOffset(doc, d.range.end);
    if (to <= from) to = Math.min(doc.length, from + 1);
    if (to <= from) from = Math.max(0, to - 1);
    const out: EditorDiagnostic = {
      from,
      to,
      severity: SEVERITY[d.severity ?? 1],
      message: d.code === undefined ? d.message : `${d.message}  [${d.code}]`,
      actions: (fixes[i] ?? []).map((a) => ({ name: a.title, apply: () => apply(a) })),
    };
    return d.source === undefined ? out : { ...out, source: d.source };
  });
}
