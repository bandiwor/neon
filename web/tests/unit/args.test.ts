import { describe, expect, it } from 'vitest';
import { optFlags, runArgs, splitArgs } from '../../src/core/toolchain/args';
import { DEFAULT_OPT, DEFAULT_SETTINGS, reviveSettings } from '../../src/core/state/stores';

describe('command line', () => {
  it('splits arguments like a shell', () => {
    expect(splitArgs(`one "two words" 'three four' five`)).toEqual(['one', 'two words', 'three four', 'five']);
    expect(splitArgs('')).toEqual([]);
  });

  it('builds `neon run` from the settings', () => {
    expect(runArgs('main.neon', { ...DEFAULT_SETTINGS, maxSteps: 0, stats: false })).toEqual(['run', 'main.neon']);
    expect(
      runArgs('a.neon', { ...DEFAULT_SETTINGS, maxSteps: 1000, maxFrames: 50, gcStress: true, gcLimit: ' 64m ', stats: true, args: 'x "y z"' }),
    ).toEqual(['run', 'a.neon', '--max-steps', '1000', '--max-frames', '50', '--gc-stress', '--gc-limit', '64m', '--stats', '--', 'x', 'y z']);
  });

  it('turns the optimizer settings into flags', () => {
    expect(optFlags(DEFAULT_OPT)).toEqual([]);
    expect(optFlags({ ...DEFAULT_OPT, fold: false, licm: false, peephole: false })).toEqual(['--no-fold', '--no-licm', '--no-peephole']);
    expect(optFlags({ ...DEFAULT_OPT, inlineThreshold: 80, maxRegs: 300 })).toEqual(['--inline-threshold', '80', '--max-regs', '300']);
    // the thresholds mean nothing without inlining
    expect(optFlags({ ...DEFAULT_OPT, inline: false, inlineThreshold: 80 })).toEqual(['--no-inline']);
  });
});

describe('stored settings', () => {
  it('fills what an older record lacks and drops what has the wrong type', () => {
    const old = { theme: 'dark', fontSize: 'big', opt: { fold: false, inlineThreshold: 'x' } };
    const got = reviveSettings(old, DEFAULT_SETTINGS);
    expect(got.theme).toBe('dark');
    expect(got.fontSize).toBe(DEFAULT_SETTINGS.fontSize);
    expect(got.opt).toEqual({ ...DEFAULT_OPT, fold: false });
    expect(got.closeBrackets).toBe(true);
    expect(reviveSettings(null, DEFAULT_SETTINGS)).toEqual(DEFAULT_SETTINGS);
    // the old shape (`opt` was a name)
    expect(reviveSettings({ opt: 'none' }, DEFAULT_SETTINGS).opt).toEqual(DEFAULT_OPT);
  });
});
