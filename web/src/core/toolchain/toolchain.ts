// What the page asks of the compiler: run a program, look at its IR / bytecode / C, format, check.
// Programs run in a worker of their own that can be killed (an endless loop must not freeze the page).
import { Engine, TerminatedError } from '../wasm/engine';
import type { CliReply, Files } from '../wasm/protocol';
import type { OptFlags, Settings } from '../state/types';
import { optFlags, runArgs } from './args';

const runnerEngine = new Engine('run');
const toolsEngine = new Engine('tools');

let runToken = 0;

export interface RunRequest {
  readonly files: Files;
  readonly entry: string;
  readonly settings: Settings;
  /** What the program may read: lines (newline-terminated) and keys (the bytes a terminal sends). */
  readonly answers: string;
}

export interface RunReport extends CliReply {
  readonly ms: number;
  readonly timedOut?: boolean;
  readonly aborted?: boolean | 'replaced';
  readonly crashed?: boolean;
}

const EMPTY: CliReply = { code: -1, stdout: '', stderr: '', needsInput: false, needsKey: false };

/**
 * Runs the program. If it wants more input than `answers` holds it stops with `needsInput`, so the page
 * can ask the user and run it again.
 */
export async function runProgram(request: RunRequest): Promise<RunReport> {
  const token = ++runToken;
  const started = performance.now();
  const elapsed = (): number => performance.now() - started;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const timeout = new Promise<'timeout'>((resolve) => {
    if (request.settings.timeout > 0) timer = setTimeout(() => resolve('timeout'), request.settings.timeout * 1000);
  });
  try {
    const reply = runnerEngine.call('cli', {
      args: runArgs(request.entry, request.settings),
      files: request.files,
      stdin: request.answers,
      interactive: true,
      exact: true,
    });
    const winner = await Promise.race([reply, timeout]);
    if (winner === 'timeout') {
      runnerEngine.terminate();
      return { ...EMPTY, ms: elapsed(), timedOut: true };
    }
    return { ...winner, ms: elapsed() };
  } catch (e) {
    if (e instanceof TerminatedError) return { ...EMPTY, ms: elapsed(), aborted: token === runToken ? true : 'replaced' };
    return { ...EMPTY, stderr: `внутренняя ошибка: ${e instanceof Error ? e.message : String(e)}\n`, ms: elapsed(), crashed: true };
  } finally {
    clearTimeout(timer);
  }
}

export function stopProgram(): void {
  runToken++;
  runnerEngine.terminate();
}

/** Warms the workers up so the first run does not wait for the module. */
export function warmUp(): void {
  runnerEngine.start().catch(() => undefined);
  toolsEngine.start().catch(() => undefined);
}

const tool = (args: readonly string[], files: Files): Promise<CliReply> =>
  toolsEngine.call('cli', { args, files, stdin: '', interactive: false });

export type ViewKind = 'ir' | 'bytecode' | 'c';

export interface ViewOptions {
  readonly opt: OptFlags;
  readonly effects?: boolean;
  readonly annotate?: boolean;
}

/** `neon ir | disasm | c` for the project. */
export function view(kind: ViewKind, files: Files, entry: string, options: ViewOptions): Promise<CliReply> {
  const base = optFlags(options.opt);
  switch (kind) {
    case 'ir':
      return tool(['ir', entry, ...base, ...(options.effects ? ['--effects'] : [])], files);
    case 'bytecode':
      return tool(['disasm', entry, ...base, ...(options.annotate ? ['--annotate'] : [])], files);
    case 'c':
      return tool(['c', entry, ...base], files);
  }
}

/** `neon check`: the rendered diagnostics. */
export const check = (files: Files, entry: string): Promise<CliReply> => tool(['check', entry], files);

export type FormatResult = { readonly ok: true; readonly text: string } | { readonly ok: false; readonly error: string };

/** `neon fmt` of one file. */
export async function format(files: Files, name: string): Promise<FormatResult> {
  const r = await tool(['fmt', name], files);
  return r.code === 0 ? { ok: true, text: r.stdout } : { ok: false, error: r.stderr };
}

export async function version(): Promise<string> {
  return (await toolsEngine.call('version', {})).version;
}
