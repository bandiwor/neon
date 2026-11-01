import { indentService } from '@codemirror/language';

// indentation: that of the line above, one level deeper after an opening bracket, one less for a closer
export const neonIndent = indentService.of((context, pos) => {
  const doc = context.state.doc;
  const line = doc.lineAt(pos);
  let prevNo = line.number - 1;
  while (prevNo > 0 && doc.line(prevNo).text.trim() === '') prevNo--;
  if (prevNo < 1) return 0;
  const prev = doc.line(prevNo).text;
  let n = prev.match(/^ */)?.[0].length ?? 0;
  if (/[{(\[]\s*$/.test(prev)) n += 4;
  if (/^\s*[})\]]/.test(line.text)) n = Math.max(0, n - 4);
  return n;
});
