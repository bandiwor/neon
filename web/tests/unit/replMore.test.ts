import { describe, expect, it } from 'vitest';
import { needsMore } from '../../src/core/util/replMore';

describe('needsMore', () => {
  it('continues inside brackets, strings and block comments', () => {
    expect(needsMore('fn f() {')).toBe(true);
    expect(needsMore('let a = [1,')).toBe(true);
    expect(needsMore('let s = "abc')).toBe(true);
    expect(needsMore('/* open')).toBe(true);
  });

  it('finishes balanced input, ignoring brackets in strings, chars and comments', () => {
    expect(needsMore('fn f() { 1 }')).toBe(false);
    expect(needsMore('println("}");')).toBe(false);
    expect(needsMore("let c = '{';")).toBe(false);
    expect(needsMore('// {\nlet a = 1;')).toBe(false);
    expect(needsMore('')).toBe(false);
  });
});
