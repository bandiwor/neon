// Modules that do not exist on disk: the build provides them.

declare module 'virtual:neon-examples' {
  import type { Example } from '@/core/project/examples';
  /** The example programs (see build/plugins/examples.ts). */
  const examples: readonly Example[];
  export default examples;
}

declare module '*.wasm?url' {
  const url: string;
  export default url;
}
