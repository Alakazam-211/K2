// What each entity kind is called in the selection panel: one and many.

const NAMES: Record<string, [string, string]> = {
  LINE: ['line', 'lines'],
  CIRCLE: ['circle', 'circles'],
  ARC: ['arc', 'arcs'],
  ELLIPSE: ['ellipse', 'ellipses'],
  LWPOLYLINE: ['polyline', 'polylines'],
  POLYLINE: ['polyline', 'polylines'],
  SPLINE: ['spline', 'splines'],
  POINT: ['point', 'points'],
  TEXT: ['text', 'texts'],
  MTEXT: ['text', 'texts'],
  INSERT: ['block', 'blocks'],
  DIMENSION: ['dimension', 'dimensions'],
  HATCH: ['hatch', 'hatches'],
  SOLID: ['solid', 'solids'],
  TRACE: ['trace', 'traces'],
  '3DFACE': ['face', 'faces'],
  '3DSOLID': ['3D solid', '3D solids'],
  LEADER: ['leader', 'leaders'],
  MLEADER: ['leader', 'leaders'],
  MULTILEADER: ['leader', 'leaders'],
  XLINE: ['construction line', 'construction lines'],
  RAY: ['ray', 'rays'],
  IMAGE: ['image', 'images'],
  WIPEOUT: ['wipeout', 'wipeouts'],
  VIEWPORT: ['viewport', 'viewports'],
}

export const kindName = (kind: string, n = 1): string => (NAMES[kind] ?? [kind.toLowerCase(), `${kind.toLowerCase()} items`])[n === 1 ? 0 : 1]

/** "2 lines, 1 block": each kind's count, most first. */
export function kindsLine(kinds: readonly string[]): string {
  const by = new Map<string, number>()
  for (const k of kinds) {
    const [one] = NAMES[k] ?? [k.toLowerCase()]
    by.set(one, (by.get(one) ?? 0) + 1)
  }
  const kindOf = new Map(kinds.map((k) => [(NAMES[k] ?? [k.toLowerCase()])[0], k]))
  return [...by]
    .sort((a, b) => b[1] - a[1])
    .map(([one, n]) => `${n} ${kindName(kindOf.get(one)!, n)}`)
    .join(', ')
}

export const capital = (s: string): string => s.charAt(0).toUpperCase() + s.slice(1)
