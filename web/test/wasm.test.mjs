// The module's JSON interface, driven from Node exactly as the worker drives it in the browser.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';

const path = new URL('../dist/neon.wasm', import.meta.url);
const built = existsSync(path);

async function load() {
  const { instance } = await WebAssembly.instantiate(readFileSync(path), {});
  const x = instance.exports;
  const enc = new TextEncoder();
  const dec = new TextDecoder();
  return (req) => {
    const bytes = enc.encode(JSON.stringify(req));
    const p = x.neon_alloc(bytes.length);
    new Uint8Array(x.memory.buffer, p, bytes.length).set(bytes);
    const out = x.neon_call(p, bytes.length);
    x.neon_dealloc(p, bytes.length);
    const len = new DataView(x.memory.buffer).getUint32(out, true);
    const text = dec.decode(new Uint8Array(x.memory.buffer, out + 4, len));
    x.neon_dealloc(out, len + 4);
    return JSON.parse(text);
  };
}

test('runs a program and reports the exit code', { skip: !built && 'dist/neon.wasm is not built' }, async () => {
  const call = await load();
  const ok = call({ op: 'cli', args: ['run', 'main.neon'], files: { 'main.neon': 'fn main() { println(6 * 7); }' } });
  assert.deepEqual([ok.code, ok.stdout, ok.stderr], [0, '42\n', '']);
  const bad = call({ op: 'cli', args: ['run', 'main.neon'], files: { 'main.neon': 'fn main() { let x = ; }' } });
  assert.equal(bad.code, 1);
  assert.match(bad.stderr, /expected expression/);
  const trap = call({ op: 'cli', args: ['run', 'main.neon'], files: { 'main.neon': 'fn main() { let z = 0; println(1 / z); }' } });
  assert.equal(trap.code, 2);
  assert.match(trap.stderr, /division by zero/);
});

test('formats, and shows the IR, the bytecode and the C translation', { skip: !built && 'not built' }, async () => {
  const call = await load();
  const files = { 'main.neon': 'fn main(){println(1+2);}' };
  assert.match(call({ op: 'cli', args: ['fmt', 'main.neon'], files }).stdout, /fn main\(\)/);
  assert.match(call({ op: 'cli', args: ['ir', 'main.neon'], files }).stdout, /fn /);
  assert.match(call({ op: 'cli', args: ['disasm', 'main.neon', '--annotate'], files }).stdout, /\S/);
  assert.match(call({ op: 'cli', args: ['c', 'main.neon'], files }).stdout, /int main\(/);
});

test('asks for input instead of failing when the program runs out of it', { skip: !built && 'not built' }, async () => {
  const call = await load();
  const files = { 'main.neon': 'fn main() { let a = input(); println("hi " + a); }' };
  const first = call({ op: 'cli', args: ['run', 'main.neon'], files, stdin: '', interactive: true });
  assert.equal(first.needsInput, true);
  const second = call({ op: 'cli', args: ['run', 'main.neon'], files, stdin: 'Ada\n', interactive: true });
  assert.deepEqual([second.needsInput, second.stdout], [false, 'hi Ada\n']);
});

test('the language server and the REPL live in the module', { skip: !built && 'not built' }, async () => {
  const call = await load();
  const init = call({ op: 'lsp', message: JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'initialize', params: { capabilities: {} } }) });
  assert.equal(JSON.parse(init.messages[0]?.jsonrpc ? JSON.stringify(init.messages[0]) : init.messages[0]).id, 1);
  assert.equal(call({ op: 'repl_eval', cell: '2 + 3' }).text, '5\n');
});

test('the prompt\'s "is the input unfinished" check agrees with the compiler\'s', { skip: !built && 'not built' }, async () => {
  const { needsMore } = await import('../src/repl-more.js');
  const call = await load();
  const samples = [
    'fn f() {', 'let a = [1,', 'fn f() { 1 }', 'println("}");', 'let s = "abc', '// {\nlet a = 1;', '/* open', "let c = '{';",
    "let q = '\\'';", '', '1 + 1', '(((', ')))', 'f"x {', "'", 'let x = "a\\"b";', '/* a */ {', 'match x {\n  1 => 2,\n}',
  ];
  for (const text of samples) {
    assert.equal(needsMore(text), call({ op: 'repl_more', text }).more, JSON.stringify(text));
  }
});
