// Everything the buttons, the shortcuts and the palette do. The editor and the stores stay dumb.
import { project, runtime, settings, ui, filesMap, getFile, setFileText, setActive, addFile, removeFile, renameFile, setProject, toast, freshName, validFileName } from './store.js';
import { lsp } from './lsp.js';
import * as sync from './sync.js';
import * as tc from './toolchain.js';
import { t, count } from './i18n.js';
import { encodeProject } from './share.js';

/** Set by the editor component: the controller of the open editor. */
export const editorRef = { current: null };

// ---- positions in plain text --------------------------------------------------------------------------------

export function offsetIn(text, pos) {
  let offset = 0;
  for (let line = 0; line < pos.line; line++) {
    const next = text.indexOf('\n', offset);
    if (next < 0) return text.length;
    offset = next + 1;
  }
  const end = text.indexOf('\n', offset);
  return Math.min(offset + pos.character, end < 0 ? text.length : end);
}

/** Applies LSP text edits (positions in the original text) to `text`. */
export function applyEdits(text, edits) {
  const spans = edits
    .map((e) => ({ from: offsetIn(text, e.range.start), to: offsetIn(text, e.range.end), insert: e.newText }))
    .sort((a, b) => b.from - a.from);
  for (const s of spans) text = text.slice(0, s.from) + s.insert + text.slice(s.to);
  return text;
}

function editsToChanges(text, edits) {
  return edits.map((e) => ({ from: offsetIn(text, e.range.start), to: offsetIn(text, e.range.end), insert: e.newText }));
}

