import type { ComponentChildren, JSX } from 'preact';
import { useEffect, useRef } from 'preact/hooks';
import { Button } from './Button';
import { cx } from './cx';
import s from './Dialog.module.scss';

interface DialogProps {
  readonly title: string;
  readonly onClose: () => void;
  readonly wide?: boolean | undefined;
  readonly footer?: ComponentChildren | undefined;
  readonly children: ComponentChildren;
}

/** A modal dialog: Escape and a click on the backdrop close it; focus goes in and comes back. */
export function Dialog({ title, onClose, wide, footer, children }: DialogProps): JSX.Element {
  const box = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const before = document.activeElement;
    box.current?.querySelector<HTMLElement>('input, textarea, button')?.focus();
    const escape = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', escape);
    return () => {
      window.removeEventListener('keydown', escape);
      if (before instanceof HTMLElement) before.focus();
    };
  }, []);
  return (
    <div class={s.backdrop} onPointerDown={(e) => e.target === e.currentTarget && onClose()}>
      <div class={cx(s.dialog, wide && s.wide)} role="dialog" aria-modal="true" aria-label={title} ref={box}>
        <header class={s.head}>
          <h2 class={s.title}>{title}</h2>
          <Button icon="x" title="×" onClick={onClose} />
        </header>
        <div class={s.body}>{children}</div>
        {footer && <footer class={s.foot}>{footer}</footer>}
      </div>
    </div>
  );
}
