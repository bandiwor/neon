import type { JSX } from 'preact';
import { t, type MessageKey } from '@/core/i18n';
import { project, runtime, ui } from '@/core/state/stores';
import { useStore } from '@/core/state/useStore';
import { Icon, cx } from '@/ui';
import u from '@/styles/utils.module.scss';
import s from './StatusBar.module.scss';

export function StatusBar(): JSX.Element {
  const cursor = useStore(runtime, (r) => r.cursor);
  const lsp = useStore(runtime, (r) => r.lsp);
  const problems = useStore(runtime, (r) => r.problems);
  const version = useStore(runtime, (r) => r.version);
  const active = useStore(project, (p) => p.active);
  const all = Object.values(problems).flat();
  const errors = all.filter((d) => (d.severity ?? 1) === 1).length;
  const warnings = all.filter((d) => d.severity === 2).length;
  const status = t(`lsp.${lsp}` satisfies MessageKey);
  return (
    <footer class={s.bar}>
      <span class={cx(s.lsp, lsp === 'ready' && s.ready, lsp === 'failed' && s.failed, lsp === 'starting' && s.starting)} title={status}>
        <span class={s.dot} />
        <span class={u.hideNarrow}>{status}</span>
      </span>
      <button type="button" class={s.problems} onClick={() => ui.set({ panelTab: 'problems', panelOpen: true })} title={t('panel.problems')}>
        <Icon name="x" size={12} class={errors > 0 ? u.errorText : undefined} /> {errors}
        <Icon name="alert" size={12} class={warnings > 0 ? u.warnText : undefined} /> {warnings}
      </button>
      <span class={u.spacer} />
      <span>{active}</span>
      <span>{t('status.cursor', { line: cursor.line, col: cursor.col })}</span>
      {version && <span class={u.hideNarrow}>Neon {version}</span>}
    </footer>
  );
}
