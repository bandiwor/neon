import { describe, expect, it } from 'vitest';
import { exportedNames, renameTokens, shortName } from '../../build/plugins/shortClassNames';

describe('short class names', () => {
  it('counts a, b, ... Z, aa, ab ... and never repeats or starts with a digit', () => {
    const names = Array.from({ length: 5000 }, (_, i) => shortName(i));
    expect(new Set(names).size).toBe(names.length);
    expect(names.slice(0, 3)).toEqual(['a', 'b', 'c']);
    expect(names[51]).toBe('Z');
    expect(names[52]).toBe('aa');
    expect(names.every((n) => /^[A-Za-z][A-Za-z0-9]*$/.test(n))).toBe(true);
    expect(Math.max(...names.map((n) => n.length))).toBeLessThanOrEqual(3);
  });

  it('reads the names a CSS module exports', () => {
    const code = 'export const btn = "fMTMOW_btn";\nexport const iconOnly = "fMTMOW_iconOnly";\nexport default {\n\tbtn: btn\n};\n';
    expect(exportedNames(code)).toEqual(['fMTMOW_btn', 'fMTMOW_iconOnly']);
  });

  it('renames whole names only, in stylesheets and scripts', () => {
    const names = new Map([
      ['fMTMOW_btn', 'a'],
      ['fMTMOW_btnBig', 'b'],
      ['-G9JGW_app', 'c'],
    ]);
    expect(renameTokens('.fMTMOW_btn{x:1}.fMTMOW_btnBig:hover{}', names)).toBe('.a{x:1}.b:hover{}');
    expect(renameTokens('h("div",{class:"fMTMOW_btn"}),class:"-G9JGW_app"', names)).toBe('h("div",{class:"a"}),class:"c"');
    expect(renameTokens('my-fMTMOW_btn fMTMOW_btn2', names)).toBe('my-fMTMOW_btn fMTMOW_btn2');
  });
});
