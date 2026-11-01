import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { afterAll, describe, expect, it } from 'vitest';
import { classesOf, declarationFor, declarationPath, writeDeclaration } from '../../build/plugins/cssModuleTypes';

const dir = mkdtempSync(join(tmpdir(), 'neon-css-'));
afterAll(() => rmSync(dir, { recursive: true, force: true }));
const styles = resolve(import.meta.dirname, '../../src/styles');

describe('CSS module declarations', () => {
  it('lists the local classes of the compiled Sass, not the global ones', () => {
    const file = join(dir, 'A.module.scss');
    writeFileSync(file, '@use "mixins" as *;\n.row { &:hover { color: red; } .nested { x: y } }\n:global(.cm-x) { color: blue; }\n.b { @include ellipsis; }\n@keyframes spin { to { opacity: 0 } }\n');
    expect(classesOf(file, [styles])).toEqual(['b', 'nested', 'row', 'spin']);
  });

  it('writes a typed declaration next to the stylesheet, only when it changes', () => {
    const file = join(dir, 'B.module.scss');
    writeFileSync(file, '.one {a: b} .two-parts {c: d}\n');
    expect(declarationPath(file)).toBe(join(dir, 'B.module.d.scss.ts'));
    expect(writeDeclaration(file, [styles])).toBe(true);
    expect(writeDeclaration(file, [styles])).toBe(false);
    const text = readFileSync(declarationPath(file), 'utf8');
    expect(text).toContain('readonly one: string;');
    expect(text).toContain('readonly "two-parts": string;');
    expect(declarationFor([])).toContain('declare const classes: {\n};');
  });
});
