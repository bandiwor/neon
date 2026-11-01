import type { JSX } from 'preact';
import { useEffect, useMemo, useRef, useState } from 'preact/hooks';
import { download } from '@/app/actions';
import { ErrorText } from '@/features/console';
import { count, t } from '@/core/i18n';
import { filesMap } from '@/core/project/files';
import { project, settings } from '@/core/state/stores';
import { useStore } from '@/core/state/useStore';
import { view, type ViewKind } from '@/core/toolchain/toolchain';
import type { CliReply } from '@/core/wasm/protocol';
import { Button, Switch, cx } from '@/ui';
import u from '@/styles/utils.module.scss';
import { highlightC, highlightListing, type ListingClasses } from './highlight';
import s from './ViewPane.module.scss';

const CLASSES: ListingClasses = { keyword: s.keyword ?? '', string: s.string ?? '', number: s.number ?? '', comment: s.comment ?? '', type: s.type ?? '', label: s.label ?? '' };

const EXTENSION: Readonly<Record<ViewKind, string>> = { ir: 'ir.txt', bytecode: 'bytecode.txt', c: 'c' };

interface Loaded extends Partial<CliReply> {
  readonly loading: boolean;
}

/** The IR, the bytecode or the C translation of the project, kept up to date while you type. */
export function ViewPane({ kind }: { readonly kind: ViewKind }): JSX.Element {
  const files = useStore(project, (p) => p.files);
  const entry = useStore(project, (p) => p.entry);
  const opt = useStore(settings, (st) => st.opt);
  const [options, setOptions] = useState({ effects: false, annotate: true });
  const [state, setState] = useState<Loaded>({ loading: true });
  const seq = useRef(0);

  useEffect(() => {
    const mine = ++seq.current;
    setState((prev) => ({ ...prev, loading: true }));
    const timer = setTimeout(() => {
      view(kind, filesMap(), entry, { opt, ...options })
        .then((r) => seq.current === mine && setState({ loading: false, ...r }))
        .catch((e: unknown) => seq.current === mine && setState({ loading: false, code: -1, stdout: '', stderr: String(e instanceof Error ? e.message : e) }));
    }, 350);
    return () => clearTimeout(timer);
  }, [kind, files, entry, options, opt]);

  const stdout = state.stdout ?? '';
  const html = useMemo(() => (stdout ? (kind === 'c' ? highlightC(stdout, CLASSES) : highlightListing(stdout, CLASSES)) : null), [stdout, kind]);
  const lines = stdout ? stdout.split('\n').length - 1 : 0;
  const base = entry.replace(/\.neon$/, '');

  return (
    <div class={s.pane}>
      <div class={s.bar}>
        {kind === 'ir' && (
          <Switch checked={options.effects} onChange={(v) => setOptions({ ...options, effects: v })}>
            {t('view.effects')}
          </Switch>
        )}
        {kind === 'bytecode' && (
          <Switch checked={options.annotate} onChange={(v) => setOptions({ ...options, annotate: v })}>
            {t('view.annotate')}
          </Switch>
        )}
        {opt !== 'default' && <span class={s.optBadge}>{t(`opt.${opt}`)}</span>}
        <span class={u.spacer} />
        {stdout && <span class={u.muted}>{count(lines, ['строка', 'строки', 'строк'])}</span>}
        <Button icon="copy" title={t('view.copy')} disabled={!stdout} onClick={() => void navigator.clipboard?.writeText(stdout)} />
        <Button icon="download" title={t('view.download')} disabled={!stdout} onClick={() => download(`${base}.${EXTENSION[kind]}`, stdout)} />
      </div>
      <div class={cx(s.out, state.loading && s.loading)}>
        {(state.code ?? 0) > 0 && state.stderr && <ErrorText text={state.stderr} />}
        {stdout &&
          (html ? <pre class={s.code} dangerouslySetInnerHTML={{ __html: html }} /> : <pre class={s.code}>{stdout}</pre>)}
        {!state.loading && !stdout && !state.stderr && <p class={s.empty}>{t('view.empty')}</p>}
      </div>
    </div>
  );
}
