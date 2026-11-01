import { editorPort } from '@/core/editor/port';
import { t } from '@/core/i18n';
import { fileOfUri, offsetIn } from '@/core/lsp/positions';
import { lsp } from '@/core/lsp/client';
import { flush } from '@/core/lsp/sync';
import type { Location, LocationLink, Range } from '@/core/lsp/types';
import { getFile, setActive } from '@/core/project/files';
import { project, runtime } from '@/core/state/stores';
import { toast } from '@/core/state/toast';
import { applyWorkspaceEdit } from './edits';

/** Shows a place of a file in the editor. */
export function jumpTo(file: string, range: Range): void {
  const f = getFile(file);
  if (!f) return;
  setActive(file);
  runtime.set({
    jump: { file, from: offsetIn(f.text, range.start), to: offsetIn(f.text, range.end), nonce: Math.random() },
  });
}

/** Jumps to a `file:line:col` seen in the console. */
export function jumpToPlace(file: string, line: number, col: number): void {
  const target = getFile(file) ? file : project.get().entry;
  const at = { line: line - 1, character: col - 1 };
  jumpTo(target, { start: at, end: at });
}

function firstLocation(found: Location | readonly (Location | LocationLink)[] | null): { uri: string; range: Range } | null {
  const loc = Array.isArray(found) ? found[0] : found;
  if (!loc) return null;
  if ('targetUri' in loc) return { uri: loc.targetUri, range: loc.targetSelectionRange };
  return { uri: loc.uri, range: loc.range };
}

export async function goToDefinition(): Promise<void> {
  const ed = editorPort.current;
  if (!ed?.name) return;
  await flush();
  try {
    const loc = firstLocation(await lsp.definition(ed.name, ed.positionAt(ed.caret())));
    if (!loc) return toast(t('toast.no_definition'));
    const file = fileOfUri(loc.uri);
    if (!getFile(file)) return toast(t('toast.definition_in_library'));
    jumpTo(file, loc.range);
  } catch (e) {
    toast(e instanceof Error ? e.message : String(e), 'error');
  }
}

export async function startRename(): Promise<void> {
  const ed = editorPort.current;
  if (!ed?.name) return;
  await flush();
  try {
    const position = ed.positionAt(ed.caret());
    const found = await lsp.prepareRename(ed.name, position);
    if (!found) return toast(t('toast.cannot_rename'));
    const range = 'start' in found ? found : found.range;
    const placeholder =
      !('start' in found) && found.placeholder !== undefined
        ? found.placeholder
        : range
          ? ed.text().slice(offsetIn(ed.text(), range.start), offsetIn(ed.text(), range.end))
          : '';
    runtime.set({ dialog: { kind: 'rename', position, placeholder } });
  } catch {
    toast(t('toast.cannot_rename'));
  }
}

export async function commitRename(newName: string): Promise<void> {
  const dialog = runtime.get().dialog;
  const ed = editorPort.current;
  if (dialog?.kind !== 'rename' || !ed?.name) return;
  try {
    const edit = await lsp.rename(ed.name, dialog.position, newName);
    toast(t('toast.renamed', { n: edit ? applyWorkspaceEdit(edit) : 0 }), 'ok');
  } catch (e) {
    toast(e instanceof Error ? e.message : String(e), 'error');
  }
  runtime.set({ dialog: null });
}
