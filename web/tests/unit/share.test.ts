import { describe, expect, it } from 'vitest';
import { decodeProject, encodeProject } from '../../src/core/util/share';

describe('project links', () => {
  const files = [
    { name: 'main.neon', text: 'import "lib/u" as u;\nfn main() { println("привет, мир"); }\n' },
    { name: 'lib/u.neon', text: 'export fn f() {}\n' },
  ];

  it('round-trips a project, Unicode included', async () => {
    const hash = await encodeProject(files, 'main.neon');
    expect(hash).toMatch(/^p=[A-Za-z0-9_-]+$/);
    expect(await decodeProject(`#${hash}`)).toEqual({ files, entry: 'main.neon' });
  });

  it('packs repetitive text small', async () => {
    const text = 'let x = 1;\n'.repeat(500);
    const hash = await encodeProject([{ name: 'main.neon', text }], 'main.neon');
    expect(hash.length).toBeLessThan(text.length / 10);
  });

  it('refuses anything else', async () => {
    for (const bad of ['', '#', '#x=1', '#p=', '#p=!!!', '#p=AAAA', `#p=${btoa('{"f":1}')}`]) {
      expect(await decodeProject(bad), bad).toBeNull();
    }
  });
});
