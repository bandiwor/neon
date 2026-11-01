import { render } from 'preact';
import { App } from './app.jsx';
import { project, runtime, settings, ui } from './store.js';
import { lsp, nameOf } from './lsp.js';
import { syncProject } from './sync.js';
import { decodeProject } from './share.js';
import { openProject } from './actions.js';
import { version, warmUp } from './toolchain.js';
import { t } from './i18n.js';

// problems published by the language server
lsp.onDiagnostics(({ uri, diagnostics }) => {
  runtime.set((s) => ({ problems: { ...s.problems, [nameOf(uri)]: diagnostics } }));
});

async function boot() {
  // on a phone the sidebar starts closed (it is an overlay there)
  if (!localStorage.getItem('neon-web:ui') && matchMedia('(max-width: 980px)').matches) ui.set({ sidebar: false });
  render(<App />, document.getElementById('app'));
  warmUp();
  // a shared link replaces the project
  const shared = await decodeProject(location.hash);
  if (shared) openProject(shared.files, shared.entry, t('share.opened_title'));
  try {
    await lsp.start();
    await syncProject(project.get().files);
    runtime.set({ lsp: 'ready' });
  } catch (e) {
    console.error(e);
    runtime.set({ lsp: 'failed' });
  }
  version()
    .then((v) => runtime.set({ version: v }))
    .catch(() => {});
}

boot();
if (settings.get().lang !== 'ru') settings.set({ lang: 'ru' });
