import type { ComponentChildren, JSX } from 'preact';
import { useEffect, useRef } from 'preact/hooks';
import s from './Field.module.scss';

interface FieldProps {
  readonly label: string;
  readonly hint?: string | undefined;
  readonly children: ComponentChildren;
}

/** A labelled form control with an optional hint under it. */
export function Field({ label, hint, children }: FieldProps): JSX.Element {
  return (
    <label class={s.field}>
      <span class={s.label}>{label}</span>
      {children}
      {hint && <span class={s.hint}>{hint}</span>}
    </label>
  );
}

/** An input that takes the focus (and selects its text) when it appears. */
export function FocusInput(props: JSX.InputHTMLAttributes<HTMLInputElement>): JSX.Element {
  const ref = useRef<HTMLInputElement>(null);
  useEffect(() => {
    ref.current?.focus();
    ref.current?.select();
  }, []);
  return <input ref={ref} {...props} />;
}
