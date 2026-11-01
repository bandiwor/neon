import { t } from '@/core/i18n';
import { flush } from '@/core/lsp/sync';
import { filesMap } from '@/core/project/files';
import { project, runtime, settings, ui } from '@/core/state/stores';
import { toast } from '@/core/state/toast';
import { check, runProgram, stopProgram } from '@/core/toolchain/toolchain';

/** The lines the program has been given so far (the console re-runs it with one more each time). */
let answers: string[] = [];

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
    answers: answers.length > 0 ? `${answers.join('\n')}\n` : '',
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
      stderr,
      stats,
      ms: report.ms,
      ...(report.timedOut ? { timedOut: true } : {}),
      ...(report.aborted === true ? { aborted: true } : {}),
      ...(report.crashed ? { crashed: true } : {}),
      inputs: [...answers],
      entry,
    },
  });
}

/** Runs the project's entry file from the start. */
export async function run(): Promise<void> {
  if (runtime.get().running) await stop();
  const given = settings.get().stdin;
  answers = given === '' ? [] : given.replace(/\n$/, '').split('\n');
  ui.set({ panelOpen: true, panelTab: 'console' });
  runtime.set({ run: null });
  await execute();
}

/** The user typed a line for a program that asked for input: run again with it. */
export async function submitInput(line: string): Promise<void> {
  answers.push(line);
  await execute();
}

export async function stop(): Promise<void> {
  stopProgram();
  const last = runtime.get().run;
  runtime.set({
    running: false,
    run: { code: -1, stdout: last?.stdout ?? '', needsInput: false, stderr: '', stats: '', ms: 0, aborted: true, inputs: [...answers], entry: project.get().entry },
  });
}

/** Forgets the input given so far (a new project was opened). */
export function resetInput(): void {
  answers = [];
}

/** `neon check`: the rendered diagnostics in the console. */
export async function checkProject(): Promise<void> {
  await flush();
  const { entry } = project.get();
  const r = await check(filesMap(), entry);
  runtime.set({
    run: { code: r.code, stdout: '', needsInput: false, stderr: r.stderr, stats: '', ms: 0, check: true, inputs: [], entry },
  });
  ui.set({ panelOpen: true, panelTab: 'console' });
  toast(r.code === 0 ? t('toast.check_ok') : t('toast.check_errors'), r.code === 0 ? 'ok' : 'error');
}
