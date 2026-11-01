import type { JSX } from 'preact';
import { useEffect } from 'preact/hooks';
import { runtime } from '@/core/state/stores';
import { useStore } from '@/core/state/useStore';
import { cx } from '@/ui';
import s from './Toast.module.scss';

/** A short message at the bottom of the page; with an action it stays a little longer. */
export function Toast(): JSX.Element | null {
  const toast = useStore(runtime, (r) => r.toast);
  useEffect(() => {
    if (!toast) return undefined;
    const id = setTimeout(() => runtime.set({ toast: null }), toast.action ? 8000 : 3200);
    return () => clearTimeout(id);
  }, [toast]);
  if (!toast) return null;
  return (
    <div class={cx(s.toast, toast.kind === 'ok' && s.ok, toast.kind === 'error' && s.error, toast.kind === 'info' && s.info)} role="status">
      <span>{toast.text}</span>
      {toast.action && (
        <button
          type="button"
          class={s.action}
          onClick={() => {
            toast.action?.run();
            runtime.set({ toast: null });
          }}
        >
          {toast.action.label}
        </button>
      )}
    </div>
  );
}
