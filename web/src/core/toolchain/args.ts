import type { OptLevel, Settings } from '../state/types';

const OPT_FLAGS: Readonly<Record<OptLevel, readonly string[]>> = {
  default: [],
  none: ['--no-opt'],
  'no-inline': ['--no-inline'],
  'no-peephole': ['--no-peephole'],
};

export const optFlags = (opt: OptLevel): readonly string[] => OPT_FLAGS[opt];

/** Shell-like words: plain, "double" and 'single' quoted. */
export function splitArgs(text: string): string[] {
  const out: string[] = [];
  const re = /"([^"]*)"|'([^']*)'|(\S+)/g;
  for (let m = re.exec(text); m; m = re.exec(text)) out.push(m[1] ?? m[2] ?? m[3] ?? '');
  return out;
}

/** The arguments of `neon run` for the settings. */
export function runArgs(entry: string, s: Settings): string[] {
  const args = ['run', entry, ...optFlags(s.opt)];
  if (s.maxSteps > 0) args.push('--max-steps', String(s.maxSteps));
  if (s.gcStress) args.push('--gc-stress');
  const limit = s.gcLimit.trim();
  if (limit) args.push('--gc-limit', limit);
  if (s.stats) args.push('--stats');
  const program = splitArgs(s.args);
  if (program.length > 0) args.push('--', ...program);
  return args;
}
