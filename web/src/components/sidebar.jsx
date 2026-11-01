import { useEffect, useState } from 'preact/hooks';
import { Button, FocusInput } from './widgets.jsx';
import { Icon } from './icons.jsx';
import { project, runtime, ui, useStore, setActive, getFile, freshName } from '../store.js';
import { t } from '../i18n.js';
import { createFile, deleteFile, jumpTo, moveFile, openProject } from '../actions.js';
import { lsp } from '../lsp.js';
import { onSynced } from '../sync.js';

export function Sidebar() {
  const tab = useStore(ui, (u) => u.sidebarTab);
  const tabs = [
    ['files', 'file', t('sidebar.files')],
    ['examples', 'book', t('sidebar.examples')],
    ['outline', 'list', t('sidebar.outline')],
  ];
  return (
    <aside class="sidebar" aria-label={t('sidebar.label')}>
      <div class="tabs" role="tablist">
        {tabs.map(([id, icon, label]) => (
          <button type="button" role="tab" aria-selected={tab === id} class={`tab ${tab === id ? 'is-active' : ''}`} onClick={() => ui.set({ sidebarTab: id })} title={label}>
            <Icon name={icon} />
            <span>{label}</span>
          </button>
        ))}
      </div>
      <div class="sidebar-body">
        {tab === 'files' && <Files />}
        {tab === 'examples' && <Examples />}
        {tab === 'outline' && <Outline />}
      </div>
    </aside>
  );
}

function Files() {
  const files = useStore(project, (p) => p.files);
  const active = useStore(project, (p) => p.active);
  const entry = useStore(project, (p) => p.entry);
  const problems = useStore(runtime, (s) => s.problems);
  const [renaming, setRenaming] = useState(null);
  const [adding, setAdding] = useState(null);

  const commitRename = (from, to) => {
    setRenaming(null);
    if (to && to !== from) moveFile(from, to);
  };

  return (
    <div class="files">
      <div class="section-head">
        <span>{t('files.title')}</span>
        <Button icon="plus" title={t('files.new')} onClick={() => setAdding(freshName('module'))} />
      </div>
      <ul class="list" role="list">
        {files.map((f) => {
          const errors = (problems[f.name] ?? []).filter((d) => d.severity === 1).length;
          return (
            <li key={f.name} class={`row ${f.name === active ? 'is-active' : ''}`}>
              {renaming === f.name ? (
                <FocusInput
                  class="inline-input"
                  defaultValue={f.name}
                  onBlur={(e) => commitRename(f.name, e.currentTarget.value.trim())}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') e.currentTarget.blur();
                    if (e.key === 'Escape') setRenaming(null);
                  }}
                />
              ) : (
                <button type="button" class="row-main" onClick={() => setActive(f.name)} onDblClick={() => setRenaming(f.name)} title={f.name}>
                  <Icon name="file" />
                  <span class="row-name">{f.name}</span>
                  {f.name === entry && <span class="badge" title={t('files.entry')}>{t('files.entry_short')}</span>}
                  {errors > 0 && <span class="dot dot-error" title={t('problems.errors', { n: errors })} />}
                </button>
              )}
              <span class="row-actions">
                {f.name !== entry && <Button icon="flag" title={t('files.make_entry')} onClick={() => project.set({ entry: f.name })} />}
                <Button icon="edit" title={t('files.rename')} onClick={() => setRenaming(f.name)} />
                <Button icon="trash" title={t('files.delete')} onClick={() => confirm(t('files.confirm_delete', { name: f.name })) && deleteFile(f.name)} />
              </span>
            </li>
          );
        })}
        {adding !== null && (
          <li class="row">
            <FocusInput
              class="inline-input"
              value={adding}
              onInput={(e) => setAdding(e.currentTarget.value)}
              onBlur={() => setAdding(null)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') {
                  createFile(adding.trim());
                  setAdding(null);
                }
                if (e.key === 'Escape') setAdding(null);
              }}
            />
          </li>
        )}
      </ul>
      <p class="hint">{t('files.hint')}</p>
    </div>
  );
}

function Examples() {
  const [list, setList] = useState(null);
  useEffect(() => {
    fetch('examples.json')
      .then((r) => r.json())
      .then(setList)
      .catch(() => setList([]));
  }, []);
  if (!list) return <p class="hint">{t('common.loading')}</p>;
  const groups = [...new Set(list.map((e) => e.group))];
  return (
    <div class="examples">
      {groups.map((g) => (
        <section>
          <div class="section-head">
            <span>{g}</span>
          </div>
          <ul class="list" role="list">
            {list
              .filter((e) => e.group === g)
              .map((e) => (
                <li class="row">
                  <button type="button" class="row-main example" onClick={() => openProject(e.files, 'main.neon', e.title)} title={e.description}>
                    <span class="row-name">{e.title}</span>
                    {e.interactive && <span class="badge badge-accent">{t('examples.interactive')}</span>}
                    <span class="row-sub">{e.description}</span>
                  </button>
                </li>
              ))}
          </ul>
        </section>
      ))}
    </div>
  );
}

const KIND = { 12: 'fn', 6: 'fn', 5: 'T', 10: 'E', 23: 'S', 11: 'I', 14: 'c', 13: 'v', 8: 'f', 22: 'e', 2: 'm' };

function Outline() {
  const active = useStore(project, (p) => p.active);
  const [items, setItems] = useState([]);
  useEffect(() => {
    let alive = true;
    const load = () =>
      lsp
        .symbols(active)
        .then((r) => alive && setItems(Array.isArray(r) ? r : []))
        .catch(() => {});
    load();
    const off = onSynced((name) => name === active && load());
    return () => {
      alive = false;
      off();
    };
  }, [active]);
  const flat = [];
  const walk = (list, depth) => {
    for (const s of list) {
      flat.push({ ...s, depth });
      if (s.children) walk(s.children, depth + 1);
    }
  };
  walk(items, 0);
  if (flat.length === 0) return <p class="hint">{t('outline.empty')}</p>;
  return (
    <ul class="list" role="list">
      {flat.map((s) => (
        <li class="row">
          <button type="button" class="row-main" style={{ paddingLeft: `${8 + s.depth * 14}px` }} onClick={() => jumpTo(active, s.selectionRange ?? s.range ?? s.location?.range)}>
            <span class="kind">{KIND[s.kind] ?? '·'}</span>
            <span class="row-name">{s.name}</span>
            {s.detail && <span class="row-sub inline">{s.detail}</span>}
          </button>
        </li>
      ))}
    </ul>
  );
}
