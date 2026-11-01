import type { ComponentChildren, JSX } from 'preact';
import { cx } from './cx';
import s from './List.module.scss';

interface ListProps {
  readonly class?: string | undefined;
  readonly children: ComponentChildren;
}

export function List({ class: cls, children }: ListProps): JSX.Element {
  return (
    <ul class={cx(s.list, cls)} role="list">
      {children}
    </ul>
  );
}

interface RowProps {
  readonly active?: boolean | undefined;
  readonly class?: string | undefined;
  /** Buttons that appear when the row is hovered or focused. */
  readonly actions?: ComponentChildren | undefined;
  readonly children: ComponentChildren;
}

export function Row({ active, class: cls, actions, children }: RowProps): JSX.Element {
  return (
    <li class={cx(s.row, active && s.active, cls)}>
      {children}
      {actions && <span class={s.actions}>{actions}</span>}
    </li>
  );
}

interface RowMainProps extends Omit<JSX.ButtonHTMLAttributes<HTMLButtonElement>, 'class'> {
  readonly class?: string | undefined;
}

/** The main, clickable part of a row. */
export function RowMain({ class: cls, children, ...rest }: RowMainProps): JSX.Element {
  return (
    <button type="button" class={cx(s.main, cls)} {...rest}>
      {children}
    </button>
  );
}

export const listClasses = { name: s.name, sub: s.sub, badge: s.badge, badgeAccent: s.badgeAccent, dot: s.dot, smallAction: s.smallAction } as const;

export function Badge({ accent, title, children }: { readonly accent?: boolean | undefined; readonly title?: string | undefined; readonly children: ComponentChildren }): JSX.Element {
  return (
    <span class={cx(s.badge, accent && s.badgeAccent)} title={title}>
      {children}
    </span>
  );
}

export function ErrorDot({ title }: { readonly title?: string }): JSX.Element {
  return <span class={s.dot} title={title} />;
}
