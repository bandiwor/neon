import type { JSX } from 'preact';
import { t } from '@/core/i18n';
import { ui } from '@/core/state/stores';
import type { SidebarTab } from '@/core/state/types';
import { useStore } from '@/core/state/useStore';
import { Icon, Tabs, type TabSpec } from '@/ui';
import { ExamplesTab } from './ExamplesTab';
import { FilesTab } from './FilesTab';
import { OutlineTab } from './OutlineTab';
import s from './Sidebar.module.scss';

/** The tabs of the sidebar: add one here (and a component) to extend it. */
const TABS: readonly { readonly id: SidebarTab; readonly icon: 'file' | 'book' | 'list'; readonly title: () => string; readonly body: () => JSX.Element }[] = [
  { id: 'files', icon: 'file', title: () => t('sidebar.files'), body: FilesTab },
  { id: 'examples', icon: 'book', title: () => t('sidebar.examples'), body: ExamplesTab },
  { id: 'outline', icon: 'list', title: () => t('sidebar.outline'), body: OutlineTab },
];

export function Sidebar(): JSX.Element {
  const tab = useStore(ui, (u) => u.sidebarTab);
  const specs: TabSpec<SidebarTab>[] = TABS.map((x) => ({
    id: x.id,
    title: x.title(),
    label: <Icon name={x.icon} />,
  }));
  const Body = (TABS.find((x) => x.id === tab) ?? TABS[0])?.body;
  return (
    <aside class={s.sidebar} aria-label={t('sidebar.label')}>
      <Tabs tabs={specs} value={tab} onChange={(id) => ui.set({ sidebarTab: id })} label={t('sidebar.label')} compact />
      <div class={s.body}>{Body && <Body />}</div>
    </aside>
  );
}
