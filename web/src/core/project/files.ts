// Operations on the project store.
import { project } from '../state/stores';
import type { Project, SourceFile } from './types';

export function getFile(name: string): SourceFile | undefined {
  return project.get().files.find((f) => f.name === name);
}

/** The files as the module wants them: path -> text. */
export function filesMap(): Record<string, string> {
  return Object.fromEntries(project.get().files.map((f) => [f.name, f.text]));
}

export function setFileText(name: string, text: string): void {
  project.set((p) => ({ files: p.files.map((f) => (f.name === name ? { ...f, text } : f)) }));
}

export function setActive(name: string): void {
  if (getFile(name)) project.set({ active: name });
}

/** A name that is not taken yet: `name.neon`, `name2.neon`, ... */
export function freshName(base = 'module'): string {
  const taken = new Set(project.get().files.map((f) => f.name));
  let n = 1;
  let name = `${base}.neon`;
  while (taken.has(name)) name = `${base}${++n}.neon`;
  return name;
}

export function validFileName(name: string): boolean {
  return /^[A-Za-z0-9_][A-Za-z0-9_\-./]*\.neon$/.test(name) && !name.includes('..') && !name.includes('//');
}

export function addFile(name: string, text = ''): void {
  project.set((p) => ({ files: [...p.files, { name, text }], active: name }));
}

export function renameFile(from: string, to: string): void {
  project.set((p) => ({
    files: p.files.map((f) => (f.name === from ? { ...f, name: to } : f)),
    active: p.active === from ? to : p.active,
    entry: p.entry === from ? to : p.entry,
  }));
}

export function removeFile(name: string): void {
  project.set((p): Partial<Project> => {
    const files = p.files.filter((f) => f.name !== name);
    const first = files[0];
    if (!first) return {};
    return {
      files,
      entry: p.entry === name ? first.name : p.entry,
      active: p.active === name ? first.name : p.active,
    };
  });
}

/** Replaces the project. */
export function setProject(files: readonly SourceFile[], entry = 'main.neon'): void {
  const first = files[0];
  if (!first) return;
  const e = files.some((f) => f.name === entry) ? entry : first.name;
  project.set({ files, entry: e, active: e });
}
