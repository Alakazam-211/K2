// Whether an object is hidden in the scene drawn, and by what: K2 Cores'
// rule (k2-mpc-blend docs/WIRE.md, "Hidden by"), the same that gives the
// mesh read's `hidden` and the live view's derived `hidden_by`:
// 1. its own viewport flag: "self";
// 2. else any path from the master collection down through collections to
//    one holding it that passes no hidden collection: shown (null);
// 3. else no path reaches it: null (it isn't in the scene drawn);
// 4. else "collection:<address>", the outermost hidden collection on the
//    first path a depth-first walk finds (children in the file's order, a
//    collection holding the object ending the path, each collection
//    visited once).

export interface HiddenByScene {
  master: string
  collections: { key: string; hidden: boolean; children: string[] }[]
  objects: { key: string; hidden: boolean; in: string[] }[]
}

/** Each object's value: "self", "collection:<address>", or null (shown, or not in the scene drawn). */
export function hiddenBy(sc: HiddenByScene): Record<string, string | null> {
  const coll = new Map(sc.collections.map((c) => [c.key, c]))
  const hidden = (k: string): boolean => k !== sc.master && !!coll.get(k)?.hidden
  // Every collection a path of shown collections reaches.
  const shown = new Set<string>()
  const stack = [sc.master]
  while (stack.length) {
    const k = stack.pop()!
    if (shown.has(k) || hidden(k)) continue
    shown.add(k)
    for (const ch of coll.get(k)?.children ?? []) stack.push(ch)
  }
  const out: Record<string, string | null> = {}
  for (const o of sc.objects) {
    if (o.hidden) {
      out[o.key] = 'self'
      continue
    }
    if (o.in.some((c) => shown.has(c))) {
      out[o.key] = null
      continue
    }
    const holds = new Set(o.in)
    const seen = new Set<string>()
    const walk = (k: string, outer: string | null): string | null | undefined => {
      if (seen.has(k)) return undefined
      seen.add(k)
      const o2 = outer ?? (hidden(k) ? k : null)
      if (holds.has(k)) return o2
      for (const ch of coll.get(k)?.children ?? []) {
        const r = walk(ch, o2)
        if (r !== undefined) return r
      }
      return undefined
    }
    const by = walk(sc.master, null)
    out[o.key] = by ? `collection:${by}` : null
  }
  return out
}
