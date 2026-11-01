import { useEffect, useMemo, useRef, useState } from 'preact/hooks';
import { Button, Switch } from './widgets.jsx';
import { Icon } from './icons.jsx';
import { Repl } from './repl-panel.jsx';
import { project, runtime, settings, ui, useStore, filesMap } from '../store.js';
import { t, count } from '../i18n.js';
import { parseAnsi } from '../ansi.js';
import { download, jumpTo, jumpToPlace, stop, submitInput } from '../actions.js';
import * as tc from '../toolchain.js';

export function Panel() {
  const tab = useStore(ui, (u) => u.panelTab);
  const problems = useStore(runtime, (s) => s.problems);
  const errors = Object.values(problems).flat().filter((d) => d.severity === 1).length;
  const warnings = Object.values(problems).flat().filter((d) => d.severity === 2).length;
  const tabs = [
    ['console', t('panel.console')],
    ['problems', t('panel.problems'), errors || warnings ? { n: errors || warnings, kind: errors ? 'error' : 'warning' } : null],
    ['ir', t('panel.ir')],
    ['bytecode', t('panel.bytecode')],
    ['c', t('panel.c')],
    ['repl', t('panel.repl')],
  ];
  return (
    <section class="panel" aria-label={t('panel.label')}>
      <div class="tabs" role="tablist">
        {tabs.map(([id, label, badge]) => (
          <button type="button" role="tab" aria-selected={tab === id} class={`tab ${tab === id ? 'is-active' : ''}`} onClick={() => ui.set({ panelTab: id })}>
            <span>{label}</span>
            {badge && <span class={`count count-${badge.kind}`}>{badge.n}</span>}
          </button>
        ))}
      </div>
      <div class="panel-body">
        {tab === 'console' && <Console />}
        {tab === 'problems' && <Problems />}
        {tab === 'ir' && <ViewPane kind="ir" />}
        {tab === 'bytecode' && <ViewPane kind="bytecode" />}
        {tab === 'c' && <ViewPane kind="c" />}
        {tab === 'repl' && <Repl />}
      </div>
    </section>
  );
}

// ---- console ---------------------------------------------------------------------------------------------------

const PLACE = /^(\s*-->\s+)([^\s:]+):(\d+):(\d+)/;

/** stderr with the `--> file:line:col` places of the diagnostics turned into links. */
function ErrorText({ text }) {
  const lines = text.replace(/\n+$/, '').split('\n');
  return (
    <pre class="out out-error">
      {lines.map((line, i) => {
        const m = PLACE.exec(line);
        if (!m) return <span key={i}>{line}{'\n'}</span>;
        return (
          <span key={i}>
            {m[1]}
            <a href="#" class="place" onClick={(e) => (e.preventDefault(), jumpToPlace(m[2], Number(m[3]), Number(m[4])))}>
              {m[2]}:{m[3]}:{m[4]}
            </a>
            {line.slice(m[0].length)}
            {'\n'}
          </span>
        );
      })}
    </pre>
  );
}

function Stats({ text }) {
  const m = /gc: (\d+) collections, (\d+) allocations, (\d+) objects freed, peak (\d+) bytes/.exec(text ?? '');
  if (!m) return null;
  const kb = Math.max(1, Math.round(Number(m[4]) / 1024));
  return (
    <span class="muted" title="gc">
      {t('console.gc', { c: m[1], a: m[2], f: m[3], kb })}
    </span>
  );
}

function statusOf(run, running) {
  if (running) return { kind: 'busy', text: t('console.running') };
  if (!run) return null;
  const ms = run.ms >= 1000 ? `${(run.ms / 1000).toFixed(2)} ${t('unit.s')}` : `${Math.max(1, Math.round(run.ms))} ${t('unit.ms')}`;
  if (run.timedOut) return { kind: 'error', text: t('console.timeout', { s: settings.get().timeout }) };
  if (run.aborted) return { kind: 'warn', text: t('console.stopped') };
  if (run.crashed) return { kind: 'error', text: t('console.crashed') };
  if (run.check) return { kind: run.code === 0 ? 'ok' : 'error', text: run.code === 0 ? t('console.check_ok') : t('console.check_failed') };
  if (run.needsInput) return { kind: 'wait', text: t('console.waiting_input') };
  if (run.code === 0) return { kind: 'ok', text: t('console.done', { ms }) };
  if (run.code === 1) return { kind: 'error', text: t('console.compile_error', { ms }) };
  return { kind: 'error', text: t('console.runtime_error', { ms }) };
}

