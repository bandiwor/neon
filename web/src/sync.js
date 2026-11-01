// Keeps the language server's copy of the documents in step with the project (changes are batched).
import { lsp } from './lsp.js';

const dirty = new Map();
const listeners = new Set();
let timer = 0;
let chain = Promise.resolve();

/** Calls `listener(name)` after the server has seen a change of `name` (and published what it found). */
export function onSynced(listener) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function scheduleChange(name, text, delay = 160) {
  dirty.set(name, text);
  clearTimeout(timer);
  timer = setTimeout(flush, delay);
}

/** Sends the pending changes now; resolves when the server has them. */
export function flush() {
  clearTimeout(timer);
  const entries = [...dirty];
  dirty.clear();
  if (entries.length === 0) return chain;
  chain = chain
    .then(async () => {
      for (const [name, text] of entries) {
        await lsp.change(name, text);
        listeners.forEach((l) => l(name));
      }
    })
    .catch(() => {});
  return chain;
}

/** Opens, closes and updates documents so the server holds exactly `files`. */
export function syncProject(files) {
  chain = chain
    .then(async () => {
      const names = new Set(files.map((f) => f.name));
      for (const f of files) {
        if (!lsp.has(f.name)) await lsp.open(f.name, f.text);
        else if (lsp.text(f.name) !== f.text) await lsp.change(f.name, f.text);
        listeners.forEach((l) => l(f.name));
      }
      for (const f of [...files]) void f;
      // documents the project no longer has
      for (const uri of [...knownNames()]) if (!names.has(uri)) await lsp.close(uri);
    })
    .catch(() => {});
  return chain;
}

function knownNames() {
  return lsp.names();
}
