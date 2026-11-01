import type { JSX } from 'preact';
import type { Command } from '@/app/commands';
import { runtime } from '@/core/state/stores';
import { useStore } from '@/core/state/useStore';
import { HelpDialog } from './Help';
import { Palette } from './Palette';
import { RenameDialog } from './Rename';
import { SettingsDialog } from './Settings';
import { ShareDialog } from './Share';

/** Shows the open dialog, if any. A new dialog is a new `kind` in `DialogState` and a case here. */
export function Dialogs({ commands }: { readonly commands: readonly Command[] }): JSX.Element | null {
  const dialog = useStore(runtime, (r) => r.dialog);
  if (!dialog) return null;
  switch (dialog.kind) {
    case 'settings':
      return <SettingsDialog />;
    case 'help':
      return <HelpDialog />;
    case 'palette':
      return <Palette commands={commands} />;
    case 'rename':
      return <RenameDialog placeholder={dialog.placeholder} />;
    case 'share':
      return <ShareDialog url={dialog.url} />;
  }
}