/** Applies a WorkspaceEdit to the project (the open file through the editor, so undo works). */
export function applyWorkspaceEdit(edit) {
  const byUri = {};
  for (const [uri, edits] of Object.entries(edit?.changes ?? {})) byUri[uri] = edits;
  for (const dc of edit?.documentChanges ?? []) if (dc.textDocument) byUri[dc.textDocument.uri] = dc.edits;
  let n = 0;
  for (const [uri, edits] of Object.entries(byUri)) {
    const name = decodeURIComponent(uri.replace(/^file:\/\/\//, ''));
    const file = getFile(name);
    if (!file) continue;
    n += edits.length;
    if (editorRef.current?.name === name) {
      editorRef.current.edit(editsToChanges(editorRef.current.view.state.doc.toString(), edits));
    } else {
      const text = applyEdits(file.text, edits);
      setFileText(name, text);
      editorRef.current?.forget(name);
      sync.scheduleChange(name, text);
    }
  }
  return n;
}

// ---- running ------------------------------------------------------------------------------------------------

let answers = []; // lines the program has been given so far

function splitStats(stderr) {
  const stats = [];
  const kept = [];
  for (const line of stderr.split('\n')) (line.startsWith('gc: ') ? stats : kept).push(line);
  return { stderr: kept.join('\n'), stats: stats.join('\n') };
}

async function execute() {
  await sync.flush();
  const p = project.get();
  const s = settings.get();
  runtime.set({ running: true });
  const input = answers.length ? answers.join('\n') + '\n' : '';
  const res = await tc.runProgram({ files: filesMap(), entry: p.entry, settings: s, answers: input, interactive: true });
  if (res.aborted === 'replaced') return; // another run took over
  // a program that stopped to ask for input did not fail: what the module reports then is only that
  const { stderr, stats } = splitStats(res.needsInput ? '' : (res.stderr ?? ''));
  runtime.set({ running: false, run: { ...res, stderr, stats, inputs: [...answers], entry: p.entry, at: Date.now() } });
}

/** Runs the project's entry file from the start. */
export async function run() {
  if (runtime.get().running) await stop();
  answers = (settings.get().stdin ?? '').split('\n');
  if (answers.at(-1) === '') answers.pop();
  if (settings.get().stdin === '') answers = [];
  ui.set({ panelTab: 'console' });
  runtime.set({ run: null });
  await execute();
}

/** The user typed a line for a program that asked for input: run again with it. */
export async function submitInput(line) {
  answers.push(line);
  await execute();
}

export async function stop() {
  tc.stopProgram();
  runtime.set({ running: false, run: { ...(runtime.get().run ?? {}), stdout: runtime.get().run?.stdout ?? '', stderr: '', aborted: true, code: -1, ms: 0, inputs: [...answers] } });
}

// ---- formatting, checking ---------------------------------------------------------------------------------------

export async function formatFile(name = project.get().active) {
  await sync.flush();
  try {
    const edits = await lsp.formatting(name);
    if (!edits || edits.length === 0) {
      toast(t('toast.formatted_already'));
      return false;
    }
    const file = getFile(name);
    if (editorRef.current?.name === name) editorRef.current.edit(editsToChanges(editorRef.current.view.state.doc.toString(), edits));
    else {
      setFileText(name, applyEdits(file.text, edits));
      editorRef.current?.forget(name);
    }
    toast(t('toast.formatted'), 'ok');
    return true;
  } catch (e) {
    const r = await tc.format(filesMap(), name).catch(() => null);
    toast(r && !r.ok ? t('toast.format_failed') : String(e.message), 'error');
    ui.set({ panelTab: 'problems' });
    return false;
  }
}

export async function formatAll() {
  let n = 0;
  for (const f of project.get().files) if (await formatFile(f.name)) n++;
  toast(t('toast.formatted_n', { n }), 'ok');
}

/** `neon check`: shows the rendered diagnostics in the console. */
export async function checkProject() {
  await sync.flush();
  const p = project.get();
  const r = await tc.check(filesMap(), p.entry);
  runtime.set({
    run: { stdout: '', stderr: r.stderr, code: r.code, ms: 0, check: true, inputs: [], entry: p.entry, at: Date.now() },
  });
  ui.set({ panelTab: 'console' });
  toast(r.code === 0 ? t('toast.check_ok') : t('toast.check_errors'), r.code === 0 ? 'ok' : 'error');
}

// ---- navigation -------------------------------------------------------------------------------------------------

export function jumpTo(file, range) {
  if (!getFile(file)) return;
  setActive(file);
  const text = getFile(file).text;
  const from = offsetIn(text, range.start);
  const to = offsetIn(text, range.end);
  runtime.set({ jump: { file, from, to, nonce: Math.random() } });
}

/** Jumps to a `file:line:col` seen in the console. */
export function jumpToPlace(file, line, col) {
  const f = getFile(file) ? file : project.get().entry;
  jumpTo(f, { start: { line: line - 1, character: col - 1 }, end: { line: line - 1, character: col - 1 } });
}

export async function goToDefinition() {
  const ed = editorRef.current;
  if (!ed?.name) return;
  await sync.flush();
  const { offsetToPosition } = await import('./editor.js');
  try {
    const res = await lsp.definition(ed.name, offsetToPosition(ed.view.state.doc, ed.cursorOffset()));
    const loc = Array.isArray(res) ? res[0] : res;
    if (!loc) return toast(t('toast.no_definition'));
    const uri = loc.uri ?? loc.targetUri;
    const range = loc.range ?? loc.targetSelectionRange;
    const file = decodeURIComponent(uri.replace(/^file:\/\/\//, ''));
    if (!getFile(file)) return toast(t('toast.definition_in_library'));
    jumpTo(file, range);
  } catch (e) {
    toast(e.message, 'error');
  }
}

export async function startRename() {
  const ed = editorRef.current;
  if (!ed?.name) return;
  await sync.flush();
  const { offsetToPosition } = await import('./editor.js');
  try {
    const pos = offsetToPosition(ed.view.state.doc, ed.cursorOffset());
    const r = await lsp.prepareRename(ed.name, pos);
    if (!r) return toast(t('toast.cannot_rename'));
    const range = r.range ?? r;
    const doc = ed.view.state.doc.toString();
    const placeholder = r.placeholder ?? doc.slice(offsetIn(doc, range.start), offsetIn(doc, range.end));
    runtime.set({ dialog: 'rename', dialogData: { pos, placeholder } });
  } catch {
    toast(t('toast.cannot_rename'));
  }
}

export async function commitRename(newName) {
  const { pos } = runtime.get().dialogData;
  const ed = editorRef.current;
  try {
    const edit = await lsp.rename(ed.name, pos, newName);
    const n = applyWorkspaceEdit(edit);
    toast(t('toast.renamed', { n }), 'ok');
  } catch (e) {
    toast(e.message, 'error');
  }
  runtime.set({ dialog: null });
}

// ---- project ------------------------------------------------------------------------------------------------------

export function createFile(name) {
  if (!validFileName(name)) return toast(t('toast.bad_file_name'), 'error');
  if (getFile(name)) return toast(t('toast.file_exists'), 'error');
  addFile(name, '');
  sync.syncProject(project.get().files);
}

export function deleteFile(name) {
  if (project.get().files.length <= 1) return toast(t('toast.last_file'), 'error');
  removeFile(name);
  editorRef.current?.forget(name);
  sync.syncProject(project.get().files);
}

export function moveFile(from, to) {
  if (from === to) return;
  if (!validFileName(to)) return toast(t('toast.bad_file_name'), 'error');
  if (getFile(to)) return toast(t('toast.file_exists'), 'error');
  renameFile(from, to);
  editorRef.current?.forgetAll();
  sync.syncProject(project.get().files);
}

/** Replaces the project (an example, a shared link, an import); the old one can be brought back. */
export function openProject(files, entry, title) {
  try {
    localStorage.setItem('neon-web:backup', JSON.stringify(project.get()));
  } catch {
    /* no backup */
  }
  setProject(files, entry);
  editorRef.current?.forgetAll();
  answers = [];
  runtime.set({ run: null, problems: {} });
  sync.syncProject(project.get().files);
  toast(t('toast.opened', { title }), 'info', {
    label: t('toast.undo'),
    run: restoreBackup,
  });
}

export function restoreBackup() {
  try {
    const b = JSON.parse(localStorage.getItem('neon-web:backup'));
    if (b?.files?.length) {
      project.set(b);
      editorRef.current?.forgetAll();
      sync.syncProject(project.get().files);
    }
  } catch {
    /* nothing to restore */
  }
}

export async function shareLink() {
  const p = project.get();
  const hash = await encodeProject(p.files, p.entry);
  const url = `${location.origin}${location.pathname}#${hash}`;
  try {
    await navigator.clipboard.writeText(url);
    toast(t('toast.link_copied'), 'ok');
  } catch {
    runtime.set({ dialog: 'share', dialogData: { url } });
  }
  history.replaceState(null, '', `#${hash}`);
  return url;
}

export function download(name, text, type = 'text/plain') {
  const url = URL.createObjectURL(new Blob([text], { type }));
  const a = document.createElement('a');
  a.href = url;
  a.download = name;
  a.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

export function downloadActive() {
  const f = getFile(project.get().active);
  download(f.name.split('/').at(-1), f.text);
}

export async function importFiles(fileList) {
  const files = [];
  for (const f of fileList) {
    const text = await f.text();
    if (f.name.endsWith('.json')) {
      try {
        const data = JSON.parse(text);
        if (Array.isArray(data.files)) return openProject(data.files, data.entry, f.name);
      } catch {
        /* not a project: treat as a file below */
      }
    }
    const name = f.name.endsWith('.neon') ? f.name : `${f.name}.neon`;
    files.push({ name: validFileName(name) ? name : freshName('imported'), text });
  }
  if (files.length) openProject(files, files[0].name, count(files.length, ['файл', 'файла', 'файлов']));
}

export function exportProjectJson() {
  const p = project.get();
  download('neon-project.json', JSON.stringify({ files: p.files, entry: p.entry }, null, 2), 'application/json');
}

export function stepLabel(n) {
  return count(n, ['шаг', 'шага', 'шагов']);
}
