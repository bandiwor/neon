// A client for the language server that runs inside the module (see web/wasm): JSON-RPC messages go in,
// the replies and notifications come back out of the very same call.
import { Engine } from './engine.js';

const engine = new Engine('lang');

let nextId = 0;
const documents = new Map(); // uri -> {version, text}
let initialized = null;
let legend = [];
const diagnosticListeners = new Set();

export const uriOf = (name) => `file:///${name}`;
export const nameOf = (uri) => decodeURIComponent(uri.replace(/^file:\/\/\//, ''));

function dispatch(messages, wantId) {
  let reply;
  for (const raw of messages) {
    const msg = typeof raw === 'string' ? JSON.parse(raw) : raw;
    if (msg.id === wantId && wantId !== undefined) reply = msg;
    else if (msg.method === 'textDocument/publishDiagnostics') {
      diagnosticListeners.forEach((l) => l(msg.params));
    }
  }
  return reply;
}

async function send(message, wantId) {
  const out = await engine.call({ op: 'lsp', message: JSON.stringify(message) });
  return dispatch(out.messages ?? [], wantId);
}

async function request(method, params) {
  await ready();
  const id = ++nextId;
  const reply = await send({ jsonrpc: '2.0', id, method, params }, id);
  if (!reply) throw new Error(`no reply to ${method}`);
  if (reply.error) throw new Error(reply.error.message);
  return reply.result;
}

function notify(method, params) {
  return send({ jsonrpc: '2.0', method, params });
}

function ready() {
  initialized ??= (async () => {
    const id = ++nextId;
    const reply = await send(
      { jsonrpc: '2.0', id, method: 'initialize', params: { processId: null, rootUri: null, capabilities: {} } },
      id,
    );
    legend = reply?.result?.capabilities?.semanticTokensProvider?.legend?.tokenTypes ?? [];
    await send({ jsonrpc: '2.0', method: 'initialized', params: {} });
    // after a crash the new instance knows nothing: tell it again what is open
    for (const [uri, d] of documents) {
      await send({ jsonrpc: '2.0', method: 'textDocument/didOpen', params: { textDocument: { uri, languageId: 'neon', version: d.version, text: d.text } } });
    }
  })();
  return initialized;
}

engine.onCrash = () => {
  initialized = null;
};

export const lsp = {
  start: () => ready(),
  onDiagnostics(listener) {
    diagnosticListeners.add(listener);
    return () => diagnosticListeners.delete(listener);
  },
  legend: () => legend,

  async open(name, text) {
    const uri = uriOf(name);
    documents.set(uri, { version: 1, text });
    await ready();
    await notify('textDocument/didOpen', { textDocument: { uri, languageId: 'neon', version: 1, text } });
  },

  async change(name, text) {
    const uri = uriOf(name);
    const d = documents.get(uri);
    if (!d) return this.open(name, text);
    d.version++;
    d.text = text;
    await ready();
    await notify('textDocument/didChange', { textDocument: { uri, version: d.version }, contentChanges: [{ text }] });
  },

  async close(name) {
    const uri = uriOf(name);
    documents.delete(uri);
    await ready();
    await notify('textDocument/didClose', { textDocument: { uri } });
  },

  has: (name) => documents.has(uriOf(name)),
  names: () => [...documents.keys()].map(nameOf),
  text: (name) => documents.get(uriOf(name))?.text,

  completion: (name, position) => request('textDocument/completion', { textDocument: { uri: uriOf(name) }, position }),
  hover: (name, position) => request('textDocument/hover', { textDocument: { uri: uriOf(name) }, position }),
  definition: (name, position) => request('textDocument/definition', { textDocument: { uri: uriOf(name) }, position }),
  references: (name, position) =>
    request('textDocument/references', { textDocument: { uri: uriOf(name) }, position, context: { includeDeclaration: true } }),
  prepareRename: (name, position) => request('textDocument/prepareRename', { textDocument: { uri: uriOf(name) }, position }),
  rename: (name, position, newName) => request('textDocument/rename', { textDocument: { uri: uriOf(name) }, position, newName }),
  symbols: (name) => request('textDocument/documentSymbol', { textDocument: { uri: uriOf(name) } }),
  formatting: (name, tabSize = 4) =>
    request('textDocument/formatting', { textDocument: { uri: uriOf(name) }, options: { tabSize, insertSpaces: true } }),
  signatureHelp: (name, position) => request('textDocument/signatureHelp', { textDocument: { uri: uriOf(name) }, position }),
  semanticTokens: (name) => request('textDocument/semanticTokens/full', { textDocument: { uri: uriOf(name) } }),
  codeActions: (name, range, diagnostics) =>
    request('textDocument/codeAction', { textDocument: { uri: uriOf(name) }, range, context: { diagnostics } }),
};

// ---- the REPL lives in the same instance ------------------------------------------------------------------

export const repl = {
  eval: (cell, stdin = '') => engine.call({ op: 'repl_eval', cell, stdin }),
  reset: () => engine.call({ op: 'repl_reset' }),
  cells: () => engine.call({ op: 'repl_cells' }),
  more: (text) => engine.call({ op: 'repl_more', text }),
  complete: (buffer, cursor) => engine.call({ op: 'repl_complete', buffer, cursor }),
  preview: (cell) => engine.call({ op: 'repl_preview', cell }),
  highlight: (text) => engine.call({ op: 'repl_highlight', text }),
};

export const langEngine = engine;
