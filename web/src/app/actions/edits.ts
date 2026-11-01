// Applying LSP edits to the project: the open file goes through the editor (so undo works), the others
// are rewritten in the store.
import { editorPort, type TextChange } from '@/core/editor/port';
import { fileOfUri, offsetIn } from '@/core/lsp/positions';
import { scheduleChange } from '@/core/lsp/sync';
import type { TextEdit, WorkspaceEdit } from '@/core/lsp/types';
import { getFile, setFileText } from '@/core/project/files';

/** Applies LSP text edits (positions in the original text) to `text`. */
export function applyEdits(text: string, edits: readonly TextEdit[]): string {
  const spans = edits
    .map((e) => ({ from: offsetIn(text, e.range.start), to: offsetIn(text, e.range.end), insert: e.newText }))
    .sort((a, b) => b.from - a.from);
  let out = text;
  for (const s of spans) out = out.slice(0, s.from) + s.insert + out.slice(s.to);
  return out;
}

export function editsToChanges(text: string, edits: readonly TextEdit[]): TextChange[] {
  return edits.map((e) => ({ from: offsetIn(text, e.range.start), to: offsetIn(text, e.range.end), insert: e.newText }));
}

/** Applies the edits to file `name`; returns whether that file exists. */
export function applyToFile(name: string, edits: readonly TextEdit[]): boolean {
  const file = getFile(name);
  if (!file) return false;
  const port = editorPort.current;
  if (port?.name === name) {
    port.edit(editsToChanges(port.text(), edits));
  } else {
    const text = applyEdits(file.text, edits);
    setFileText(name, text);
    port?.forget(name);
    scheduleChange(name, text);
  }
  return true;
}

/** Applies a WorkspaceEdit to the project; returns how many edits were made. */
export function applyWorkspaceEdit(edit: WorkspaceEdit): number {
  const byUri = new Map<string, readonly TextEdit[]>(Object.entries(edit.changes ?? {}));
  for (const dc of edit.documentChanges ?? []) byUri.set(dc.textDocument.uri, dc.edits);
  let n = 0;
  for (const [uri, edits] of byUri) if (applyToFile(fileOfUri(uri), edits)) n += edits.length;
  return n;
}
