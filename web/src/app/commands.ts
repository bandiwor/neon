// The commands of the application: the palette lists them, the header and the shortcuts run them.
import { t, type MessageKey } from '@/core/i18n';
import { runtime, settings, ui } from '@/core/state/stores';
import type { PanelTab, SidebarTab, Theme } from '@/core/state/types';
import {
  checkProject,
  downloadActive,
  exportProjectJson,
  formatAll,
  formatFile,
  goToDefinition,
  run,
  shareLink,
  startRename,
  stop,
} from './actions';
import { MOD } from './platform';

export interface Command {
  readonly id: string;
  readonly title: string;
  readonly hint?: string;
  readonly run: () => void;
}

const THEMES: readonly Theme[] = ['system', 'light', 'dark'];

export function cycleTheme(): void {
  const cur = settings.get().theme;
  settings.set({ theme: THEMES[(THEMES.indexOf(cur) + 1) % THEMES.length] ?? 'system' });
}

export const toggleSidebar = (): void => ui.set({ sidebar: !ui.get().sidebar });
export const togglePanel = (): void => ui.set({ panelOpen: !ui.get().panelOpen });
export const openDialog = (kind: 'settings' | 'help' | 'palette'): void => runtime.set({ dialog: { kind } });

const PANELS: readonly PanelTab[] = ['console', 'problems', 'ir', 'bytecode', 'c', 'repl'];
const SIDEBARS: readonly SidebarTab[] = ['files', 'examples', 'outline'];

/** The commands, with their titles in the current language. */
export function commands(): Command[] {
  const cmd = (id: string, title: MessageKey, fn: () => void, hint?: string): Command => ({
    id,
    title: t(title),
    ...(hint === undefined ? {} : { hint }),
    run: fn,
  });
  return [
    cmd('run', 'actions.run', () => void run(), `${MOD}+Enter`),
    cmd('stop', 'actions.stop', () => void stop(), `${MOD}+.`),
    cmd('format', 'actions.format', () => void formatFile(), `${MOD}+Shift+F`),
    cmd('format-all', 'actions.format_all', () => void formatAll()),
    cmd('check', 'actions.check', () => void checkProject(), `${MOD}+Shift+B`),
    cmd('definition', 'actions.definition', () => void goToDefinition(), 'F12'),
    cmd('rename', 'actions.rename', () => void startRename(), 'F2'),
    cmd('share', 'actions.share', () => void shareLink()),
    cmd('download', 'actions.download_file', downloadActive),
    cmd('export', 'actions.export_project', exportProjectJson),
    cmd('sidebar', 'actions.toggle_sidebar', toggleSidebar, `${MOD}+B`),
    cmd('panel', 'actions.toggle_panel', togglePanel, `${MOD}+J`),
    ...PANELS.map((id) => ({
      id: `tab-${id}`,
      title: t('actions.show', { what: t(`panel.${id}`) }),
      run: () => ui.set({ panelTab: id, panelOpen: true }),
    })),
    ...SIDEBARS.map((id) => ({
      id: `side-${id}`,
      title: t('actions.show', { what: t(`sidebar.${id}`) }),
      run: () => ui.set({ sidebarTab: id, sidebar: true }),
    })),
    cmd('theme', 'settings.theme', cycleTheme),
    cmd('settings', 'actions.settings', () => openDialog('settings')),
    cmd('help', 'actions.help', () => openDialog('help'), 'F1'),
  ];
}
