import type { JSX } from 'preact';
import { jumpTo } from '@/app/actions';
import { t } from '@/core/i18n';
import type { Diagnostic } from '@/core/lsp/types';
import { runtime } from '@/core/state/stores';
import { useStore } from '@/core/state/useStore';
import { Icon, List, Row, RowMain, cx } from '@/ui';
import u from '@/styles/utils.module.scss';
import s from './Problems.module.scss';

interface Problem extends Diagnostic {
  readonly file: string;
}

/** Every problem the language server found, in all files. */
export function Problems(): JSX.Element {
  const problems = useStore(runtime, (r) => r.problems);
  const rows: Problem[] = Object.entries(problems).flatMap(([file, list]) => list.map((d) => ({ ...d, file })));
  rows.sort(
    (a, b) =>
      (a.severity ?? 1) - (b.severity ?? 1) || a.file.localeCompare(b.file) || a.range.start.line - b.range.start.line,
  );
  if (rows.length === 0) {
    return (
      <div class={s.empty}>
        <Icon name="check" size={20} />
        <p>{t('problems.none')}</p>
      </div>
    );
  }
  return (
    <List class={s.list}>
      {rows.map((d, i) => (
        <Row key={i}>
          <RowMain class={s.problem} onClick={() => jumpTo(d.file, d.range)}>
            <Icon name={(d.severity ?? 1) === 1 ? 'x' : 'alert'} class={(d.severity ?? 1) === 1 ? u.errorText : u.warnText} />
            <span class={s.message}>{d.message}</span>
            {d.code !== undefined && <span class={cx(u.muted, s.code)}>{d.code}</span>}
            <span class={cx(u.muted, s.where)}>{`${d.file}:${d.range.start.line + 1}:${d.range.start.character + 1}`}</span>
          </RowMain>
        </Row>
      ))}
    </List>
  );
}
