import type { JSX } from 'preact';
import { useRef } from 'preact/hooks';
import { t, type MessageKey } from '@/core/i18n';
import { freshName } from '@/core/project/files';
import { runtime, settings, ui } from '@/core/state/stores';
import { useStore } from '@/core/state/useStore';
import { Button, Menu, MenuItem, MenuSeparator, type IconName } from '@/ui';
import u from '@/styles/utils.module.scss';
import { checkProject, createFile, downloadActive, exportProjectJson, formatAll, formatFile, importFiles, restoreBackup, run, shareLink, stop } from './actions';
import { cycleTheme, openDialog, togglePanel, toggleSidebar } from './commands';
import { MOD } from './platform';
import s from './Header.module.scss';

const THEME_ICON: Readonly<Record<'system' | 'light' | 'dark', IconName>> = { system: 'monitor', light: 'sun', dark: 'moon' };

export function Header(): JSX.Element {
  const running = useStore(runtime, (r) => r.running);
  const theme = useStore(settings, (st) => st.theme);
  const sidebar = useStore(ui, (v) => v.sidebar);
  const panelOpen = useStore(ui, (v) => v.panelOpen);
  const picker = useRef<HTMLInputElement>(null);
  const label = (key: MessageKey, keys?: string): string => (keys ? `${t(key)} (${keys})` : t(key));
  return (
    <header class={s.header}>
      <a class={s.brand} href="./" title="Neon">
        <img src={`${import.meta.env.BASE_URL}favicon.svg`} alt="" width="22" height="22" />
        <span>Neon</span>
      </a>
      <div class={s.group}>
        {running ? (
          <Button icon="stop" danger onClick={() => void stop()} title={label('actions.stop', `${MOD}+.`)}>
            {t('actions.stop')}
          </Button>
        ) : (
          <Button icon="play" primary onClick={() => void run()} title={label('actions.run', `${MOD}+Enter`)}>
            {t('actions.run')}
          </Button>
        )}
        <Button icon="check" class={u.hideNarrow} onClick={() => void checkProject()} title={label('actions.check', `${MOD}+Shift+B`)}>
          {t('actions.check')}
        </Button>
        <Button icon="format" class={u.hideNarrow} onClick={() => void formatFile()} title={label('actions.format', `${MOD}+Shift+F`)}>
          {t('actions.format')}
        </Button>
        <Menu icon="file" label={t('menu.file')} title={t('menu.file')}>
          <MenuItem icon="plus" onClick={() => createFile(freshName('module'))}>
            {t('menu.new_file')}
          </MenuItem>
          <MenuItem icon="upload" onClick={() => picker.current?.click()}>
            {t('menu.import')}
          </MenuItem>
          <MenuItem icon="download" onClick={downloadActive}>
            {t('actions.download_file')}
          </MenuItem>
          <MenuItem icon="download" onClick={exportProjectJson}>
            {t('actions.export_project')}
          </MenuItem>
          <MenuSeparator />
          <MenuItem icon="format" onClick={() => void formatAll()}>
            {t('actions.format_all')}
          </MenuItem>
          <MenuItem icon="reset" onClick={restoreBackup}>
            {t('menu.restore')}
          </MenuItem>
        </Menu>
        <input
          ref={picker}
          type="file"
          accept=".neon,.json,text/plain"
          multiple
          hidden
          onChange={(e) => {
            const input = e.currentTarget;
            if (input.files) void importFiles(input.files);
            input.value = '';
          }}
        />
      </div>
      <span class={u.spacer} />
      <div class={s.group}>
        <Button icon="share" class={u.hideNarrow} onClick={() => void shareLink()} title={t('actions.share')}>
          {t('actions.share')}
        </Button>
        <Button icon="command" onClick={() => openDialog('palette')} title={label('palette.title', `${MOD}+K`)} />
        <Button icon="sidebar" active={sidebar} onClick={toggleSidebar} title={label('actions.toggle_sidebar', `${MOD}+B`)} />
        <Button icon="panel-bottom" active={panelOpen} onClick={togglePanel} title={label('actions.toggle_panel', `${MOD}+J`)} />
        <Button icon={THEME_ICON[theme]} onClick={cycleTheme} title={`${t('actions.cycle_theme')}: ${t(`theme.${theme}` satisfies MessageKey)}`} />
        <Button icon="gear" onClick={() => openDialog('settings')} title={t('actions.settings')} />
        <Button icon="help" class={u.hidePhone} onClick={() => openDialog('help')} title={label('actions.help', 'F1')} />
      </div>
    </header>
  );
}
