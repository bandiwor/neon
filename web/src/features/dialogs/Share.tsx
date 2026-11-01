import type { JSX } from 'preact';
import { useEffect, useRef } from 'preact/hooks';
import { t } from '@/core/i18n';
import { runtime } from '@/core/state/stores';
import { Dialog } from '@/ui';
import u from '@/styles/utils.module.scss';

export function ShareDialog({ url }: { readonly url: string }): JSX.Element {
  const box = useRef<HTMLTextAreaElement>(null);
  useEffect(() => box.current?.select(), []);
  return (
    <Dialog title={t('share.title')} onClose={() => runtime.set({ dialog: null })}>
      <textarea class={u.mono} ref={box} readOnly rows={5} value={url} onFocus={(e) => e.currentTarget.select()} />
    </Dialog>
  );
}
