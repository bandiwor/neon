import type { JSX } from 'preact';
import { useEffect, useState } from 'preact/hooks';
import { openProject } from '@/app/actions';
import { t } from '@/core/i18n';
import type { Example } from '@/core/project/types';
import { Badge, List, Row, RowMain, listClasses } from '@/ui';
import s from './Sidebar.module.scss';

/** The example programs, in groups; they are loaded on demand (a chunk of their own). */
export function ExamplesTab(): JSX.Element {
  const [list, setList] = useState<readonly Example[] | null>(null);
  useEffect(() => {
    void import('virtual:neon-examples').then((m) => setList(m.default));
  }, []);
  if (!list) return <p class={s.hint}>{t('common.loading')}</p>;
  const groups = [...new Set(list.map((e) => e.group))];
  return (
    <div>
      {groups.map((g) => (
        <section key={g}>
          <div class={s.head}>
            <span>{g}</span>
          </div>
          <List>
            {list
              .filter((e) => e.group === g)
              .map((e) => (
                <Row key={e.id}>
                  <RowMain class={s.example} onClick={() => openProject(e.files, 'main.neon', e.title)} title={e.description}>
                    <span class={listClasses.name}>{e.title}</span>
                    {e.interactive && <Badge accent>{t('examples.interactive')}</Badge>}
                  </RowMain>
                </Row>
              ))}
          </List>
        </section>
      ))}
    </div>
  );
}
