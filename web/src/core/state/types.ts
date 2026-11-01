import type { CliReply } from '../wasm/protocol';
import type { Diagnostic, Position } from '../lsp/types';
import type { Project } from '../project/types';

export type Theme = 'system' | 'light' | 'dark';
export type Accent = 'ember' | 'moss' | 'azure' | 'graphite';
export type PanelPosition = 'auto' | 'right' | 'bottom';
export type SyntaxStyle = 'neon' | 'quiet' | 'contrast';
export type LineHeight = 'tight' | 'normal' | 'loose';

/** The passes of the optimizer, one by one (`neon --no-fold`, `--inline-threshold`, ...). */
export interface OptFlags {
  readonly fold: boolean;
  readonly dce: boolean;
  readonly simplify: boolean;
  readonly licm: boolean;
  readonly rotate: boolean;
  readonly unroll: boolean;
  readonly inline: boolean;
  readonly peephole: boolean;
  /** Callees of at most this many instructions are inlined. */
  readonly inlineThreshold: number;
  readonly maxRegs: number;
}

export interface Settings {
  // appearance
  readonly theme: Theme;
  readonly accent: Accent;
  readonly lang: 'ru';
  readonly panelPos: PanelPosition;
  // editor
  readonly fontSize: number;
  readonly lineHeight: LineHeight;
  readonly tabSize: number;
  readonly wrap: boolean;
  readonly lineNumbers: boolean;
  readonly activeLine: boolean;
  readonly folding: boolean;
  readonly closeBrackets: boolean;
  readonly matchBrackets: boolean;
  readonly syntax: SyntaxStyle;
  // compiler
  readonly opt: OptFlags;
  // virtual machine
  /** VM instruction limit; 0 is none. */
  readonly maxSteps: number;
  readonly maxFrames: number;
  /** Seconds before a program is stopped; 0 is never. */
  readonly timeout: number;
  readonly gcStress: boolean;
  readonly gcLimit: string;
  readonly stats: boolean;
  // program
  readonly args: string;
  readonly stdin: string;
}

export type SidebarTab = 'files' | 'examples' | 'outline';
export type PanelTab = 'console' | 'problems' | 'ir' | 'bytecode' | 'c' | 'repl';

export interface UiState {
  readonly sidebar: boolean;
  readonly sidebarTab: SidebarTab;
  readonly panelOpen: boolean;
  readonly panelTab: PanelTab;
  /** The share of the work area the panel takes (0..1). */
  readonly panelSize: number;
  readonly sidebarWidth: number;
}

/** What the last run (or check) left in the console. */
export interface RunOutcome extends Pick<CliReply, 'code' | 'stdout' | 'needsInput'> {
  readonly stderr: string;
  readonly stats: string;
  readonly ms: number;
  readonly timedOut?: boolean;
  readonly aborted?: boolean;
  readonly crashed?: boolean;
  readonly check?: boolean;
  /** The lines of input the program was given. */
  readonly inputs: readonly string[];
  readonly entry: string;
}

export type ToastKind = 'info' | 'ok' | 'error';

export interface Toast {
  readonly text: string;
  readonly kind: ToastKind;
  readonly action: { readonly label: string; readonly run: () => void } | null;
  readonly nonce: number;
}

/** The open dialog, with what it needs. */
export type DialogState =
  | { readonly kind: 'settings' }
  | { readonly kind: 'help' }
  | { readonly kind: 'palette' }
  | { readonly kind: 'rename'; readonly position: Position; readonly placeholder: string }
  | { readonly kind: 'share'; readonly url: string };

export type LspStatus = 'starting' | 'ready' | 'failed';

/** A request to the editor: show this place of this file. */
export interface JumpRequest {
  readonly file: string;
  readonly from: number;
  readonly to: number;
  readonly nonce: number;
}

export interface RuntimeState {
  readonly running: boolean;
  readonly run: RunOutcome | null;
  /** File name -> what the language server found in it. */
  readonly problems: Readonly<Record<string, readonly Diagnostic[]>>;
  readonly cursor: { readonly line: number; readonly col: number };
  readonly lsp: LspStatus;
  readonly version: string;
  readonly dialog: DialogState | null;
  readonly toast: Toast | null;
  readonly jump: JumpRequest | null;
}

export type { Project };
