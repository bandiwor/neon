import type { JSX } from 'preact';
import { useState } from 'preact/hooks';
import { createFile, deleteFile, moveFile } from '@/app/actions';
import { t } from '@/core/i18n';
import { freshName, setActive } from '@/core/project/files';
import { project, runtime } from '@/core/state/stores';
import { useStore } from '@/core/state/useStore';
import { Badge, Button, ErrorDot, FocusInput, Icon, List, Row, RowMain, listClasses } from '@/ui';
import s from './Sidebar.module.scss';

export function FilesTab(): JSX.Element {
  const files = useStore(project, (p) => p.files);
  const active = useStore(project, (p) => p.active);
  const entry = useStore(project, (p) => p.entry);
  const problems = useStore(runtime, (r) => r.problems);
  const [renaming, setRenaming] = useState<string | null>(null);
  const [adding, setAdding] = useState<string | null>(null);

  const commitRename = (from: string, to: string): void => {
    setRenaming(null);
    if (to && to !== from) moveFile(from, to);
  };

  return (
    <div>
      <div class={s.head}>
        <span>{t('files.title')}</span>
        <Button icon="plus" title={t('files.new')} onClick={() => setAdding(freshName('module'))} />
      </div>
      <List>
        {files.map((f) => {
          const errors = (problems[f.name] ?? []).filter((d) => (d.severity ?? 1) === 1).length;
          return (
            <Row
              key={f.name}
              active={f.name === active}
              actions={
                <>
                  {f.name !== entry && <Button class={listClasses.smallAction} icon="flag" title={t('files.make_entry')} onClick={() => project.set({ entry: f.name })} />}
                  <Button class={listClasses.smallAction} icon="edit" title={t('files.rename')} onClick={() => setRenaming(f.name)} />
                  <Button
                    class={listClasses.smallAction}
                    icon="trash"
                    title={t('files.delete')}
                    onClick={() => confirm(t('files.confirm_delete', { name: f.name })) && deleteFile(f.name)}
                  />
                </>
              }
            >
              {renaming === f.name ? (
                <FocusInput
                  class={s.inline}
                  defaultValue={f.name}
                  onBlur={(e) => commitRename(f.name, e.currentTarget.value.trim())}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') e.currentTarget.blur();
                    if (e.key === 'Escape') setRenaming(null);
                  }}
                />
              ) : (
                <RowMain onClick={() => setActive(f.name)} onDblClick={() => setRenaming(f.name)} title={f.name}>
                  <Icon name="file" />
                  <span class={listClasses.name}>{f.name}</span>
                  {f.name === entry && <Badge title={t('files.entry')}>{t('files.entry_short')}</Badge>}
                  {errors > 0 && <ErrorDot title={t('problems.errors', { n: errors })} />}
                </RowMain>
              )}
            </Row>
          );
        })}
        {adding !== null && (
          <Row>
            <FocusInput
              class={s.inline}
              value={adding}
              onInput={(e) => setAdding(e.currentTarget.value)}
              onBlur={() => setAdding(null)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') {
                  createFile(adding.trim());
                  setAdding(null);
                }
                if (e.key === 'Escape') setAdding(null);
              }}
            />
          </Row>
        )}
      </List>
      <p class={s.hint}>{t('files.hint')}</p>
    </div>
  );
}
