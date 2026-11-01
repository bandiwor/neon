// A client for the language server that runs inside the module (see wasm/src/lib.rs): JSON-RPC messages go
// in, the replies and notifications come back out of the very same call.
import { Engine } from '../wasm/engine';
import type {
  CodeAction,
  CompletionItem,
  CompletionList,
  Diagnostic,
  DocumentSymbol,
  Hover,
  InitializeResult,
  Location,
  LocationLink,
  NotificationMessage,
  Position,
  PrepareRenameResult,
  PublishDiagnosticsParams,
  Range,
  ResponseMessage,
  SemanticTokens,
  TextEdit,
  WorkspaceEdit,
} from './types';
import { fileOfUri, uriOfFile } from './positions';

/** The engine that hosts the language server and the REPL session. */
export const languageEngine = new Engine('lang');

interface OpenDocument {
  version: number;
  text: string;
}

const documents = new Map<string, OpenDocument>();
const diagnosticListeners = new Set<(file: string, diagnostics: readonly Diagnostic[]) => void>();
let nextId = 0;
let initialized: Promise<void> | null = null;
let legend: readonly string[] = [];

function isResponse(m: unknown): m is ResponseMessage {
  return typeof m === 'object' && m !== null && 'id' in m && ('result' in m || 'error' in m);
}

function isDiagnosticsNotification(m: unknown): m is NotificationMessage & { params: PublishDiagnosticsParams } {
  return typeof m === 'object' && m !== null && (m as NotificationMessage).method === 'textDocument/publishDiagnostics';
}

async function send(message: object, wantId?: number): Promise<ResponseMessage | undefined> {
  const out = await languageEngine.call('lsp', { message: JSON.stringify(message) });
  let reply: ResponseMessage | undefined;
  for (const m of out.messages) {
    if (isResponse(m) && m.id === wantId) reply = m;
    else if (isDiagnosticsNotification(m)) {
      const { uri, diagnostics } = m.params;
      diagnosticListeners.forEach((l) => l(fileOfUri(uri), diagnostics));
    }
  }
  return reply;
}

function ready(): Promise<void> {
  initialized ??= (async () => {
    const id = ++nextId;
    const reply = await send(
      { jsonrpc: '2.0', id, method: 'initialize', params: { processId: null, rootUri: null, capabilities: {} } },
      id,
    );
    const result = reply?.result as InitializeResult | undefined;
    legend = result?.capabilities?.semanticTokensProvider?.legend?.tokenTypes ?? [];
    await send({ jsonrpc: '2.0', method: 'initialized', params: {} });
    // after a crash the new instance knows nothing: tell it again what is open
    for (const [uri, d] of documents) {
      await send({
        jsonrpc: '2.0',
        method: 'textDocument/didOpen',
        params: { textDocument: { uri, languageId: 'neon', version: d.version, text: d.text } },
      });
    }
  })();
  return initialized;
}

languageEngine.onCrash = () => {
  initialized = null;
};

async function request<T>(method: string, params: object): Promise<T> {
  await ready();
  const id = ++nextId;
  const reply = await send({ jsonrpc: '2.0', id, method, params }, id);
  if (!reply) throw new Error(`no reply to ${method}`);
  if (reply.error) throw new Error(reply.error.message);
  return reply.result as T;
}

const notify = (method: string, params: object): Promise<unknown> => send({ jsonrpc: '2.0', method, params });
const doc = (file: string): { uri: string } => ({ uri: uriOfFile(file) });

export const lsp = {
  start: (): Promise<void> => ready(),

  onDiagnostics(listener: (file: string, diagnostics: readonly Diagnostic[]) => void): () => void {
    diagnosticListeners.add(listener);
    return () => diagnosticListeners.delete(listener);
  },

  /** The token types of the semantic tokens, in the order of their numbers. */
  legend: (): readonly string[] => legend,

  async open(file: string, text: string): Promise<void> {
    const uri = uriOfFile(file);
    documents.set(uri, { version: 1, text });
    await ready();
    await notify('textDocument/didOpen', { textDocument: { uri, languageId: 'neon', version: 1, text } });
  },

  async change(file: string, text: string): Promise<void> {
    const uri = uriOfFile(file);
    const d = documents.get(uri);
    if (!d) return this.open(file, text);
    d.version++;
    d.text = text;
    await ready();
    await notify('textDocument/didChange', { textDocument: { uri, version: d.version }, contentChanges: [{ text }] });
  },

  async close(file: string): Promise<void> {
    documents.delete(uriOfFile(file));
    await ready();
    await notify('textDocument/didClose', { textDocument: doc(file) });
  },

  has: (file: string): boolean => documents.has(uriOfFile(file)),
  text: (file: string): string | undefined => documents.get(uriOfFile(file))?.text,
  files: (): string[] => [...documents.keys()].map(fileOfUri),

  completion: (file: string, position: Position): Promise<readonly CompletionItem[] | CompletionList | null> =>
    request('textDocument/completion', { textDocument: doc(file), position }),
  hover: (file: string, position: Position): Promise<Hover | null> =>
    request('textDocument/hover', { textDocument: doc(file), position }),
  definition: (file: string, position: Position): Promise<Location | readonly (Location | LocationLink)[] | null> =>
    request('textDocument/definition', { textDocument: doc(file), position }),
  prepareRename: (file: string, position: Position): Promise<PrepareRenameResult | Range | null> =>
    request('textDocument/prepareRename', { textDocument: doc(file), position }),
  rename: (file: string, position: Position, newName: string): Promise<WorkspaceEdit | null> =>
    request('textDocument/rename', { textDocument: doc(file), position, newName }),
  symbols: (file: string): Promise<readonly DocumentSymbol[] | null> =>
    request('textDocument/documentSymbol', { textDocument: doc(file) }),
  formatting: (file: string): Promise<readonly TextEdit[] | null> =>
    request('textDocument/formatting', { textDocument: doc(file), options: { tabSize: 4, insertSpaces: true } }),
  semanticTokens: (file: string): Promise<SemanticTokens | null> =>
    request('textDocument/semanticTokens/full', { textDocument: doc(file) }),
  codeActions: (file: string, range: Range, diagnostics: readonly Diagnostic[]): Promise<readonly CodeAction[] | null> =>
    request('textDocument/codeAction', { textDocument: doc(file), range, context: { diagnostics } }),
} as const;
