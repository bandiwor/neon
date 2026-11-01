// Writes the type declarations of the CSS modules (see build/plugins/cssModuleTypes.ts); `npm run typecheck`
// and `npm run build` run it first.
import { resolve } from 'node:path';
import { writeAllDeclarations } from '../build/plugins/cssModuleTypes';

const src = resolve(import.meta.dirname, '../src');
const changed = writeAllDeclarations(src, [resolve(src, 'styles')]);
console.log(`css module types: ${changed} written`);
