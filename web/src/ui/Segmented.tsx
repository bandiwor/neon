import type { JSX } from 'preact';
import { Icon, type IconName } from './Icon';
import { cx } from './cx';
import s from './Segmented.module.scss';

export interface SegmentedOption<V extends string> {
  readonly value: V;
  readonly label?: string | undefined;
  readonly icon?: IconName | undefined;
  readonly title?: string | undefined;
}

interface SegmentedProps<V extends string> {
  readonly value: V;
  readonly options: readonly SegmentedOption<V>[];
  readonly onChange: (value: V) => void;
  readonly label: string;
}

/** Mutually exclusive choices drawn as one control. */
export function Segmented<V extends string>({ value, options, onChange, label }: SegmentedProps<V>): JSX.Element {
  return (
    <div class={s.segmented} role="radiogroup" aria-label={label}>
      {options.map((o) => (
        <button
          key={o.value}
          type="button"
          role="radio"
          aria-checked={value === o.value}
          class={cx(s.option, value === o.value && s.active)}
          onClick={() => onChange(o.value)}
          title={o.title}
        >
          {o.icon && <Icon name={o.icon} />}
          {o.label && <span>{o.label}</span>}
        </button>
      ))}
    </div>
  );
}
