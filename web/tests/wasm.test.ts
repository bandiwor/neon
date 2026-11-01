// The module's JSON interface, driven from Node exactly as the worker drives it in the browser.
import { beforeAll, describe, expect, it } from 'vitest';
import { needsMore } from '../src/core/util/replMore';
import { loadModule, wasmBuilt, type Call } from './helpers/wasm';

describe.skipIf(!wasmBuilt)('the Neon module', () => {
  let call: Call;
  beforeAll(async () => {
    call = await loadModule();
  });

  const cli = (args: string[], files: Record<string, string>, stdin = '', interactive = false) =>
    call('cli', { args, files, stdin, interactive });

  it('runs a program and reports the exit code', () => {
    const ok = cli(['run', 'main.neon'], { 'main.neon': 'fn main() { println(6 * 7); }' });
    expect([ok.code, ok.stdout, ok.stderr]).toEqual([0, '42\n', '']);
    const bad = cli(['run', 'main.neon'], { 'main.neon': 'fn main() { let x = ; }' });
    expect(bad.code).toBe(1);
    expect(bad.stderr).toMatch(/expected expression/);
    const trap = cli(['run', 'main.neon'], { 'main.neon': 'fn main() { let z = 0; println(1 / z); }' });
    expect(trap.code).toBe(2);
    expect(trap.stderr).toMatch(/division by zero/);
  });

  it('formats and shows the IR, the bytecode and the C translation', () => {
    const files = { 'main.neon': 'fn main(){println(1+2);}' };
    expect(cli(['fmt', 'main.neon'], files).stdout).toMatch(/fn main\(\)/);
    expect(cli(['ir', 'main.neon'], files).stdout).toMatch(/fn /);
    expect(cli(['disasm', 'main.neon', '--annotate'], files).stdout).toMatch(/\S/);
    expect(cli(['c', 'main.neon'], files).stdout).toMatch(/int main\(/);
  });

  it('runs a project of several files', () => {
    const r = cli(['run', 'main.neon'], {
      'main.neon': 'import { sq } from "lib/util";\nfn main() { println(sq(7)); }',
      'lib/util.neon': 'export fn sq(n: i64) -> i64 { n * n }',
    });
    expect([r.code, r.stdout]).toEqual([0, '49\n']);
  });

  it('asks for input instead of failing when the program runs out of it', () => {
    const files = { 'main.neon': 'fn main() { let a = input(); println("hi " + a); }' };
    expect(cli(['run', 'main.neon'], files, '', true).needsInput).toBe(true);
    const second = cli(['run', 'main.neon'], files, 'Ada\n', true);
    expect([second.needsInput, second.stdout]).toEqual([false, 'hi Ada\n']);
  });

  it('hosts the language server', () => {
    const init = call('lsp', { message: JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'initialize', params: { capabilities: {} } }) });
    expect(init.messages).toHaveLength(1);
    expect((init.messages[0] as { id: number }).id).toBe(1);
  });

  it('hosts the REPL', () => {
    expect(call('repl_eval', { cell: '2 + 3' }).text).toBe('5\n');
    expect(call('repl_preview', { cell: '2 + 3' }).value).toBe('5');
  });

  it('agrees with the prompt on when input is unfinished', () => {
    const samples = [
      'fn f() {', 'let a = [1,', 'fn f() { 1 }', 'println("}");', 'let s = "abc', '// {\nlet a = 1;', '/* open', "let c = '{';",
      "let q = '\\'';", '', '1 + 1', '(((', ')))', 'f"x {', "'", 'let x = "a\\"b";', '/* a */ {', 'match x {\n  1 => 2,\n}',
    ];
    for (const text of samples) expect(needsMore(text), JSON.stringify(text)).toBe(call('repl_more', { text }).more);
  });

  it('answers a bad request with an error, not a crash', () => {
    const reply = (call as unknown as (op: string, payload: object) => { error?: string })('nonsense', {});
    expect(reply.error).toMatch(/unknown op/);
  });
});
