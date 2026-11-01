import { render } from 'preact';
import { App } from '@/app/App';
import { openProject } from '@/app/actions';
import { t } from '@/core/i18n';
import { lsp } from '@/core/lsp/client';
import { syncProject } from '@/core/lsp/sync';
import { project, runtime, ui } from '@/core/state/stores';
import { version, warmUp } from '@/core/toolchain/toolchain';
import { decodeProject } from '@/core/util/share';
import '@/styles/global.scss';

// problems published by the language server
lsp.onDiagnostics((file, diagnostics) => {
  // the server repeats what it found after every change: only a different answer is news
  const known = runtime.get().problems[file];
  if (known && JSON.stringify(known) === JSON.stringify(diagnostics)) return;
  runtime.set((r) => ({ problems: { ...r.problems, [file]: diagnostics } }));
});

async function boot(): Promise<void> {
  // on a phone the sidebar starts closed (it is an overlay there)
  if (!localStorage.getItem('neon-web:ui') && matchMedia('(max-width: 980px)').matches) ui.set({ sidebar: false });
  const root = document.getElementById('app');
  if (!root) throw new Error('#app is missing');
  render(<App />, root);
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
    .catch(() => undefined);
}

void boot();
