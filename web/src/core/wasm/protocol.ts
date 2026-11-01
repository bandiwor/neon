// The JSON interface of the Neon module (see wasm/src/lib.rs for the other side).

/** Project files by path. */
export type Files = Readonly<Record<string, string>>;

export interface CliReply {
  /** The exit code of the command line: 0 ok, 1 compile errors, 2 run-time error. */
  readonly code: number;
  readonly stdout: string;
  readonly stderr: string;
  /** The program asked for more input than it was given. */
  readonly needsInput: boolean;
  /** What it asked for was a key (`term.read_key()`), not a line. */
  readonly needsKey: boolean;
}

export interface ReplEvalReply {
  readonly kind: 'ok' | 'rejected' | 'trapped';
  readonly text: string;
  readonly needsInput: boolean;
}

/** `[from, to, class]` in characters. */
export type HighlightRun = readonly [from: number, to: number, cls: string];

export interface ReplCandidate {
  readonly label: string;
  readonly detail: string;
}

/** Every operation: what it takes and what it answers. */
export interface OpTable {
  version: { req: Record<never, never>; reply: { readonly version: string } };
  cli: {
    req: {
      readonly args: readonly string[];
      readonly files: Files;
      readonly stdin: string;
      readonly interactive: boolean;
      /** `stdin` is the program's input byte for byte (keys), not lines. */
      readonly exact?: boolean;
    };
    reply: CliReply;
  };
  /** One JSON-RPC message in; the replies and notifications out (already parsed). */
  lsp: { req: { readonly message: string }; reply: { readonly messages: readonly unknown[] } };
  repl_eval: { req: { readonly cell: string; readonly stdin?: string }; reply: ReplEvalReply };
  repl_reset: { req: Record<never, never>; reply: { readonly cells: number } };
  repl_cells: { req: Record<never, never>; reply: { readonly cells: readonly string[] } };
  repl_more: { req: { readonly text: string }; reply: { readonly more: boolean } };
  repl_complete: {
    req: { readonly buffer: string; readonly cursor: number };
    reply: { readonly start: number; readonly items: readonly ReplCandidate[] };
  };
  repl_preview: { req: { readonly cell: string }; reply: { readonly value: string | null } };
  repl_highlight: { req: { readonly text: string }; reply: { readonly runs: readonly HighlightRun[] } };
}

export type Op = keyof OpTable;
export type RequestOf<K extends Op> = OpTable[K]['req'];
export type ReplyOf<K extends Op> = OpTable[K]['reply'];

/** The module answers a request it cannot serve with `{error}`. */
export interface ModuleError {
  readonly error: string;
}

// ---- the worker's messages -------------------------------------------------------------------------------

export type WorkerIn =
  | { readonly type: 'init'; readonly module: WebAssembly.Module }
  | { readonly type: 'call'; readonly id: number; readonly op: Op; readonly payload: object };

export type WorkerOut =
  | { readonly type: 'ready' }
  | { readonly type: 'result'; readonly id: number; readonly result: unknown }
  | { readonly type: 'error'; readonly id: number; readonly message: string; readonly crashed: boolean };
