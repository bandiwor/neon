// What the page asks of the compiler: run a program, look at its IR / bytecode / C, format, check.
// Programs run in a worker of their own that can be killed (an endless loop must not freeze the page).
import { Engine } from './engine.js';

const runnerEngine = new Engine('run');
const toolsEngine = new Engine('tools');

let runToken = 0;

/** The command line flags for the optimizer setting. */
function optFlags(opt) {
  return { none: ['--no-opt'], 'no-inline': ['--no-inline'], 'no-peephole': ['--no-peephole'] }[opt] ?? [];
}

function splitArgs(text) {
  // shell-like: words, "double" and 'single' quotes
  const out = [];
  const re = /"([^"]*)"|'([^']*)'|(\S+)/g;
  let m;
  while ((m = re.exec(text ?? ''))) out.push(m[1] ?? m[2] ?? m[3]);
  return out;
}

/** The arguments of `neon run` for the settings. */
export function runArgs(entry, s) {
  const args = ['run', entry, ...optFlags(s.opt)];
  if (s.maxSteps > 0) args.push('--max-steps', String(s.maxSteps));
  if (s.gcStress) args.push('--gc-stress');
  if (String(s.gcLimit).trim()) args.push('--gc-limit', String(s.gcLimit).trim());
  if (s.stats) args.push('--stats');
  const prog = splitArgs(s.args);
  if (prog.length) args.push('--', ...prog);
  return args;
}

/**
 * Runs the program. Resolves with {code, stdout, stderr, needsInput, ms, timedOut, aborted}.
 * `answers` are the lines of input the program may read; if it wants more and `interactive` is set, it
 * stops with `needsInput` so the page can ask the user and run it again.
 */
export async function runProgram({ files, entry, settings, answers, interactive }) {
  const token = ++runToken;
  const started = performance.now();
  let timer;
  const timeout = new Promise((resolve) => {
    if (settings.timeout > 0) {
      timer = setTimeout(() => resolve('timeout'), settings.timeout * 1000);
    }
  });
  try {
    const reply = runnerEngine.call({
      op: 'cli',
      args: runArgs(entry, settings),
      files,
      stdin: answers,
      interactive,
    });
    const winner = await Promise.race([reply, timeout]);
    if (winner === 'timeout') {
      runnerEngine.terminate();
      return { code: -1, stdout: '', stderr: '', needsInput: false, ms: performance.now() - started, timedOut: true };
    }
    return { ...winner, ms: performance.now() - started };
  } catch (e) {
    if (e.terminated) {
      return { code: -1, stdout: '', stderr: '', needsInput: false, ms: performance.now() - started, aborted: token === runToken ? true : 'replaced' };
    }
    return { code: -1, stdout: '', stderr: `внутренняя ошибка: ${e.message}\n`, needsInput: false, ms: performance.now() - started, crashed: true };
  } finally {
    clearTimeout(timer);
  }
}

export function stopProgram() {
  runToken++;
  runnerEngine.terminate();
}

/** Warms the workers up so the first run does not wait for the module. */
export function warmUp() {
  runnerEngine.start().catch(() => {});
  toolsEngine.start().catch(() => {});
}

async function tool(args, files, extra = {}) {
  return toolsEngine.call({ op: 'cli', args, files, stdin: '', ...extra });
}

/** `neon ir | disasm | c` for the project: {code, stdout, stderr}. */
export function view(kind, files, entry, opts = {}) {
  const base = optFlags(opts.opt ?? 'default');
  if (kind === 'ir') return tool(['ir', entry, ...base, ...(opts.effects ? ['--effects'] : [])], files);
  if (kind === 'bytecode') return tool(['disasm', entry, ...base, ...(opts.annotate ? ['--annotate'] : [])], files);
  if (kind === 'c') return tool(['c', entry, ...base], files);
  throw new Error(`unknown view ${kind}`);
}

/** `neon check`: the rendered diagnostics. */
export function check(files, entry) {
  return tool(['check', entry], files);
}

/** `neon fmt` of one file: {ok, text, error}. */
export async function format(files, name) {
  const r = await tool(['fmt', name], files);
  return r.code === 0 ? { ok: true, text: r.stdout } : { ok: false, error: r.stderr };
}

export const toolsRestart = (f) => (toolsEngine.onCrash = f);
export async function version() {
  return (await toolsEngine.call({ op: 'version' })).version;
}
