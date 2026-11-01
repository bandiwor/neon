import type { JSX } from 'preact';
import { useEffect, useState } from 'preact/hooks';
import { jumpTo } from '@/app/actions';
import { lsp } from '@/core/lsp/client';
import { onSynced } from '@/core/lsp/sync';
import type { DocumentSymbol } from '@/core/lsp/types';
import { project } from '@/core/state/stores';
import { useStore } from '@/core/state/useStore';
import { List, Row, RowMain, cx, listClasses } from '@/ui';
import u from '@/styles/utils.module.scss';
import s from './Sidebar.module.scss';

// a short mark per LSP symbol kind
const KIND: Readonly<Record<number, string>> = { 12: 'fn', 6: 'fn', 5: 'T', 10: 'E', 23: 'S', 11: 'I', 14: 'c', 13: 'v', 8: 'f', 22: 'e', 2: 'm' };

interface Flat extends DocumentSymbol {
  readonly depth: number;
}

function flatten(list: readonly DocumentSymbol[], depth = 0): Flat[] {
  return list.flatMap((sym) => [{ ...sym, depth }, ...flatten(sym.children ?? [], depth + 1)]);
}

/** The declarations of the open file. */
export function OutlineTab(): JSX.Element {
  const active = useStore(project, (p) => p.active);
  const [items, setItems] = useState<readonly DocumentSymbol[]>([]);
  useEffect(() => {
    let alive = true;
    const load = (): void => {
      lsp
        .symbols(active)
        .then((r) => alive && setItems(r ?? []))
        .catch(() => undefined);
    };
    load();
    const off = onSynced((file) => file === active && load());
    return () => {
      alive = false;
      off();
    };
  }, [active]);
  const flat = flatten(items);
  if (flat.length === 0) return <></>;
  return (
    <List>
      {flat.map((sym, i) => {
        const range = sym.selectionRange ?? sym.range ?? sym.location?.range;
        return (
          <Row key={i}>
            <RowMain style={{ paddingLeft: `${8 + sym.depth * 14}px` }} onClick={() => range && jumpTo(active, range)}>
              <span class={s.kind}>{KIND[sym.kind] ?? '·'}</span>
              <span class={listClasses.name}>{sym.name}</span>
              {sym.detail && <span class={cx(u.muted, s.detail)}>{sym.detail}</span>}
            </RowMain>
          </Row>
        );
      })}
    </List>
  );
}
