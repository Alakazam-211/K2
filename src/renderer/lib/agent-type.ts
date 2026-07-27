/**
 * Endgame L2 (Stage A): `'k2'` and legacy `'k2so'` are the **same**
 * builtin agent type. Every renderer comparison for the builtin mode
 * must go through here so a later Stage-B value migration cannot strand
 * a reader that still only accepts the legacy spelling.
 *
 * Writer flip is **not** this stage — stored values stay `"k2so"` until
 * Stage B; this only makes readers dual-tolerant.
 */
export function isBuiltinAgentType(t?: string | null): boolean {
  return t === 'k2' || t === 'k2so'
}
