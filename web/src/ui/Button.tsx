import type { ComponentChildren, JSX } from 'preact';
import { Icon, type IconName } from './Icon';
import { cx } from './cx';
import s from './Button.module.scss';

export interface ButtonProps extends Omit<JSX.ButtonHTMLAttributes<HTMLButtonElement>, 'icon' | 'class' | 'size' | 'title'> {
  readonly icon?: IconName | undefined;
  readonly primary?: boolean | undefined;
  readonly danger?: boolean | undefined;
  /** Pressed / selected. */
  readonly active?: boolean | undefined;
  /** Also the accessible name when there is no label. */
  readonly title?: string | undefined;
  readonly class?: string | undefined;
  readonly children?: ComponentChildren;
}

/** A button with an icon and/or a label. */
export function Button({ icon, children, primary, danger, active, title, class: cls, ...rest }: ButtonProps): JSX.Element {
  const hasLabel = children !== undefined && children !== null && children !== false;
  return (
    <button
      type="button"
      class={cx(s.btn, primary && s.primary, danger && s.danger, active && s.active, !hasLabel && s.iconOnly, cls)}
      title={title}
      aria-label={hasLabel ? undefined : title}
      aria-pressed={active === undefined ? undefined : active}
      {...rest}
    >
      {icon && <Icon name={icon} />}
      {hasLabel && <span>{children}</span>}
    </button>
  );
}
