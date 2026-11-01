import type { JSX } from 'preact';
import { cx } from './cx';
import s from './Resizer.module.scss';

interface ResizerProps {
  readonly axis: 'x' | 'y';
  /** Where the splitter sits: `start` of the box it belongs to or its `end`. */
  readonly edge?: 'start' | 'end' | undefined;
  /** The distance dragged so far, in pixels (from where the drag began). */
  readonly onDrag: (delta: number) => void;
  readonly onDone?: (() => void) | undefined;
}

/** A drag handle on the edge of a box. */
export function Resizer({ axis, edge = 'start', onDrag, onDone }: ResizerProps): JSX.Element {
  const start = (e: PointerEvent): void => {
    e.preventDefault();
    const origin = axis === 'x' ? e.clientX : e.clientY;
    const move = (m: PointerEvent): void => onDrag((axis === 'x' ? m.clientX : m.clientY) - origin);
    const up = (): void => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
      delete document.body.dataset['resizing'];
      onDone?.();
    };
    document.body.dataset['resizing'] = axis;
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
  };
  return (
    <div
      class={cx(s.resizer, axis === 'x' ? s.x : s.y, edge === 'end' && s.end)}
      onPointerDown={start}
      role="separator"
      aria-orientation={axis === 'x' ? 'vertical' : 'horizontal'}
    />
  );
}