function Console() {
  const run = useStore(runtime, (s) => s.run);
  const running = useStore(runtime, (s) => s.running);
  const showOptions = useStore(ui, (u) => u.showRunOptions);
  const s = useStore(settings);
  const parsed = useMemo(() => parseAnsi(run?.stdout ?? ''), [run?.stdout]);
  const status = statusOf(run, running);
  const [line, setLine] = useState('');
  const box = useRef();
  const input = useRef();

  useEffect(() => {
    if (box.current) box.current.scrollTop = box.current.scrollHeight;
    if (run?.needsInput) input.current?.focus();
  }, [run]);

  const style = (r) => {
    const parts = [];
    if (r.fg?.rgb) parts.push(`color:rgb(${r.fg.rgb.join(',')})`);
    return parts.join(';') || undefined;
  };
  const cls = (r) => ['ansi', r.fg?.cls && `fg-${r.fg.cls}`, r.bold && 'bold', r.dim && 'dim', r.underline && 'underline'].filter(Boolean).join(' ');

  return (
    <div class="console">
      <div class="console-bar">
        {running && (
          <Button icon="stop" onClick={stop} danger>
            {t('actions.stop')}
          </Button>
        )}
        <span class="spacer" />
        <Button icon="terminal" active={showOptions} title={t('console.run_options')} onClick={() => ui.set({ showRunOptions: !showOptions })}>
          {t('console.run_options_short')}
        </Button>
        <Button icon="trash" title={t('console.clear')} onClick={() => runtime.set({ run: null })} />
      </div>
      {showOptions && (
        <div class="run-options">
          <label class="field">
            <span class="field-label">{t('options.args')}</span>
            <input value={s.args} placeholder={t('options.args_placeholder')} onInput={(e) => settings.set({ args: e.currentTarget.value })} spellcheck={false} />
          </label>
          <label class="field">
            <span class="field-label">{t('options.stdin')}</span>
            <textarea rows={3} value={s.stdin} placeholder={t('options.stdin_placeholder')} onInput={(e) => settings.set({ stdin: e.currentTarget.value })} spellcheck={false} />
          </label>
        </div>
      )}
      <div class="console-out" ref={box} aria-live="polite">
        {!run && !running && <p class="placeholder">{t('console.empty')}</p>}
        {parsed.truncated && <p class="muted">{t('console.truncated')}</p>}
        {run && (
          <pre class="out">
            {parsed.runs.map((r) => (
              <span class={cls(r)} style={style(r)}>
                {r.text}
              </span>
            ))}
          </pre>
        )}
        {run?.inputs?.length > 0 && (
          <p class="typed">
            <span class="muted">{t('console.typed')}:</span> {run.inputs.map((l, i) => <code key={i}>{l === '' ? '⏎' : l}</code>)}
          </p>
        )}
        {run?.stderr && <ErrorText text={run.stderr} />}
        {run?.needsInput && !running && (
          <form
            class="input-row"
            onSubmit={(e) => {
              e.preventDefault();
              const v = line;
              setLine('');
              submitInput(v);
            }}
          >
            <span class="prompt">›</span>
            <input ref={input} value={line} onInput={(e) => setLine(e.currentTarget.value)} placeholder={t('console.input_placeholder')} spellcheck={false} autocomplete="off" />
          </form>
        )}
      </div>
      {status && (
        <div class={`statusline status-${status.kind}`}>
          <span class="status-dot" />
          <span>{status.text}</span>
          {run?.stats && !running && <Stats text={run.stats} />}
        </div>
      )}
    </div>
  );
}

// ---- problems --------------------------------------------------------------------------------------------------

export function Problems() {
  const problems = useStore(runtime, (s) => s.problems);
  const rows = [];
  for (const [file, list] of Object.entries(problems)) for (const d of list) rows.push({ file, ...d });
  rows.sort((a, b) => a.severity - b.severity || a.file.localeCompare(b.file) || a.range.start.line - b.range.start.line);
  if (rows.length === 0) {
    return (
      <div class="problems empty">
        <Icon name="check" size={20} />
        <p>{t('problems.none')}</p>
      </div>
    );
  }
  return (
    <ul class="problems list" role="list">
      {rows.map((d) => (
        <li class="row">
          <button type="button" class="row-main problem" onClick={() => jumpTo(d.file, d.range)}>
            <Icon name={d.severity === 1 ? 'x' : 'alert'} class={d.severity === 1 ? 'c-error' : 'c-warn'} />
            <span class="problem-msg">{d.message}</span>
            {d.code && <span class="muted code">{d.code}</span>}
            <span class="muted where">
              {d.file}:{d.range.start.line + 1}:{d.range.start.character + 1}
            </span>
          </button>
        </li>
      ))}
    </ul>
  );
}

// ---- IR / bytecode / C ---------------------------------------------------------------------------------------------

const C_KEYWORDS = new Set('static void int char const if else for while return goto switch case break default struct union enum typedef unsigned signed long short double float sizeof inline extern do continue'.split(' '));
const C_TYPES = new Set(['uint8_t', 'uint16_t', 'uint32_t', 'uint64_t', 'int8_t', 'int16_t', 'int32_t', 'int64_t', 'size_t', 'bool', 'W', 'Obj', 'Agg', 'Arr', 'Map', 'Str', 'Frame', 'unit_t']);

