// What the rest of the application may ask of the open editor. The editor feature implements it and
// registers itself; actions depend on this interface, not on CodeMirror.
import type { Position } from '../lsp/types';

export interface TextChange {
  readonly from: number;
  readonly to: number;
  readonly insert: string;
}

export interface EditorPort {
  /** The file shown, if any. */
  readonly name: string | null;
  /** The text of the shown file. */
  text(): string;
  /** The caret; the start of the selection when something is selected. */
  caret(): number;
  positionAt(offset: number): Position;
  /** Applies the changes (offsets in the current text) as one undoable step. */
  edit(changes: readonly TextChange[]): void;
  select(from: number, to?: number): void;
  /** Drops what the editor remembers about a file (its undo history). */
  forget(name: string): void;
  forgetAll(): void;
}

export const editorPort: { current: EditorPort | null } = { current: null };
