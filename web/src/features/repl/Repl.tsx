import type { JSX } from 'preact';
import { useEffect, useRef, useState } from 'preact/hooks';
import { t } from '@/core/i18n';
import { repl } from '@/core/lsp/repl';
import { needsMore } from '@/core/util/replMore';
import type { HighlightRun, ReplCandidate } from '@/core/wasm/protocol';
import { Button, cx } from '@/ui';
import u from '@/styles/utils.module.scss';
import { Highlighted } from './Highlighted';
import s from './Repl.module.scss';

type Entry =
  | { readonly kind: 'input'; readonly text: string; readonly runs: readonly HighlightRun[] }
  | { readonly kind: 'typed' | 'note' | 'ok' | 'rejected' | 'trapped'; readonly text: string };

interface Menu {
  readonly start: number;
  readonly items: readonly ReplCandidate[];
  readonly selected: number | null;
}

/** An interactive session: every cell sees the ones before it. */
export function Repl(): JSX.Element {
  const [entries, setEntries] = useState<readonly Entry[]>([]);
  const [text, setText] = useState('');
  const [runs, setRuns] = useState<readonly HighlightRun[]>([]);
  const [preview, setPreview] = useState<string | null>(null);
  const [menu, setMenu] = useState<Menu | null>(null);
  const [history, setHistory] = useState<readonly string[]>([]);
  const [at, setAt] = useState(-1);
  const [asking, setAsking] = useState<string | null>(null); // a cell waiting for a line of input
  const area = useRef<HTMLTextAreaElement>(null);
  const log = useRef<HTMLDivElement>(null);
  const seq = useRef(0);

  // the colors and the preview follow the text, a little behind it
  useEffect(() => {
    const mine = ++seq.current;
    setPreview(null);
    if (!text.trim()) {
      setRuns([]);
      return undefined;
    }
    const timer = setTimeout(() => {
      void (async () => {
        try {
          const h = await repl.highlight(text);
          if (seq.current !== mine) return;
          setRuns(h.runs);
          if (!text.startsWith(':') && asking === null) {
            const p = await repl.preview(text);
            if (seq.current === mine) setPreview(p.value);
          }
        } catch {
          /* the instance is restarting */
        }
      })();
    }, 90);
    return () => clearTimeout(timer);
  }, [text]);

  useEffect(() => {
    if (log.current) log.current.scrollTop = log.current.scrollHeight;
  }, [entries]);

  useEffect(() => {
    const el = area.current;
    if (el) {
      el.style.height = 'auto';
      el.style.height = `${Math.min(el.scrollHeight, 160)}px`;
    }
  }, [text]);

  const push = (e: Entry): void => setEntries((list) => [...list, e]);

  const evaluate = async (cell: string, stdin = ''): Promise<void> => {
    const r = await repl.eval(cell, stdin);
    if (r.needsInput) {
      setAsking(cell);
      push({ kind: 'note', text: t('repl.needs_input') });
      return;
    }
    setAsking(null);
    if (r.text) push({ kind: r.kind, text: r.text });
  };

  const submit = async (): Promise<void> => {
    const cell = text;
    if (asking !== null) {
      push({ kind: 'typed', text: cell });
      setText('');
      return evaluate(asking, `${cell}\n`);
    }
    if (!cell.trim()) return;
    setHistory((h) => (h.at(-1) === cell ? h : [...h, cell]));
    setAt(-1);
    setText('');
    setMenu(null);
    push({ kind: 'input', text: cell, runs });
    const cmd = cell.trim();
    if (cmd.startsWith(':')) {
      const name = cmd.slice(1).split(/\s+/)[0];
      if (name === 'reset') {
        await repl.reset();
        return push({ kind: 'note', text: t('repl.reset_done') });
      }
      if (name === 'clear') return setEntries([]);
      if (name === 'history') {
        const { cells } = await repl.cells();
        return push({ kind: 'ok', text: cells.map((c, i) => `[${i + 1}] ${c}`).join('\n') || t('repl.no_history') });
      }
      return push({ kind: 'note', text: t('repl.help') });
    }
    await evaluate(cell);
  };

  const complete = async (backward: boolean): Promise<void> => {
    const el = area.current;
    if (!el) return;
    if (menu) {
      const n = menu.items.length;
      const sel = menu.selected === null ? (backward ? n - 1 : 0) : (menu.selected + (backward ? n - 1 : 1)) % n;
      const label = menu.items[sel]?.label ?? '';
      setText(text.slice(0, menu.start) + label + text.slice(el.selectionStart));
      setMenu({ ...menu, selected: sel });
      requestAnimationFrame(() => el.setSelectionRange(menu.start + label.length, menu.start + label.length));
      return;
    }
    const upToCaret = text.slice(0, el.selectionStart);
    const r = await repl.complete(text, [...upToCaret].length);
    const first = r.items[0];
    if (!first) return;
    const start = upToCaret.length - [...upToCaret].slice(r.start).join('').length;
    if (r.items.length === 1) {
      setText(text.slice(0, start) + first.label + text.slice(el.selectionStart));
      return;
    }
    setMenu({ start, items: r.items, selected: null });
  };

  const onKey = (e: JSX.TargetedKeyboardEvent<HTMLTextAreaElement>): void => {
    const el = e.currentTarget;
    if (e.key === 'Tab') {
      e.preventDefault();
      void complete(e.shiftKey);
      return;
    }
    if (e.key === 'Escape') return setMenu(null);
    if (menu && e.key !== 'Shift') setMenu(null);
    if (e.key === 'Enter' && !e.shiftKey && !e.ctrlKey && !e.altKey) {
      e.preventDefault();
      if (asking === null && !text.startsWith(':') && needsMore(text)) {
        const last = text.split('\n').at(-1) ?? '';
        const indent = (/^\s*/.exec(last)?.[0] ?? '') + (/[{(\[]\s*$/.test(text) ? '    ' : '');
        const pos = el.selectionStart;
        setText(`${text.slice(0, pos)}\n${indent}${text.slice(el.selectionEnd)}`);
        requestAnimationFrame(() => el.setSelectionRange(pos + 1 + indent.length, pos + 1 + indent.length));
        return;
      }
      void submit();
      return;
    }
    if (e.key === 'ArrowUp' && !text.slice(0, el.selectionStart).includes('\n') && history.length > 0) {
      e.preventDefault();
      const i = at < 0 ? history.length - 1 : Math.max(0, at - 1);
      setAt(i);
      setText(history[i] ?? '');
    }
    if (e.key === 'ArrowDown' && at >= 0 && !text.slice(el.selectionEnd).includes('\n')) {
      e.preventDefault();
      const i = at + 1;
      if (i >= history.length) {
        setAt(-1);
        setText('');
      } else {
        setAt(i);
        setText(history[i] ?? '');
      }
    }
    if (e.key === 'l' && e.ctrlKey) {
      e.preventDefault();
      setEntries([]);
    }
  };

  return (
    <div class={s.repl} onClick={() => area.current?.focus()}>
      <div class={s.bar}>
        <span class={u.muted}>{t('repl.hint')}</span>
        <span class={u.spacer} />
        <Button
          icon="reset"
          title={t('repl.reset')}
          onClick={() => {
            void repl.reset();
            setEntries([]);
          }}
        />
        <Button icon="trash" title={t('console.clear')} onClick={() => setEntries([])} />
      </div>
      <div class={s.log} ref={log}>
        {entries.length === 0 && <p class={s.placeholder}>{t('repl.empty')}</p>}
        {entries.map((e, i) => {
          switch (e.kind) {
            case 'input':
              return (
                <pre key={i} class={cx(s.entry, s.input)}>
                  <span class={s.prompt}>›</span> <Highlighted text={e.text} runs={e.runs} />
                </pre>
              );
            case 'typed':
              return (
                <pre key={i} class={cx(s.entry, s.input)}>
                  <span class={s.prompt}>·</span> {e.text}
                </pre>
              );
            case 'note':
              return (
                <pre key={i} class={cx(s.entry, u.muted)}>
                  {e.text}
                </pre>
              );
            case 'ok':
              return (
                <pre key={i} class={cx(s.entry, s.out)}>
                  {e.text.replace(/\n$/, '')}
                </pre>
              );
            default:
              return (
                <pre key={i} class={cx(s.entry, s.out, s.error)}>
                  {e.text.replace(/\n$/, '')}
                </pre>
              );
          }
        })}
      </div>
      <div class={s.editor}>
        <span class={s.prompt}>{asking !== null ? '·' : '›'}</span>
        <div class={s.field}>
          <pre class={s.overlay} aria-hidden="true">
            <Highlighted text={text} runs={runs} />
            {'\n'}
          </pre>
          <textarea
            ref={area}
            value={text}
            rows={1}
            spellcheck={false}
            autocomplete="off"
            autocapitalize="off"
            placeholder={asking !== null ? t('repl.input_placeholder') : t('repl.placeholder')}
            onInput={(e) => setText(e.currentTarget.value)}
            onKeyDown={onKey}
            aria-label={t('panel.repl')}
          />
        </div>
      </div>
      {menu && (
        <ul class={s.menu} role="listbox">
          {menu.items.slice(0, 40).map((it, i) => (
            <li key={it.label} class={cx(i === menu.selected && s.selected)} role="option" aria-selected={i === menu.selected}>
              <span>{it.label}</span>
              {it.detail && <span class={u.muted}>{it.detail}</span>}
            </li>
          ))}
        </ul>
      )}
      {preview && !menu && <div class={s.preview}>↳ {preview.split('\n')[0]}</div>}
    </div>
  );
}
