import { useEffect, useRef, useState } from 'preact/hooks';
import { Button } from './widgets.jsx';
import { repl } from '../lsp.js';
import { needsMore } from '../repl-more.js';
import { t } from '../i18n.js';

/** `text` with the classes the highlighter found: runs are [from, to, class] in characters. */
function Highlighted({ text, runs }) {
  const chars = [...text];
  const out = [];
  let at = 0;
  for (const [from, to, cls] of runs ?? []) {
    if (from > at) out.push(chars.slice(at, from).join(''));
    out.push(<span class={`hl hl-${cls}`}>{chars.slice(from, to).join('')}</span>);
    at = to;
  }
  if (at < chars.length) out.push(chars.slice(at).join(''));
  return <>{out}</>;
}

const HELP = ['help', 'reset', 'clear', 'history'];

export function Repl() {
  const [entries, setEntries] = useState([]); // {input, runs, kind, text}
  const [text, setText] = useState('');
  const [runs, setRuns] = useState([]);
  const [preview, setPreview] = useState(null);
  const [menu, setMenu] = useState(null); // {start, items, selected}
  const [history, setHistory] = useState([]);
  const [at, setAt] = useState(-1);
  const [asking, setAsking] = useState(null); // a cell waiting for a line of input
  const area = useRef();
  const log = useRef();
  const seq = useRef(0);

  // highlighting and the preview follow the text, a little behind it
  useEffect(() => {
    const mine = ++seq.current;
    setPreview(null);
    if (!text.trim()) return void setRuns([]);
    const timer = setTimeout(async () => {
      try {
        const h = await repl.highlight(text);
        if (seq.current !== mine) return;
        setRuns(h.runs);
        if (!text.startsWith(':') && !asking) {
          const p = await repl.preview(text);
          if (seq.current === mine) setPreview(p.value);
        }
      } catch {
        /* the instance is restarting */
      }
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

  const push = (e) => setEntries((list) => [...list, e]);

  const evaluate = async (cell, stdin = '') => {
    const r = await repl.eval(cell, stdin);
    if (r.needsInput) {
      setAsking(cell);
      push({ kind: 'note', text: t('repl.needs_input') });
      return;
    }
    setAsking(null);
    if (r.text) push({ kind: r.kind, text: r.text });
  };

  const submit = async () => {
    const cell = text;
    if (asking) {
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
        const c = (await repl.cells()).cells;
        return push({ kind: 'ok', text: c.map((x, i) => `[${i + 1}] ${x}`).join('\n') || t('repl.no_history') });
      }
      return push({ kind: 'note', text: t('repl.help') });
    }
    await evaluate(cell);
  };

  const complete = async (backward) => {
    const el = area.current;
    if (menu) {
      const n = menu.items.length;
      const sel = menu.selected == null ? (backward ? n - 1 : 0) : (menu.selected + (backward ? n - 1 : 1)) % n;
      const label = menu.items[sel].label;
      const next = text.slice(0, menu.start) + label + text.slice(el.selectionStart);
      setText(next);
      setMenu({ ...menu, selected: sel });
      requestAnimationFrame(() => el.setSelectionRange(menu.start + label.length, menu.start + label.length));
      return;
    }
    const cursor = [...text.slice(0, el.selectionStart)].length;
    const r = await repl.complete(text, cursor);
    if (!r.items.length) return;
    const start = text.slice(0, el.selectionStart).length - [...text.slice(0, el.selectionStart)].slice(r.start).join('').length;
    if (r.items.length === 1) {
      const label = r.items[0].label;
      setText(text.slice(0, start) + label + text.slice(el.selectionStart));
      return;
    }
    setMenu({ start, items: r.items, selected: null });
  };

  const onKey = (e) => {
    const el = e.currentTarget;
    if (e.key === 'Tab') {
      e.preventDefault();
      return complete(e.shiftKey);
    }
    if (e.key === 'Escape') return setMenu(null);
    if (menu && e.key !== 'Shift') setMenu(null);
    if (e.key === 'Enter' && !e.shiftKey && !e.ctrlKey && !e.altKey) {
      e.preventDefault();
      const more = !asking && !text.startsWith(':') && needsMore(text);
      if (more) {
        const indent = /^\s*/.exec(text.split('\n').at(-1))[0] + (/[{(\[]\s*$/.test(text) ? '    ' : '');
        const pos = el.selectionStart;
        setText(`${text.slice(0, pos)}\n${indent}${text.slice(el.selectionEnd)}`);
        requestAnimationFrame(() => el.setSelectionRange(pos + 1 + indent.length, pos + 1 + indent.length));
        return;
      }
      return submit();
    }
    if (e.key === 'ArrowUp' && !text.slice(0, el.selectionStart).includes('\n') && history.length) {
      e.preventDefault();
      const i = at < 0 ? history.length - 1 : Math.max(0, at - 1);
      setAt(i);
      setText(history[i]);
    }
    if (e.key === 'ArrowDown' && at >= 0 && !text.slice(el.selectionEnd).includes('\n')) {
      e.preventDefault();
      const i = at + 1;
      if (i >= history.length) {
        setAt(-1);
        setText('');
      } else {
        setAt(i);
        setText(history[i]);
      }
    }
    if (e.key === 'l' && e.ctrlKey) {
      e.preventDefault();
      setEntries([]);
    }
  };

  return (
    <div class="repl" onClick={() => area.current?.focus()}>
      <div class="console-bar">
        <span class="muted">{t('repl.hint')}</span>
        <span class="spacer" />
        <Button icon="reset" title={t('repl.reset')} onClick={async () => { await repl.reset(); setEntries([]); }} />
        <Button icon="trash" title={t('console.clear')} onClick={() => setEntries([])} />
      </div>
      <div class="repl-log" ref={log}>
        {entries.length === 0 && <p class="placeholder">{t('repl.empty')}</p>}
        {entries.map((e) => {
          if (e.kind === 'input') {
            return (
              <pre class="repl-entry repl-input">
                <span class="prompt">›</span> <Highlighted text={e.text} runs={e.runs} />
              </pre>
            );
          }
          if (e.kind === 'typed') return <pre class="repl-entry repl-input"><span class="prompt">·</span> {e.text}</pre>;
          if (e.kind === 'note') return <pre class="repl-entry muted">{e.text}</pre>;
          const cls = e.kind === 'ok' ? 'out' : 'out out-error';
          return <pre class={`repl-entry ${cls}`}>{e.text.replace(/\n$/, '')}</pre>;
        })}
      </div>
      <div class="repl-editor">
        <span class="prompt">{asking ? '·' : '›'}</span>
        <div class="repl-field">
          <pre class="repl-overlay" aria-hidden="true">
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
            placeholder={asking ? t('repl.input_placeholder') : t('repl.placeholder')}
            onInput={(e) => setText(e.currentTarget.value)}
            onKeyDown={onKey}
            aria-label={t('panel.repl')}
          />
        </div>
      </div>
      {menu && (
        <ul class="repl-menu" role="listbox">
          {menu.items.slice(0, 40).map((it, i) => (
            <li class={i === menu.selected ? 'is-active' : ''} role="option" aria-selected={i === menu.selected}>
              <span>{it.label}</span>
              {it.detail && <span class="muted">{it.detail}</span>}
            </li>
          ))}
        </ul>
      )}
      {preview && !menu && <div class="repl-preview">↳ {preview.split('\n')[0]}</div>}
    </div>
  );
}
