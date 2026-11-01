// The application's state: what survives a reload (settings, project, layout) and what does not (runtime).
import type { Project } from '../project/types';
import { createStore } from './store';
import type { Settings, UiState, RuntimeState } from './types';

/** Keeps only the fields of `stored` whose type matches the default (a hand-edited or old record can not break the page). */
function reviveFields<T extends object>(stored: unknown, defaults: T): T {
  if (typeof stored !== 'object' || stored === null) return defaults;
  const out = { ...defaults } as unknown as Record<string, unknown>;
  for (const [key, def] of Object.entries(defaults)) {
    const value = (stored as Record<string, unknown>)[key];
    if (typeof value === typeof def) out[key] = value;
  }
  return out as T;
}

export const DEFAULT_SETTINGS: Settings = {
  theme: 'system',
  accent: 'ember',
  lang: 'ru',
  fontSize: 14,
  wrap: false,
  opt: 'default',
  maxSteps: 500_000_000,
  timeout: 20,
  gcStress: false,
  gcLimit: '',
  stats: true,
  args: '',
  stdin: '',
  panelPos: 'auto',
};

export const settings = createStore<Settings>(DEFAULT_SETTINGS, {
  persistKey: 'neon-web:settings',
  revive: reviveFields,
});

const STARTER = `// Добро пожаловать в Neon!  Ctrl+Enter — запустить, Ctrl+Shift+F — отформатировать.

struct Point { x: i64, y: i64 }

impl Point {
    fn dist2(self) -> i64 { self.x * self.x + self.y * self.y }
}

fn main() {
    let points = [Point { x: 3, y: 4 }, Point { x: 1, y: 1 }, Point { x: 6, y: 8 }];
    for p in points {
        println(f"({p.x}, {p.y}) -> {p.dist2()}");
    }
    let total = points.iter().map(|p| => p.dist2()).sum();
    println(f"сумма квадратов расстояний: {total}");
}
`;

export const DEFAULT_PROJECT: Project = {
  files: [{ name: 'main.neon', text: STARTER }],
  active: 'main.neon',
  entry: 'main.neon',
};

function reviveProject(stored: unknown, defaults: Project): Project {
  if (typeof stored !== 'object' || stored === null) return defaults;
  const { files, active, entry } = stored as Partial<Record<keyof Project, unknown>>;
  if (!Array.isArray(files) || files.length === 0) return defaults;
  const valid = (files as unknown[]).filter(
    (f): f is { name: string; text: string } =>
      typeof f === 'object' && f !== null && typeof (f as { name?: unknown }).name === 'string' && typeof (f as { text?: unknown }).text === 'string',
  );
  if (valid.length === 0) return defaults;
  const names = valid.map((f) => f.name);
  const first = names[0] ?? defaults.entry;
  return {
    files: valid.map((f) => ({ name: f.name, text: f.text })),
    active: typeof active === 'string' && names.includes(active) ? active : first,
    entry: typeof entry === 'string' && names.includes(entry) ? entry : first,
  };
}

export const project = createStore<Project>(DEFAULT_PROJECT, { persistKey: 'neon-web:project', revive: reviveProject });

export const DEFAULT_UI: UiState = {
  sidebar: true,
  sidebarTab: 'files',
  panelOpen: true,
  panelTab: 'console',
  panelSize: 0.42,
  sidebarWidth: 232,
  showRunOptions: false,
};

export const ui = createStore<UiState>(DEFAULT_UI, { persistKey: 'neon-web:ui', revive: reviveFields });

export const runtime = createStore<RuntimeState>({
  running: false,
  run: null,
  problems: {},
  cursor: { line: 1, col: 1 },
  lsp: 'starting',
  version: '',
  dialog: null,
  toast: null,
  jump: null,
});
