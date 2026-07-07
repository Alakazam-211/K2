/** Join class fragments, dropping falsy values. The primitives layer's only helper. */
export function cx(...parts: Array<string | false | null | undefined>): string {
  return parts.filter(Boolean).join(' ')
}
