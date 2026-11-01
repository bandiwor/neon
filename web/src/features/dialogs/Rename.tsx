import type { JSX } from 'preact';
import { useState } from 'preact/hooks';
import { commitRename } from '@/app/actions';
import { t } from '@/core/i18n';
import { runtime } from '@/core/state/stores';
import { Button, Dialog, Field, FocusInput } from '@/ui';
import u from '@/styles/utils.module.scss';

export function RenameDialog({ placeholder }: { readonly placeholder: string }): JSX.Element {
  const [name, setName] = useState(placeholder);
  const valid = /^[A-Za-z_][A-Za-z0-9_]*$/.test(name);
  const close = (): void => runtime.set({ dialog: null });
  return (
    <Dialog
      title={t('rename.title')}
      onClose={close}
      footer={
        <>
          <span class={u.spacer} />
          <Button onClick={close}>{t('common.cancel')}</Button>
          <Button primary disabled={!valid} onClick={() => void commitRename(name)}>
            {t('rename.apply')}
          </Button>
        </>
      }
    >
      <form
        onSubmit={(e) => {
          e.preventDefault();
          if (valid) void commitRename(name);
        }}
      >
        <Field label={t('rename.new_name')} hint={t('rename.hint')}>
          <FocusInput value={name} onInput={(e) => setName(e.currentTarget.value)} spellcheck={false} />
        </Field>
      </form>
    </Dialog>
  );
}
