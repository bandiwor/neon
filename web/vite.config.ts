import preact from '@preact/preset-vite';
import { resolve } from 'node:path';
import { defineConfig } from 'vite';
import { cssTargets, jsTarget } from './build/browsers';
import { cssModuleTypes } from './build/plugins/cssModuleTypes';
import { examples } from './build/plugins/examples';
import { shortClassNames } from './build/plugins/shortClassNames';

const root = import.meta.dirname;
const src = resolve(root, 'src');
const styles = resolve(src, 'styles');

export default defineConfig(({ command }) => {
  const production = command === 'build';
  return {
    // relative URLs: the site works from any directory (GitHub Pages serves it under /<repository>/)
    base: './',
    plugins: [preact(), examples({ root }), cssModuleTypes({ srcDir: src, loadPaths: [styles] }), shortClassNames()],
    resolve: { alias: { '@': src } },

    css: {
      // Lightning CSS compiles, prefixes and minifies the styles, and generates the module class names
      transformer: 'lightningcss',
      lightningcss: {
        targets: cssTargets,
        cssModules: {
          // unique names here; in production shortClassNames() then shortens all of them to one or two letters
          pattern: production ? '[hash]_[local]' : '[name]__[local]_[hash]',
          dashedIdents: false,
        },
      },
      preprocessorOptions: { scss: { loadPaths: [styles], api: 'modern-compiler' } },
      devSourcemap: true,
    },

    worker: { format: 'es' },

    build: {
      target: [...jsTarget],
      cssTarget: [...jsTarget],
      cssMinify: 'lightningcss',
      minify: 'terser',
      terserOptions: {
        ecma: 2022,
        module: true,
        // names in the top scope of a module are renamed too; properties are not (libraries use them by name)
        toplevel: true,
        mangle: { toplevel: true, module: true },
        compress: { ecma: 2022, passes: 3, pure_getters: true, unsafe_arrows: true, drop_debugger: true },
        format: { comments: false, ecma: 2022 },
      },
      sourcemap: false,
      reportCompressedSize: true,
      chunkSizeWarningLimit: 700,
      rollupOptions: {
        output: {
          // the editor and the framework change rarely: their own files cache across releases
          manualChunks(id: string): string | undefined {
            if (id.includes('node_modules/@codemirror') || id.includes('node_modules/@lezer') || id.includes('node_modules/style-mod')) return 'codemirror';
            if (id.includes('node_modules/preact')) return 'preact';
            return undefined;
          },
        },
      },
    },

    test: {
      include: ['tests/**/*.test.ts'],
      environment: 'node',
      testTimeout: 60_000,
      hookTimeout: 60_000,
    },
  };
});
