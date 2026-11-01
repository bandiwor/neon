// The parts of the Language Server Protocol the page uses.

export interface Position {
  readonly line: number;
  /** UTF-16 code units, like JavaScript strings. */
  readonly character: number;
}

export interface Range {
  readonly start: Position;
  readonly end: Position;
}

export interface Location {
  readonly uri: string;
  readonly range: Range;
}

export interface LocationLink {
  readonly targetUri: string;
  readonly targetSelectionRange: Range;
}

export interface TextEdit {
  readonly range: Range;
  readonly newText: string;
}

export interface WorkspaceEdit {
  readonly changes?: Readonly<Record<string, readonly TextEdit[]>>;
  readonly documentChanges?: readonly { readonly textDocument: { readonly uri: string }; readonly edits: readonly TextEdit[] }[];
}

export const DiagnosticSeverity = { Error: 1, Warning: 2, Information: 3, Hint: 4 } as const;
export type DiagnosticSeverity = (typeof DiagnosticSeverity)[keyof typeof DiagnosticSeverity];

export interface Diagnostic {
  readonly range: Range;
  readonly severity?: DiagnosticSeverity;
  readonly code?: string | number;
  readonly source?: string;
  readonly message: string;
}

export interface CompletionItem {
  readonly label: string;
  readonly kind?: number;
  readonly detail?: string;
  readonly sortText?: string;
}

export interface CompletionList {
  readonly items: readonly CompletionItem[];
}

export interface MarkupContent {
  readonly kind: string;
  readonly value: string;
}

export interface Hover {
  readonly contents: MarkupContent | string;
  readonly range?: Range;
}

export interface DocumentSymbol {
  readonly name: string;
  readonly kind: number;
  readonly detail?: string;
  readonly range?: Range;
  readonly selectionRange?: Range;
  readonly location?: { readonly range: Range };
  readonly children?: readonly DocumentSymbol[];
}

export interface CodeAction {
  readonly title: string;
  readonly edit?: WorkspaceEdit;
}

export interface SemanticTokens {
  readonly data: readonly number[];
}

export interface PrepareRenameResult {
  readonly range?: Range;
  readonly placeholder?: string;
}

// ---- JSON-RPC ---------------------------------------------------------------------------------------------------

export interface ResponseMessage {
  readonly jsonrpc: '2.0';
  readonly id: number;
  readonly result?: unknown;
  readonly error?: { readonly code: number; readonly message: string };
}

export interface NotificationMessage {
  readonly jsonrpc: '2.0';
  readonly method: string;
  readonly params?: unknown;
}

export interface PublishDiagnosticsParams {
  readonly uri: string;
  readonly diagnostics: readonly Diagnostic[];
}

export interface InitializeResult {
  readonly capabilities?: {
    readonly semanticTokensProvider?: { readonly legend?: { readonly tokenTypes?: readonly string[] } };
  };
}
