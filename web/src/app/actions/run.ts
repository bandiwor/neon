import { t } from '@/core/i18n';
import { flush } from '@/core/lsp/sync';
import { filesMap } from '@/core/project/files';
import { project, runtime, settings, ui } from '@/core/state/stores';
import { toast } from '@/core/state/toast';
import type { TerminalKey } from '@/core/util/keys';
import { check, runProgram, stopProgram } from '@/core/toolchain/toolchain';

/** What the program is given at the start: the text of the settings. */
let initial = '';

/** What it has been given since: a line or a key at a time (the console re-runs it with one more each time). */
let answers: { shown: string; raw: string; line: boolean }[] = [];

/** `gc: ...` lines of `--stats` come out on stderr; the console shows them apart. */
function splitStats(stderr: string): { stderr: string; stats: string } {
  const stats: string[] = [];
  const kept: string[] = [];
  for (const line of stderr.split('\n')) (line.startsWith('gc: ') ? stats : kept).push(line);
  return { stderr: kept.join('\n'), stats: stats.join('\n') };
}

async function execute(): Promise<void> {
  await flush();
  const { entry } = project.get();
  runtime.set({ running: true });
  const report = await runProgram({
    files: filesMap(),
    entry,
    settings: settings.get(),
    answers: initial + answers.map((a) => a.raw).join(''),
  });
  if (report.aborted === 'replaced') return; // another run took over
  // a program that stopped to ask for input did not fail: what the module reports then is only that
  const { stderr, stats } = splitStats(report.needsInput ? '' : report.stderr);
  runtime.set({
    running: false,
    run: {
      code: report.code,
      stdout: report.stdout,
      needsInput: report.needsInput,
      needsKey: report.needsKey,
      stderr,
      stats,
      ms: report.ms,
      ...(report.timedOut ? { timedOut: true } : {}),
      ...(report.aborted === true ? { aborted: true } : {}),
      ...(report.crashed ? { crashed: true } : {}),
      inputs: typedLines(),
      entry,
    },
  });
}

/** The lines the user typed (the keys are not listed: a program that reads keys draws what they did). */
const typedLines = (): string[] => answers.filter((a) => a.line).map((a) => a.shown);

/** Runs the project's entry file from the start. */
export async function run(): Promise<void> {
  if (runtime.get().running) await stop();
  const given = settings.get().stdin;
  initial = given === '' ? '' : `${given.replace(/\n$/, '')}\n`;
  answers = [];
  ui.set({ panelOpen: true, panelTab: 'console' });
  runtime.set({ run: null });
  await execute();
}

/** The user typed a line for a program that asked for input: run again with it. */
export async function submitInput(line: string): Promise<void> {
  answers.push({ shown: line, raw: `${line}\n`, line: true });
  await execute();
}

/** The user pressed a key for a program that waits for one: run again with it. */
export async function submitKey(key: TerminalKey): Promise<void> {
  if (runtime.get().running) return;
  answers.push({ shown: key.name, raw: key.bytes, line: false });
  await execute();
}

export async function stop(): Promise<void> {
  stopProgram();
  const last = runtime.get().run;
  runtime.set({
    running: false,
    run: {
      code: -1,
      stdout: last?.stdout ?? '',
      needsInput: false,
      needsKey: false,
      stderr: '',
      stats: '',
      ms: 0,
      aborted: true,
      inputs: typedLines(),
      entry: project.get().entry,
    },
  });
}

/** Forgets the input given so far (a new project was opened). */
export function resetInput(): void {
  initial = '';
  answers = [];
}

/** `neon check`: the rendered diagnostics in the console. */
export async function checkProject(): Promise<void> {
  await flush();
  const { entry } = project.get();
  const r = await check(filesMap(), entry);
  runtime.set({
    run: { code: r.code, stdout: '', needsInput: false, needsKey: false, stderr: r.stderr, stats: '', ms: 0, check: true, inputs: [], entry },
  });
  ui.set({ panelOpen: true, panelTab: 'console' });
  toast(r.code === 0 ? t('toast.check_ok') : t('toast.check_errors'), r.code === 0 ? 'ok' : 'error');
}
