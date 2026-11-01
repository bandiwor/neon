// The oldest browsers the site supports. The styles use `light-dark()`, which these all have, so the CSS
// compiler has nothing to lower for them.
import type { Targets } from 'lightningcss';

/** Lightning CSS packs a version as `major << 16 | minor << 8 | patch`. */
const v = (major: number, minor = 0): number => (major << 16) | (minor << 8);

export const cssTargets: Targets = {
  chrome: v(123),
  edge: v(123),
  firefox: v(120),
  safari: v(17, 5),
  ios_saf: v(17, 5),
};

/** The same for the JavaScript compiler (esbuild's names). */
export const jsTarget = ['es2022', 'chrome123', 'firefox120', 'safari17.5'] as const;
