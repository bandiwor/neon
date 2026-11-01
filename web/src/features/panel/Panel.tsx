import type { JSX } from 'preact';
import { Console } from '@/features/console';
import { Problems } from '@/features/problems';
import { Repl } from '@/features/repl';
import { ViewPane } from '@/features/views';
import { t, type MessageKey } from '@/core/i18n';
import { runtime, ui } from '@/core/state/stores';
import type { PanelTab } from '@/core/state/types';
import { useStore } from '@/core/state/useStore';
import { Tabs, type TabSpec } from '@/ui';
import s from './Panel.module.scss';

/** The tabs of the bottom panel: add one here to extend it (and a key in `PanelTab`). */
const BODIES: Readonly<Record<PanelTab, () => JSX.Element>> = {
  console: Console,
  problems: Problems,
  ir: () => <ViewPane kind="ir" />,
  bytecode: () => <ViewPane kind="bytecode" />,
  c: () => <ViewPane kind="c" />,
  repl: Repl,
};

const ORDER: readonly PanelTab[] = ['console', 'problems', 'ir', 'bytecode', 'c', 'repl'];

export function Panel(): JSX.Element {
  const tab = useStore(ui, (v) => v.panelTab);
  const problems = useStore(runtime, (r) => r.problems);
  const all = Object.values(problems).flat();
  const errors = all.filter((d) => (d.severity ?? 1) === 1).length;
  const warnings = all.filter((d) => d.severity === 2).length;
  const tabs: TabSpec<PanelTab>[] = ORDER.map((id) => ({
    id,
    label: t(`panel.${id}` satisfies MessageKey),
    badge: id === 'problems' && (errors > 0 || warnings > 0) ? { count: errors || warnings, kind: errors > 0 ? 'error' : 'warning' } : null,
  }));
  const Body = BODIES[tab];
  return (
    <section class={s.panel} aria-label={t('panel.label')}>
      <Tabs tabs={tabs} value={tab} onChange={(id) => ui.set({ panelTab: id })} />
      <div class={s.body}>
        <Body />
      </div>
    </section>
  );
}
