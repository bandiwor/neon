import { useEffect, useMemo, useRef, useState } from 'preact/hooks';
import { Button, Menu, MenuItem, MenuSeparator, Resizer } from './components/widgets.jsx';
import { Icon } from './components/icons.jsx';
import { Editor } from './components/editor-view.jsx';
import { Sidebar } from './components/sidebar.jsx';
import { Panel } from './components/panels.jsx';
import { Dialogs, MOD } from './components/dialogs.jsx';
import { project, runtime, settings, ui, useStore, setActive, freshName } from './store.js';
import { t, count } from './i18n.js';
import * as A from './actions.js';

// ---- the commands (header, palette and shortcuts share them) ----------------------------------------------------------

function useCommands() {
  return useMemo(
    () => [
      { id: 'run', title: t('actions.run'), hint: `${MOD}+Enter`, run: A.run },
      { id: 'stop', title: t('actions.stop'), hint: `${MOD}+.`, run: A.stop },
      { id: 'format', title: t('actions.format'), hint: `${MOD}+Shift+F`, run: () => A.formatFile() },
      { id: 'format-all', title: t('actions.format_all'), run: A.formatAll },
      { id: 'check', title: t('actions.check'), hint: `${MOD}+Shift+B`, run: A.checkProject },
      { id: 'definition', title: t('actions.definition'), hint: 'F12', run: A.goToDefinition },
      { id: 'rename', title: t('actions.rename'), hint: 'F2', run: A.startRename },
      { id: 'share', title: t('actions.share'), run: A.shareLink },
      { id: 'download', title: t('actions.download_file'), run: A.downloadActive },
      { id: 'export', title: t('actions.export_project'), run: A.exportProjectJson },
      { id: 'sidebar', title: t('actions.toggle_sidebar'), hint: `${MOD}+B`, run: () => ui.set({ sidebar: !ui.get().sidebar }) },
      { id: 'panel', title: t('actions.toggle_panel'), hint: `${MOD}+J`, run: () => ui.set({ panelOpen: ui.get().panelOpen === false }) },
      ...['console', 'problems', 'ir', 'bytecode', 'c', 'repl'].map((id) => ({
        id: `tab-${id}`,
        title: t('actions.show', { what: t(`panel.${id}`) }),
        run: () => ui.set({ panelTab: id, panelOpen: true }),
      })),
      ...['files', 'examples', 'outline'].map((id) => ({
        id: `side-${id}`,
        title: t('actions.show', { what: t(`sidebar.${id}`) }),
        run: () => ui.set({ sidebarTab: id, sidebar: true }),
      })),
      { id: 'theme', title: t('actions.cycle_theme'), run: cycleTheme },
      { id: 'settings', title: t('actions.settings'), run: () => runtime.set({ dialog: 'settings' }) },
      { id: 'help', title: t('actions.help'), hint: 'F1', run: () => runtime.set({ dialog: 'help' }) },
    ],
    [],
  );
}

const THEMES = ['system', 'light', 'dark'];
function cycleTheme() {
  const cur = settings.get().theme;
  settings.set({ theme: THEMES[(THEMES.indexOf(cur) + 1) % THEMES.length] });
}

// ---- the page --------------------------------------------------------------------------------------------------------------

