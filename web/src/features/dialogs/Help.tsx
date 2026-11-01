import type { JSX } from 'preact';
import { MOD } from '@/app/platform';
import { t, type MessageKey } from '@/core/i18n';
import { runtime } from '@/core/state/stores';
import { Dialog } from '@/ui';
import s from './Help.module.scss';

const SHORTCUTS: readonly (readonly [id: 'run' | 'stop' | 'format' | 'check' | 'palette' | 'sidebar' | 'panel' | 'complete' | 'definition' | 'rename' | 'find' | 'comment' | 'help', keys: string])[] = [
  ['run', `${MOD}+Enter`],
  ['stop', `${MOD}+.`],
  ['format', `${MOD}+Shift+F`],
  ['check', `${MOD}+Shift+B`],
  ['palette', `${MOD}+K`],
  ['sidebar', `${MOD}+B`],
  ['panel', `${MOD}+J`],
  ['complete', `${MOD}+Space`],
  ['definition', `F12 / ${MOD}+клик`],
  ['rename', 'F2'],
  ['find', `${MOD}+F`],
  ['comment', `${MOD}+/`],
  ['help', 'F1'],
];

const PANEL_HELP: readonly MessageKey[] = ['help.p_console', 'help.p_problems', 'help.p_ir', 'help.p_bytecode', 'help.p_c', 'help.p_repl'];

export function HelpDialog(): JSX.Element {
  return (
    <Dialog title={t('help.title')} onClose={() => runtime.set({ dialog: null })} wide>
      <div class={s.help}>
        <p>{t('help.intro')}</p>
        <h3>{t('help.shortcuts')}</h3>
        <dl class={s.shortcuts}>
          {SHORTCUTS.map(([id, keys]) => (
            <>
              <dt key={`t-${id}`}>{t(`shortcut.${id}` satisfies MessageKey)}</dt>
              <dd key={`d-${id}`}>
                <kbd>{keys}</kbd>
              </dd>
            </>
          ))}
        </dl>
        <h3>{t('help.panels')}</h3>
        <ul>
          {PANEL_HELP.map((k) => (
            <li key={k}>{t(k)}</li>
          ))}
        </ul>
        <h3>{t('help.about')}</h3>
        <p>{t('help.about_text')}</p>
      </div>
    </Dialog>
  );
}