/** A cheap highlighter for the generated C (it only has to tell comments, strings, numbers and words apart). */
function highlightC(src) {
  if (src.length > 400_000) return null;
  const esc = (s) => s.replace(/&/g, '&amp;').replace(/</g, '&lt;');
  const re = /\/\*[\s\S]*?\*\/|\/\/[^\n]*|"(?:\\.|[^"\\\n])*"|'(?:\\.|[^'\\\n])+'|^#[^\n]*|\b0x[0-9a-fA-F]+[uUlL]*\b|\b\d+(?:\.\d+)?[uUlLfF]*\b|[A-Za-z_]\w*/gm;
  let out = '';
  let last = 0;
  for (const m of src.matchAll(re)) {
    out += esc(src.slice(last, m.index));
    const w = m[0];
    let cls = null;
    if (w.startsWith('/*') || w.startsWith('//')) cls = 'tok-comment';
    else if (w[0] === '"' || w[0] === "'") cls = 'tok-string';
    else if (w[0] === '#') cls = 'tok-keyword';
    else if (/^\d/.test(w)) cls = 'tok-number';
    else if (C_KEYWORDS.has(w)) cls = 'tok-keyword';
    else if (C_TYPES.has(w)) cls = 'tok-type';
    out += cls ? `<span class="${cls}">${esc(w)}</span>` : esc(w);
    last = m.index + w.length;
  }
  return out + esc(src.slice(last));
}

/** The same for the IR and the bytecode: labels, registers, numbers, comments. */
function highlightListing(src) {
  if (src.length > 400_000) return null;
  const esc = (s) => s.replace(/&/g, '&amp;').replace(/</g, '&lt;');
  const re = /\/\/[^\n]*|#[^\n]*|^f\d+:|^\s*b\d+:|^\s*L\d+:|\bv\d+\b|\br\d+\b|\b(?:return|branch|jump|unreachable|const|intrinsic|call|ret|jmp|loadi|loadk)\b|\b0x[0-9a-fA-F]+\b|\b\d+\b/gm;
  let out = '';
  let last = 0;
  for (const m of src.matchAll(re)) {
    out += esc(src.slice(last, m.index));
    const w = m[0];
    let cls = 'tok-number';
    if (w.startsWith('//') || w.startsWith('#')) cls = 'tok-comment';
    else if (/^\s*(f|b|L)\d+:$/.test(w)) cls = 'tok-function';
    else if (/^[vr]\d+$/.test(w)) cls = 'tok-type';
    else if (/^[a-z]+$/.test(w)) cls = 'tok-keyword';
    out += `<span class="${cls}">${esc(w)}</span>`;
    last = m.index + w.length;
  }
  return out + esc(src.slice(last));
}

export function ViewPane({ kind }) {
  const files = useStore(project, (p) => p.files);
  const entry = useStore(project, (p) => p.entry);
  const opt = useStore(settings, (s) => s.opt);
  const [opts, setOpts] = useState({ effects: false, annotate: true });
  const [state, setState] = useState({ loading: true });
  const seq = useRef(0);

  useEffect(() => {
    const mine = ++seq.current;
    setState((s) => ({ ...s, loading: true }));
    const timer = setTimeout(async () => {
      try {
        const r = await tc.view(kind, filesMap(), entry, { ...opts, opt });
        if (seq.current === mine) setState({ loading: false, ...r });
      } catch (e) {
        if (seq.current === mine) setState({ loading: false, code: -1, stdout: '', stderr: String(e.message) });
      }
    }, 350);
    return () => clearTimeout(timer);
  }, [kind, files, entry, opts, opt]);

  const html = useMemo(() => {
    if (!state.stdout) return null;
    return kind === 'c' ? highlightC(state.stdout) : highlightListing(state.stdout);
  }, [state.stdout, kind]);

  const ext = { ir: 'ir.txt', bytecode: 'bytecode.txt', c: 'c' }[kind];
  const base = entry.replace(/\.neon$/, '');
  const lines = state.stdout ? state.stdout.split('\n').length - 1 : 0;

  return (
    <div class="viewpane">
      <div class="console-bar">
        {kind === 'ir' && <Switch checked={opts.effects} onChange={(v) => setOpts({ ...opts, effects: v })}>{t('view.effects')}</Switch>}
        {kind === 'bytecode' && <Switch checked={opts.annotate} onChange={(v) => setOpts({ ...opts, annotate: v })}>{t('view.annotate')}</Switch>}
        {opt !== 'default' && <span class="badge badge-accent">{t(`opt.${opt}`)}</span>}
        <span class="spacer" />
        {state.stdout && <span class="muted">{count(lines, ['строка', 'строки', 'строк'])}</span>}
        <Button icon="copy" title={t('view.copy')} disabled={!state.stdout} onClick={() => navigator.clipboard?.writeText(state.stdout)} />
        <Button icon="download" title={t('view.download')} disabled={!state.stdout} onClick={() => download(`${base}.${ext}`, state.stdout)} />
      </div>
      <div class={`view-out ${state.loading ? 'is-loading' : ''}`}>
        {state.code > 0 && state.stderr && <ErrorText text={state.stderr} />}
        {state.stdout &&
          (html ? <pre class="code-view" dangerouslySetInnerHTML={{ __html: html }} /> : <pre class="code-view">{state.stdout}</pre>)}
        {!state.loading && !state.stdout && !state.stderr && <p class="placeholder">{t('view.empty')}</p>}
      </div>
    </div>
  );
}
