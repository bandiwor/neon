import { t } from '@/core/i18n';
import { lsp } from '@/core/lsp/client';
import { flush } from '@/core/lsp/sync';
import { getFile, filesMap } from '@/core/project/files';
import { project, ui } from '@/core/state/stores';
import { toast } from '@/core/state/toast';
import { format } from '@/core/toolchain/toolchain';
import { applyToFile } from './edits';

/** Formats one file; returns whether anything changed. */
export async function formatFile(name: string = project.get().active): Promise<boolean> {
  await flush();
  if (!getFile(name)) return false;
  try {
    const edits = await lsp.formatting(name);
    if (!edits || edits.length === 0) {
      toast(t('toast.formatted_already'));
      return false;
    }
    applyToFile(name, edits);
    toast(t('toast.formatted'), 'ok');
    return true;
  } catch (e) {
    // the server refuses code with syntax errors: ask the formatter itself for the reason
    const r = await format(filesMap(), name).catch(() => null);
    toast(r && !r.ok ? t('toast.format_failed') : e instanceof Error ? e.message : String(e), 'error');
    ui.set({ panelOpen: true, panelTab: 'problems' });
    return false;
  }
}

export async function formatAll(): Promise<void> {
  let n = 0;
  for (const f of project.get().files) if (await formatFile(f.name)) n++;
  toast(t('toast.formatted_n', { n }), 'ok');
}

