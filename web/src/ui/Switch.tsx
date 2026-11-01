import type { ComponentChildren, JSX } from 'preact';
import s from './Switch.module.scss';

interface SwitchProps {
  readonly checked: boolean;
  readonly onChange: (checked: boolean) => void;
  readonly children: ComponentChildren;
}

export function Switch({ checked, onChange, children }: SwitchProps): JSX.Element {
  return (
    <label class={s.switch}>
      <input type="checkbox" class={s.input} checked={checked} onChange={(e) => onChange(e.currentTarget.checked)} />
      <span class={s.box} aria-hidden="true" />
      <span>{children}</span>
    </label>
  );
}
