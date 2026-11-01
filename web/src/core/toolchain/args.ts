import { DEFAULT_OPT, DEFAULT_SETTINGS } from '../state/stores';
import type { OptFlags, Settings } from '../state/types';

/** The `neon` flags that switch off what the optimizer settings switched off. */
export function optFlags(opt: OptFlags): string[] {
  const flags: string[] = [];
  if (!opt.fold) flags.push('--no-fold');
  if (!opt.dce) flags.push('--no-dce');
  if (!opt.simplify) flags.push('--no-simplify');
  if (!opt.licm) flags.push('--no-licm');
  if (!opt.rotate) flags.push('--no-rotate');
  if (!opt.unroll) flags.push('--no-unroll');
  if (!opt.inline) flags.push('--no-inline');
  else {
    if (opt.inlineThreshold !== DEFAULT_OPT.inlineThreshold) flags.push('--inline-threshold', String(opt.inlineThreshold));
    if (opt.maxRegs !== DEFAULT_OPT.maxRegs) flags.push('--max-regs', String(opt.maxRegs));
  }
  if (!opt.peephole) flags.push('--no-peephole');
  return flags;
}

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
  if (s.maxFrames > 0 && s.maxFrames !== DEFAULT_SETTINGS.maxFrames) args.push('--max-frames', String(s.maxFrames));
  if (s.gcStress) args.push('--gc-stress');
  const limit = s.gcLimit.trim();
  if (limit) args.push('--gc-limit', limit);
  if (s.stats) args.push('--stats');
  const program = splitArgs(s.args);
  if (program.length > 0) args.push('--', ...program);
  return args;
}
