// `import examples from 'virtual:neon-examples'`: the example programs, read from the repository at build
// time. data/examples.json lists them (id, group, title, description, where the source is).
import { existsSync, readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import type { Plugin } from 'vite';
import type { Example } from '../../src/core/project/types';

const ID = 'virtual:neon-examples';
const RESOLVED = `\0${ID}`;

interface ManifestEntry {
  readonly id: string;
  readonly group: string;
  readonly title: string;
  readonly description: string;
  /** Path of the source, from the repository root. */
  readonly source: string;
  readonly interactive?: boolean;
}

function isManifest(data: unknown): data is readonly ManifestEntry[] {
  return (
    Array.isArray(data) &&
    data.every(
      (e: unknown) =>
        typeof e === 'object' &&
        e !== null &&
        ['id', 'group', 'title', 'description', 'source'].every((k) => typeof (e as Record<string, unknown>)[k] === 'string'),
    )
  );
}

export interface ExamplesOptions {
  /** The web/ directory. */
  readonly root: string;
  /** The manifest, relative to `root`. */
  readonly manifest?: string;
}

export function examples({ root, manifest = 'data/examples.json' }: ExamplesOptions): Plugin {
  const manifestPath = resolve(root, manifest);
  const repo = resolve(root, '..');
  return {
    name: 'neon:examples',
    resolveId: (id) => (id === ID ? RESOLVED : null),
    load(id) {
      if (id !== RESOLVED) return null;
      this.addWatchFile(manifestPath);
      const data: unknown = JSON.parse(readFileSync(manifestPath, 'utf8'));
      if (!isManifest(data)) this.error(`${manifest}: not a list of examples`);
      const list: Example[] = data.map((e) => {
        const file = resolve(repo, e.source);
        if (!existsSync(file)) this.error(`${manifest}: the source of "${e.id}" is missing: ${e.source}`);
        this.addWatchFile(file);
        return {
          id: e.id,
          group: e.group,
          title: e.title,
          description: e.description,
          interactive: e.interactive === true,
          files: [{ name: 'main.neon', text: readFileSync(file, 'utf8') }],
        };
      });
      return `export default ${JSON.stringify(list)};`;
    },
  };
}