export function App() {
  const s = useStore(settings);
  const sidebar = useStore(ui, (u) => u.sidebar);
  const panelOpen = useStore(ui, (u) => u.panelOpen !== false);
  const commands = useCommands();
  const work = useRef();
  const narrow = useNarrow();

  // theme, accent, font size
  useEffect(() => {
    document.documentElement.dataset.theme = s.theme;
    document.documentElement.dataset.accent = s.accent;
    document.documentElement.style.setProperty('--code-size', `${s.fontSize}px`);
    document.documentElement.lang = s.lang;
  }, [s.theme, s.accent, s.fontSize, s.lang]);

  useEffect(() => runtime.set({ commands }), [commands]);

  // shortcuts: caught before the editor sees them
  useEffect(() => {
    const onKey = (e) => {
      const mod = e.ctrlKey || e.metaKey;
      const key = e.key.toLowerCase();
      const hit = (fn) => (e.preventDefault(), e.stopPropagation(), fn());
      if (mod && e.key === 'Enter') return hit(A.run);
      if (mod && e.key === '.') return hit(A.stop);
      if (mod && e.shiftKey && key === 'f') return hit(() => A.formatFile());
      if (mod && e.shiftKey && key === 'b') return hit(A.checkProject);
      if (mod && !e.shiftKey && key === 'k') return hit(() => runtime.set({ dialog: 'palette' }));
      if (mod && e.shiftKey && key === 'p') return hit(() => runtime.set({ dialog: 'palette' }));
      if (mod && !e.shiftKey && key === 'b') return hit(() => ui.set({ sidebar: !ui.get().sidebar }));
      if (mod && !e.shiftKey && key === 'j') return hit(() => ui.set({ panelOpen: ui.get().panelOpen === false }));
      if (mod && !e.shiftKey && key === 's') return hit(() => A.formatFile());
      if (e.key === 'F1') return hit(() => runtime.set({ dialog: 'help' }));
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  }, []);

  const pos = s.panelPos === 'auto' ? (narrow ? 'bottom' : 'right') : s.panelPos;

  return (
    <div class={`app panel-${pos} ${sidebar ? '' : 'no-sidebar'} ${panelOpen ? '' : 'no-panel'}`}>
      <Header />
      <div class="main">
        {sidebar && <SidebarArea narrow={narrow} />}
        <div class="work" ref={work}>
          <EditorArea />
          {panelOpen && <PanelArea work={work} pos={pos} />}
        </div>
      </div>
      <StatusBar />
      <Dialogs />
      <Toast />
    </div>
  );
}

function useNarrow() {
  const [narrow, setNarrow] = useState(() => matchMedia('(max-width: 980px)').matches);
  useEffect(() => {
    const q = matchMedia('(max-width: 980px)');
    const on = () => setNarrow(q.matches);
    q.addEventListener('change', on);
    return () => q.removeEventListener('change', on);
  }, []);
  return narrow;
}

function SidebarArea({ narrow }) {
  const width = useStore(ui, (u) => u.sidebarWidth);
  const startW = useRef(width);
  return (
    <>
      <div class={`sidebar-area ${narrow ? 'overlay' : ''}`} style={narrow ? undefined : { width: `${width}px` }}>
        <Sidebar />
        {!narrow && (
          <Resizer
            axis="x"
            onDrag={(d) => ui.set({ sidebarWidth: Math.max(160, Math.min(480, startW.current + d)) })}
            onDone={() => (startW.current = ui.get().sidebarWidth)}
          />
        )}
      </div>
      {narrow && <div class="scrim" onClick={() => ui.set({ sidebar: false })} />}
    </>
  );
}

function EditorArea() {
  const files = useStore(project, (p) => p.files);
  const active = useStore(project, (p) => p.active);
  const entry = useStore(project, (p) => p.entry);
  const problems = useStore(runtime, (s) => s.problems);
  return (
    <div class="editor-area">
      <div class="filetabs" role="tablist" aria-label={t('files.title')}>
        {files.map((f) => {
          const errors = (problems[f.name] ?? []).filter((d) => d.severity === 1).length;
          return (
            <button type="button" role="tab" aria-selected={f.name === active} class={`filetab ${f.name === active ? 'is-active' : ''}`} onClick={() => setActive(f.name)} title={f.name === entry ? t('files.entry') : f.name}>
              <span>{f.name}</span>
              {errors > 0 && <span class="dot dot-error" />}
              {f.name === entry && <Icon name="flag" size={12} class="entry-flag" />}
            </button>
          );
        })}
      </div>
      <Editor />
    </div>
  );
}

function PanelArea({ work, pos }) {
  const size = useStore(ui, (u) => u.panelSize);
  const start = useRef(size);
  const style = pos === 'right' ? { width: `${size * 100}%` } : { height: `${size * 100}%` };
  return (
    <div class="panel-area" style={style}>
      <Resizer
        axis={pos === 'right' ? 'x' : 'y'}
        onDrag={(d) => {
          const box = work.current.getBoundingClientRect();
          const total = pos === 'right' ? box.width : box.height;
          const next = start.current - d / total;
          ui.set({ panelSize: Math.max(0.15, Math.min(0.8, next)) });
        }}
        onDone={() => (start.current = ui.get().panelSize)}
      />
      <Panel />
    </div>
  );
}

// ---- header ------------------------------------------------------------------------------------------------------------------

function Header() {
  const running = useStore(runtime, (s) => s.running);
  const theme = useStore(settings, (s) => s.theme);
  const sidebar = useStore(ui, (u) => u.sidebar);
  const panelOpen = useStore(ui, (u) => u.panelOpen !== false);
  const fileInput = useRef();
  return (
    <header class="header">
      <a class="brand" href="./" title="Neon">
        <img src="favicon.svg" alt="" width="22" height="22" />
        <span>Neon</span>
      </a>
      <div class="header-actions">
        {running ? (
          <Button icon="stop" onClick={A.stop} title={`${t('actions.stop')} (${MOD}+.)`} danger>
            {t('actions.stop')}
          </Button>
        ) : (
          <Button icon="play" primary onClick={A.run} title={`${t('actions.run')} (${MOD}+Enter)`}>
            {t('actions.run')}
          </Button>
        )}
        <Button icon="check" onClick={A.checkProject} title={`${t('actions.check')} (${MOD}+Shift+B)`} class="hide-narrow">
          {t('actions.check')}
        </Button>
        <Button icon="format" onClick={() => A.formatFile()} title={`${t('actions.format')} (${MOD}+Shift+F)`} class="hide-narrow">
          {t('actions.format')}
        </Button>
        <Menu icon="file" label={t('menu.file')} title={t('menu.file')}>
          <MenuItem icon="plus" onClick={() => A.createFile(freshName('module'))}>
            {t('menu.new_file')}
          </MenuItem>
          <MenuItem icon="upload" onClick={() => fileInput.current.click()}>
            {t('menu.import')}
          </MenuItem>
          <MenuItem icon="download" onClick={A.downloadActive}>
            {t('actions.download_file')}
          </MenuItem>
          <MenuItem icon="download" onClick={A.exportProjectJson}>
            {t('actions.export_project')}
          </MenuItem>
          <MenuSeparator />
          <MenuItem icon="format" onClick={A.formatAll}>
            {t('actions.format_all')}
          </MenuItem>
          <MenuItem icon="reset" onClick={A.restoreBackup}>
            {t('menu.restore')}
          </MenuItem>
        </Menu>
        <input ref={fileInput} type="file" accept=".neon,.json,text/plain" multiple hidden onChange={(e) => (A.importFiles(e.currentTarget.files), (e.currentTarget.value = ''))} />
      </div>
      <span class="spacer" />
      <div class="header-actions">
        <Button icon="share" onClick={A.shareLink} title={t('actions.share')} class="hide-narrow">
          {t('actions.share')}
        </Button>
        <Button icon="command" onClick={() => runtime.set({ dialog: 'palette' })} title={`${t('palette.title')} (${MOD}+K)`} />
        <Button icon="sidebar" active={sidebar} onClick={() => ui.set({ sidebar: !sidebar })} title={`${t('actions.toggle_sidebar')} (${MOD}+B)`} />
        <Button icon="panel-bottom" active={panelOpen} onClick={() => ui.set({ panelOpen: !panelOpen })} title={`${t('actions.toggle_panel')} (${MOD}+J)`} />
        <Button icon={theme === 'dark' ? 'moon' : theme === 'light' ? 'sun' : 'monitor'} onClick={cycleTheme} title={`${t('actions.cycle_theme')}: ${t(`theme.${theme}`)}`} />
        <Button icon="gear" onClick={() => runtime.set({ dialog: 'settings' })} title={t('actions.settings')} />
        <Button icon="help" onClick={() => runtime.set({ dialog: 'help' })} title={`${t('actions.help')} (F1)`} />
      </div>
    </header>
  );
}

// ---- status bar, toast --------------------------------------------------------------------------------------------------------

function StatusBar() {
  const cursor = useStore(runtime, (s) => s.cursor);
  const lsp = useStore(runtime, (s) => s.lsp);
  const problems = useStore(runtime, (s) => s.problems);
  const version = useStore(runtime, (s) => s.version);
  const active = useStore(project, (p) => p.active);
  const all = Object.values(problems).flat();
  const errors = all.filter((d) => d.severity === 1).length;
  const warnings = all.filter((d) => d.severity === 2).length;
  return (
    <footer class="statusbar">
      <span class={`lsp lsp-${lsp}`} title={t(`lsp.${lsp}`)}>
        <span class="status-dot" />
        <span class="hide-narrow">{t(`lsp.${lsp}`)}</span>
      </span>
      <button type="button" class="status-btn" onClick={() => ui.set({ panelTab: 'problems', panelOpen: true })} title={t('panel.problems')}>
        <Icon name="x" size={12} class={errors ? 'c-error' : ''} /> {errors}
        <Icon name="alert" size={12} class={warnings ? 'c-warn' : ''} /> {warnings}
      </button>
      <span class="spacer" />
      <span>{active}</span>
      <span>{t('status.cursor', { line: cursor.line, col: cursor.col })}</span>
      {version && <span class="hide-narrow">Neon {version}</span>}
    </footer>
  );
}

function Toast() {
  const toast = useStore(runtime, (s) => s.toast);
  useEffect(() => {
    if (!toast) return;
    const id = setTimeout(() => runtime.set({ toast: null }), toast.action ? 8000 : 3200);
    return () => clearTimeout(id);
  }, [toast]);
  if (!toast) return null;
  return (
    <div class={`toast toast-${toast.kind}`} role="status">
      <span>{toast.text}</span>
      {toast.action && (
        <button type="button" class="toast-action" onClick={() => (toast.action.run(), runtime.set({ toast: null }))}>
          {toast.action.label}
        </button>
      )}
    </div>
  );
}
