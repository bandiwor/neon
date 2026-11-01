// Keeps the language server's copy of the documents in step with the project (changes are batched).
import { lsp } from './client';
import type { SourceFile } from '../project/types';

const dirty = new Map<string, string>();
const listeners = new Set<(file: string) => void>();
let timer: ReturnType<typeof setTimeout> | undefined;
let chain: Promise<void> = Promise.resolve();

/** Calls `listener(file)` after the server has seen a change of `file` (and published what it found). */
export function onSynced(listener: (file: string) => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function scheduleChange(file: string, text: string, delay = 160): void {
  dirty.set(file, text);
  clearTimeout(timer);
  timer = setTimeout(() => void flush(), delay);
}

/** Sends the pending changes now; resolves when the server has them. */
export function flush(): Promise<void> {
  clearTimeout(timer);
  const entries = [...dirty];
  dirty.clear();
  if (entries.length === 0) return chain;
  chain = chain
    .then(async () => {
      for (const [file, text] of entries) {
        await lsp.change(file, text);
        listeners.forEach((l) => l(file));
      }
    })
    .catch(() => undefined);
  return chain;
}

/** Opens, closes and updates documents so the server holds exactly `files`. */
export function syncProject(files: readonly SourceFile[]): Promise<void> {
  chain = chain
    .then(async () => {
      const names = new Set(files.map((f) => f.name));
      for (const f of files) {
        if (!lsp.has(f.name)) await lsp.open(f.name, f.text);
        else if (lsp.text(f.name) !== f.text) await lsp.change(f.name, f.text);
        listeners.forEach((l) => l(f.name));
      }
      // documents the project no longer has
      for (const file of lsp.files()) if (!names.has(file)) await lsp.close(file);
    })
    .catch(() => undefined);
  return chain;
}
