import type { JSX } from 'preact';
import { useEffect, useMemo, useRef, useState } from 'preact/hooks';
import { jumpToPlace, submitInput, submitKey } from '@/app/actions';
import { t } from '@/core/i18n';
import { runtime, settings } from '@/core/state/stores';
import type { RunOutcome } from '@/core/state/types';
import { useStore } from '@/core/state/useStore';
import { parseAnsi, type AnsiRun } from '@/core/util/ansi';
import { terminalKey } from '@/core/util/keys';
import { Button, cx } from '@/ui';
import u from '@/styles/utils.module.scss';
import s from './Console.module.scss';

const PLACE = /^(\s*-->\s+)([^\s:]+):(\d+):(\d+)/;

/** stderr, with the `--> file:line:col` places of the diagnostics turned into links. */
export function ErrorText({ text }: { readonly text: string }): JSX.Element {
  const lines = text.replace(/\n+$/, '').split('\n');
  return (
    <pre class={cx(s.out, s.error)}>
      {lines.map((line, i) => {
        const m = PLACE.exec(line);
        if (!m) return <span key={i}>{`${line}\n`}</span>;
        const [whole, lead = '', file = '', row = '0', col = '0'] = m;
        return (
          <span key={i}>
            {lead}
            <a
              href="#"
              class={s.place}
              onClick={(e) => {
                e.preventDefault();
                jumpToPlace(file, Number(row), Number(col));
              }}
            >{`${file}:${row}:${col}`}</a>
            {`${line.slice(whole.length)}\n`}
          </span>
        );
      })}
    </pre>
  );
}

// the named ANSI colors the theme draws
const NAMED: Readonly<Record<string, string | undefined>> = {
  black: s.fgBlack, red: s.fgRed, green: s.fgGreen, yellow: s.fgYellow, blue: s.fgBlue, magenta: s.fgMagenta, cyan: s.fgCyan, white: s.fgWhite,
  'b-black': s.fgBBlack, 'b-red': s.fgBRed, 'b-green': s.fgBGreen, 'b-yellow': s.fgBYellow, 'b-blue': s.fgBBlue, 'b-magenta': s.fgBMagenta, 'b-cyan': s.fgBCyan, 'b-white': s.fgBWhite,
};

function Run({ run }: { readonly run: AnsiRun }): JSX.Element {
  const fg = run.fg;
  const named = fg && 'name' in fg ? NAMED[fg.name] : undefined;
  const style = fg && 'rgb' in fg ? { color: `rgb(${fg.rgb.join(',')})` } : undefined;
  return (
    <span class={cx(named, run.bold && s.bold, run.dim && s.dim, run.underline && s.underline)} style={style}>
      {run.text}
    </span>
  );
}

function Stats({ text }: { readonly text: string }): JSX.Element | null {
  const m = /gc: (\d+) collections, (\d+) allocations, (\d+) objects freed, peak (\d+) bytes/.exec(text);
  if (!m) return null;
  const kb = Math.max(1, Math.round(Number(m[4]) / 1024));
  return <span class={cx(u.muted, s.statsText)}>{t('console.gc', { c: m[1] ?? '0', a: m[2] ?? '0', f: m[3] ?? '0', kb })}</span>;
}

type StatusKind = 'busy' | 'ok' | 'error' | 'warn' | 'wait';

function statusOf(run: RunOutcome | null, running: boolean): { kind: StatusKind; text: string } | null {
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

const STATUS_CLASS: Readonly<Record<StatusKind, string>> = { busy: s.busy ?? '', ok: s.ok ?? '', error: s.err ?? '', warn: s.warn ?? '', wait: s.warn ?? '' };

/** The output of the last run, the input prompt of a program that waits, the run options. */
export function Console(): JSX.Element {
  const run = useStore(runtime, (r) => r.run);
  const running = useStore(runtime, (r) => r.running);
  const parsed = useMemo(() => parseAnsi(run?.stdout ?? ''), [run?.stdout]);
  const status = statusOf(run, running);
  const [line, setLine] = useState('');
  const box = useRef<HTMLDivElement>(null);
  const input = useRef<HTMLInputElement>(null);
  const keys = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (box.current) box.current.scrollTop = box.current.scrollHeight;
    if (run?.needsInput) (run.needsKey ? keys.current : input.current)?.focus();
  }, [run]);

  return (
    <div class={s.console}>
      <div class={s.bar}>
        <span class={u.spacer} />
        <Button icon="trash" title={t('console.clear')} onClick={() => runtime.set({ run: null })} />
      </div>
      <div class={s.output} ref={box} aria-live="polite">
        {parsed.truncated && <p class={u.muted}>{t('console.truncated')}</p>}
        {run && (
          <pre class={s.out}>
            {parsed.runs.map((r, i) => (
              <Run key={i} run={r} />
            ))}
          </pre>
        )}
        {run && run.inputs.length > 0 && (
          <p class={s.typed}>
            <span class={u.muted}>{t('console.typed')}:</span>{' '}
            {run.inputs.map((l, i) => (
              <code key={i}>{l === '' ? '⏎' : l}</code>
            ))}
          </p>
        )}
        {run?.stderr && <ErrorText text={run.stderr} />}
        {run?.needsInput && run.needsKey && !running && (
          <div
            ref={keys}
            class={s.keys}
            tabIndex={0}
            role="textbox"
            aria-label={t('console.press_key')}
            onKeyDown={(e) => {
              const key = terminalKey(e);
              if (!key) return;
              e.preventDefault();
              e.stopPropagation();
              void submitKey(key);
            }}
          >
            <span class={s.prompt}>⌨</span>
            <span class={u.muted}>{t('console.press_key')}</span>
          </div>
        )}
        {run?.needsInput && !run.needsKey && !running && (
          <form
            class={s.inputRow}
            onSubmit={(e) => {
              e.preventDefault();
              const value = line;
              setLine('');
              void submitInput(value);
            }}
          >
            <span class={s.prompt}>›</span>
            <input ref={input} value={line} onInput={(e) => setLine(e.currentTarget.value)} placeholder={t('console.input_placeholder')} spellcheck={false} autocomplete="off" />
          </form>
        )}
      </div>
      {status && (
        <div class={cx(s.status, STATUS_CLASS[status.kind])}>
          <span class={s.dotMark} />
          <span>{status.text}</span>
          {run?.stats && !running && <Stats text={run.stats} />}
        </div>
      )}
    </div>
  );
}
