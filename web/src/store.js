// Two tiny stores: what survives a reload (the project, the settings, the layout) and what does not.
import { useEffect, useReducer, useRef } from 'preact/hooks';

function createStore(initial, key) {
  let state = initial;
  const listeners = new Set();
  let timer = 0;
  const persist = () => {
    if (!key) return;
    clearTimeout(timer);
    timer = setTimeout(() => {
      try {
        localStorage.setItem(key, JSON.stringify(state));
      } catch {
        /* private mode, quota: the session just does not survive */
      }
    }, 300);
  };
  return {
    get: () => state,
    set(patch) {
      const next = typeof patch === 'function' ? patch(state) : patch;
      state = { ...state, ...next };
      persist();
      listeners.forEach((l) => l());
    },
    subscribe(l) {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    flush() {
      if (key) localStorage.setItem(key, JSON.stringify(state));
    },
  };
}

function load(key, fallback) {
  try {
    const raw = localStorage.getItem(key);
    if (raw) return { ...fallback, ...JSON.parse(raw) };
  } catch {
    /* damaged: start over */
  }
  return fallback;
}

export const DEFAULT_SETTINGS = {
  theme: 'system', // system | light | dark
  accent: 'ember', // ember | moss | azure | graphite
  lang: 'ru',
  fontSize: 14,
  wrap: false,
  opt: 'default', // default | none | no-inline | no-peephole
  maxSteps: 500_000_000,
  timeout: 20, // seconds
  gcStress: false,
  gcLimit: '',
  stats: true,
  args: '',
  stdin: '',
  panelPos: 'auto', // auto | right | bottom
  autoCheck: true,
};

export const settings = createStore(load('neon-web:settings', DEFAULT_SETTINGS), 'neon-web:settings');

const DEFAULT_PROJECT = {
  files: [
    {
      name: 'main.neon',
      text: `// Добро пожаловать в Neon!  Ctrl+Enter — запустить, Ctrl+Shift+F — отформатировать.

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
`,
    },
  ],
  active: 'main.neon',
  entry: 'main.neon',
};

export const project = createStore(load('neon-web:project', DEFAULT_PROJECT), 'neon-web:project');

export const ui = createStore(
  load('neon-web:ui', {
    sidebar: true,
    sidebarTab: 'files', // files | examples | outline
    panelTab: 'console', // console | problems | ir | bytecode | c | repl
    panelSize: 0.42, // share of the work area
    sidebarWidth: 232,
    showRunOptions: false,
  }),
  'neon-web:ui',
);

/** State that is not kept: the last run, the problems, the cursor. */
export const runtime = createStore({
  running: false,
  run: null, // {stdout, stderr, code, ms, needsInput, inputs, timedOut, aborted, stats}
  problems: {}, // file -> [{from, to, severity, message, code}] (LSP ranges)
  cursor: { line: 1, col: 1 },
  lsp: 'starting', // starting | ready | failed
  version: '',
  dialog: null, // null | 'settings' | 'help' | 'palette' | 'rename' | 'share' | 'new-file'
  dialogData: null,
  toast: null,
  jump: null, // {file, from, to, nonce}: asks the editor to show a place
  outline: [],
});

/** Reads a store in a component; re-renders when the selected part changes. */
export function useStore(store, select = (s) => s) {
  const [, force] = useReducer((n) => n + 1, 0);
  const last = useRef();
  last.current = select(store.get());
  useEffect(() => {
    return store.subscribe(() => {
      const next = select(store.get());
      if (!Object.is(next, last.current)) force();
    });
  }, [store]);
  return last.current;
}

// ---- project helpers ---------------------------------------------------------------------------------------

export function filesMap() {
  return Object.fromEntries(project.get().files.map((f) => [f.name, f.text]));
}

export function getFile(name) {
  return project.get().files.find((f) => f.name === name);
}

export function setFileText(name, text) {
  project.set((p) => ({ files: p.files.map((f) => (f.name === name ? { ...f, text } : f)) }));
}

export function setActive(name) {
  if (getFile(name)) project.set({ active: name });
}

/** A name that is not taken yet: `name.neon`, `name2.neon`, ... */
export function freshName(base = 'module') {
  const taken = new Set(project.get().files.map((f) => f.name));
  let n = 1;
  let name = `${base}.neon`;
  while (taken.has(name)) name = `${base}${++n}.neon`;
  return name;
}

export function validFileName(name) {
  return /^[A-Za-z0-9_][A-Za-z0-9_\-./]*\.neon$/.test(name) && !name.includes('..') && !name.includes('//');
}

export function addFile(name, text = '') {
  project.set((p) => ({ files: [...p.files, { name, text }], active: name }));
}

export function renameFile(from, to) {
  project.set((p) => ({
    files: p.files.map((f) => (f.name === from ? { ...f, name: to } : f)),
    active: p.active === from ? to : p.active,
    entry: p.entry === from ? to : p.entry,
  }));
}

export function removeFile(name) {
  project.set((p) => {
    const files = p.files.filter((f) => f.name !== name);
    if (files.length === 0) return {};
    const entry = p.entry === name ? files[0].name : p.entry;
    const active = p.active === name ? files[0].name : p.active;
    return { files, entry, active };
  });
}

export function setProject(files, entry = 'main.neon') {
  const e = files.some((f) => f.name === entry) ? entry : files[0].name;
  project.set({ files, entry: e, active: e });
}

export function toast(text, kind = 'info', action = null) {
  runtime.set({ toast: { text, kind, action, nonce: Math.random() } });
}
