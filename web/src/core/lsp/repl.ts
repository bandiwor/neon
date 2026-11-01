// The REPL session lives in the same module instance as the language server.
import { languageEngine } from './client';

export const repl = {
  eval: (cell: string, stdin = '') => languageEngine.call('repl_eval', { cell, stdin }),
  reset: () => languageEngine.call('repl_reset', {}),
  cells: () => languageEngine.call('repl_cells', {}),
  more: (text: string) => languageEngine.call('repl_more', { text }),
  complete: (buffer: string, cursor: number) => languageEngine.call('repl_complete', { buffer, cursor }),
  preview: (cell: string) => languageEngine.call('repl_preview', { cell }),
  highlight: (text: string) => languageEngine.call('repl_highlight', { text }),
} as const;
