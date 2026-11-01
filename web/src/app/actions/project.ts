import { count, t } from '@/core/i18n';
import { syncProject } from '@/core/lsp/sync';
import { editorPort } from '@/core/editor/port';
import { addFile, freshName, getFile, removeFile, renameFile, setProject, validFileName } from '@/core/project/files';
import type { Project, SourceFile } from '@/core/project/types';
import { project, runtime } from '@/core/state/stores';
import { toast } from '@/core/state/toast';
import { encodeProject } from '@/core/util/share';
import { resetInput } from './run';

const BACKUP_KEY = 'neon-web:backup';

export function createFile(name: string): void {
  if (!validFileName(name)) return toast(t('toast.bad_file_name'), 'error');
  if (getFile(name)) return toast(t('toast.file_exists'), 'error');
  addFile(name, '');
  void syncProject(project.get().files);
}

export function deleteFile(name: string): void {
  if (project.get().files.length <= 1) return toast(t('toast.last_file'), 'error');
  removeFile(name);
  editorPort.current?.forget(name);
  void syncProject(project.get().files);
}

export function moveFile(from: string, to: string): void {
  if (from === to) return;
  if (!validFileName(to)) return toast(t('toast.bad_file_name'), 'error');
  if (getFile(to)) return toast(t('toast.file_exists'), 'error');
  renameFile(from, to);
  editorPort.current?.forgetAll();
  void syncProject(project.get().files);
}

export function restoreBackup(): void {
  try {
    const raw = localStorage.getItem(BACKUP_KEY);
    const backup = raw ? (JSON.parse(raw) as Partial<Project>) : null;
    if (backup?.files && backup.files.length > 0 && backup.active && backup.entry) {
      project.set({ files: backup.files, active: backup.active, entry: backup.entry });
      editorPort.current?.forgetAll();
      void syncProject(project.get().files);
    }
  } catch {
    /* nothing to restore */
  }
}

/** Replaces the project (an example, a shared link, an import); the old one can be brought back. */
export function openProject(files: readonly SourceFile[], entry: string | undefined, title: string): void {
  try {
    localStorage.setItem(BACKUP_KEY, JSON.stringify(project.get()));
  } catch {
    /* no backup */
  }
  setProject(files, entry);
  editorPort.current?.forgetAll();
  resetInput();
  runtime.set({ run: null, problems: {} });
  void syncProject(project.get().files);
  toast(t('toast.opened', { title }), 'info', { label: t('toast.undo'), run: restoreBackup });
}

export async function shareLink(): Promise<void> {
  const p = project.get();
  const hash = await encodeProject(p.files, p.entry);
  const url = `${location.origin}${location.pathname}#${hash}`;
  try {
    await navigator.clipboard.writeText(url);
    toast(t('toast.link_copied'), 'ok');
  } catch {
    runtime.set({ dialog: { kind: 'share', url } });
  }
  history.replaceState(null, '', `#${hash}`);
}

export function download(name: string, text: string, type = 'text/plain'): void {
  const url = URL.createObjectURL(new Blob([text], { type }));
  const a = document.createElement('a');
  a.href = url;
  a.download = name;
  a.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

export function downloadActive(): void {
  const f = getFile(project.get().active);
  if (f) download(f.name.split('/').at(-1) ?? f.name, f.text);
}

export function exportProjectJson(): void {
  const p = project.get();
  download('neon-project.json', JSON.stringify({ files: p.files, entry: p.entry }, null, 2), 'application/json');
}

function isProjectJson(data: unknown): data is { files: SourceFile[]; entry?: string } {
  return (
    typeof data === 'object' &&
    data !== null &&
    Array.isArray((data as { files?: unknown }).files) &&
    (data as { files: unknown[] }).files.every(
      (f) => typeof f === 'object' && f !== null && typeof (f as SourceFile).name === 'string' && typeof (f as SourceFile).text === 'string',
    )
  );
}

export async function importFiles(list: FileList): Promise<void> {
  const files: SourceFile[] = [];
  for (const f of Array.from(list)) {
    const text = await f.text();
    if (f.name.endsWith('.json')) {
      try {
        const data: unknown = JSON.parse(text);
        if (isProjectJson(data) && data.files.length > 0) return openProject(data.files, data.entry, f.name);
      } catch {
        /* not a project: treat as a source file below */
      }
    }
    const name = f.name.endsWith('.neon') ? f.name : `${f.name}.neon`;
    files.push({ name: validFileName(name) ? name : freshName('imported'), text });
  }
  const first = files[0];
  if (first) openProject(files, first.name, count(files.length, ['файл', 'файла', 'файлов']));
}
