import { describe, expect, it } from 'vitest';
import { runArgs, splitArgs } from '../../src/core/toolchain/args';
import { DEFAULT_SETTINGS } from '../../src/core/state/stores';

describe('command line', () => {
  it('splits arguments like a shell', () => {
    expect(splitArgs(`one "two words" 'three four' five`)).toEqual(['one', 'two words', 'three four', 'five']);
    expect(splitArgs('')).toEqual([]);
  });

  it('builds `neon run` from the settings', () => {
    expect(runArgs('main.neon', { ...DEFAULT_SETTINGS, maxSteps: 0, stats: false })).toEqual(['run', 'main.neon']);
    expect(
      runArgs('a.neon', { ...DEFAULT_SETTINGS, opt: 'none', maxSteps: 1000, gcStress: true, gcLimit: ' 64m ', stats: true, args: 'x "y z"' }),
    ).toEqual(['run', 'a.neon', '--no-opt', '--max-steps', '1000', '--gc-stress', '--gc-limit', '64m', '--stats', '--', 'x', 'y z']);
  });
});
