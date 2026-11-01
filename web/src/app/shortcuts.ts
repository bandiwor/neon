// Global keys. They are caught before the editor sees them, so they work wherever the focus is.
import { checkProject, formatFile, run, stop } from './actions';
import { openDialog, togglePanel, toggleSidebar } from './commands';

interface Shortcut {
  /** `e.key`, lower-cased. */
  readonly key: string;
  readonly mod?: boolean;
  readonly shift?: boolean;
  readonly run: () => void;
}

const SHORTCUTS: readonly Shortcut[] = [
  { key: 'enter', mod: true, run: () => void run() },
  { key: '.', mod: true, run: () => void stop() },
  { key: 'f', mod: true, shift: true, run: () => void formatFile() },
  { key: 's', mod: true, run: () => void formatFile() },
  { key: 'b', mod: true, shift: true, run: () => void checkProject() },
  { key: 'k', mod: true, run: () => openDialog('palette') },
  { key: 'p', mod: true, shift: true, run: () => openDialog('palette') },
  { key: 'b', mod: true, run: toggleSidebar },
  { key: 'j', mod: true, run: togglePanel },
  { key: 'f1', run: () => openDialog('help') },
];

/** Installs the shortcuts; returns a function that removes them. */
export function installShortcuts(): () => void {
  const onKey = (e: KeyboardEvent): void => {
    const mod = e.ctrlKey || e.metaKey;
    const key = e.key.toLowerCase();
    const hit = SHORTCUTS.find((s) => s.key === key && !!s.mod === mod && !!s.shift === e.shiftKey);
    if (!hit) return;
    e.preventDefault();
    e.stopPropagation();
    hit.run();
  };
  window.addEventListener('keydown', onKey, true);
  return () => window.removeEventListener('keydown', onKey, true);
}
