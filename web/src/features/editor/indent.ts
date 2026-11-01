import { getIndentUnit, indentService } from '@codemirror/language';

// indentation: that of the line above, one level deeper after an opening bracket, one less for a closer
export const neonIndent = indentService.of((context, pos) => {
  const doc = context.state.doc;
  const unit = getIndentUnit(context.state);
  const line = doc.lineAt(pos);
  let prevNo = line.number - 1;
  while (prevNo > 0 && doc.line(prevNo).text.trim() === '') prevNo--;
  if (prevNo < 1) return 0;
  const prev = doc.line(prevNo).text;
  let n = 0;
  for (const ch of /^[ \t]*/.exec(prev)?.[0] ?? '') n += ch === '\t' ? unit : 1;
  if (/[{(\[]\s*$/.test(prev.replace(/\s*\/\/.*$/, ''))) n += unit;
  if (/^\s*[})\]]/.test(line.text)) n = Math.max(0, n - unit);
  return n;
});
