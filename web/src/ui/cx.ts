/** Joins class names, skipping the empty ones: `cx(s.a, active && s.on, props.class)`. */
export function cx(...names: readonly (string | false | null | undefined)[]): string {
  return names.filter((n): n is string => typeof n === 'string' && n !== '').join(' ');
}
