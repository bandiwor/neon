import type { ComponentChildren, JSX } from 'preact';
import { useEffect, useRef, useState } from 'preact/hooks';
import { Button } from './Button';
import { Icon, type IconName } from './Icon';
import s from './Menu.module.scss';

interface MenuProps {
  readonly label?: string | undefined;
  readonly icon?: IconName | undefined;
  readonly title: string;
  readonly align?: 'left' | 'right' | undefined;
  readonly children: ComponentChildren;
}

/** A dropdown that opens under its button and closes on an outside click or Escape. */
export function Menu({ label, icon, title, children, align = 'left' }: MenuProps): JSX.Element {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return undefined;
    const outside = (e: PointerEvent): void => {
      if (e.target instanceof Node && !root.current?.contains(e.target)) setOpen(false);
    };
    const escape = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') setOpen(false);
    };
    window.addEventListener('pointerdown', outside);
    window.addEventListener('keydown', escape);
    return () => {
      window.removeEventListener('pointerdown', outside);
      window.removeEventListener('keydown', escape);
    };
  }, [open]);
  return (
    <div class={s.menu} ref={root}>
      <Button {...(icon ? { icon } : {})} title={title} onClick={() => setOpen(!open)} active={open}>
        {label}
      </Button>
      {open && (
        <div class={`${s.pop} ${align === 'left' ? s.left : s.right}`} role="menu" onClick={() => setOpen(false)}>
          {children}
        </div>
      )}
    </div>
  );
}

interface MenuItemProps {
  readonly icon?: IconName | undefined;
  readonly hint?: string | undefined;
  readonly disabled?: boolean | undefined;
  readonly onClick: () => void;
  readonly children: ComponentChildren;
}

export function MenuItem({ icon, children, hint, onClick, disabled }: MenuItemProps): JSX.Element {
  return (
    <button type="button" role="menuitem" class={s.item} onClick={onClick} disabled={disabled ?? false}>
      <span class={s.itemIcon}>{icon && <Icon name={icon} />}</span>
      <span class={s.itemLabel}>{children}</span>
      {hint && <kbd>{hint}</kbd>}
    </button>
  );
}

export function MenuSeparator(): JSX.Element {
  return <div class={s.sep} role="separator" />;
}
