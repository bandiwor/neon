import type { ComponentChildren, JSX } from 'preact';
import { cx } from './cx';
import s from './Tabs.module.scss';

export interface TabSpec<Id extends string> {
  readonly id: Id;
  readonly label: ComponentChildren;
  readonly title?: string | undefined;
  /** A small number next to the label (the problems tab). */
  readonly badge?: { readonly count: number; readonly kind: 'error' | 'warning' } | null | undefined;
}

interface TabsProps<Id extends string> {
  readonly tabs: readonly TabSpec<Id>[];
  readonly value: Id;
  readonly onChange: (id: Id) => void;
  /** Show the label of the selected tab only (the sidebar). */
  readonly compact?: boolean | undefined;
  readonly label?: string | undefined;
}

/** A row of tabs. */
export function Tabs<Id extends string>({ tabs, value, onChange, compact, label }: TabsProps<Id>): JSX.Element {
  return (
    <div class={cx(s.tabs, compact && s.compact)} role="tablist" aria-label={label}>
      {tabs.map((t) => (
        <button
          key={t.id}
          type="button"
          role="tab"
          title={t.title}
          aria-selected={value === t.id}
          class={cx(s.tab, value === t.id && s.selected)}
          onClick={() => onChange(t.id)}
        >
          {t.label}
          {t.badge && <span class={cx(s.count, t.badge.kind === 'error' ? s.countError : s.countWarning)}>{t.badge.count}</span>}
        </button>
      ))}
    </div>
  );
}
