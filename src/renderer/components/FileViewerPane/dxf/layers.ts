// Layer visibility in model space, by K2DRAW1 §3 and §7. A layer is hidden
// when it is off (a negative colour) or frozen (flag 1). Flag 2, "frozen in
// new viewports", only governs viewports made later, so model space draws it.
// dxf-parser folds flags 1 and 2 into one `frozen`, so the flags are read
// from the LAYER table's group codes here.

/** LAYER name → its group 70 flags, straight from the TABLES section. */
export function layerFlags(text: string): Map<string, number> {
  const lines = text.split(/\r?\n/)
  const out = new Map<string, number>()
  let inTables = false
  let name: string | null = null
  let flags = 0
  const close = () => {
    if (name !== null) out.set(name, flags)
    name = null
    flags = 0
  }
  let inLayer = false
  for (let i = 0; i + 1 < lines.length; i += 2) {
    const code = lines[i].trim()
    const v = lines[i + 1].trim()
    if (code === '0') {
      if (inLayer) close()
      inLayer = false
      if (v === 'SECTION') inTables = lines[i + 2]?.trim() === '2' && lines[i + 3]?.trim() === 'TABLES'
      else if (v === 'ENDSEC' && inTables) break
      else if (v === 'LAYER' && inTables) inLayer = true
      continue
    }
    if (!inLayer) continue
    // Trimmed, as dxf-parser keys its layers.
    if (code === '2') name = v
    else if (code === '70') flags = Number.parseInt(v, 10) || 0
  }
  return out
}

/** dxf-parser's layer record, plus the raw flags when they were read. */
export function layerHidden(l: { visible?: boolean; frozen?: boolean }, flags: number | undefined): boolean {
  if (l.visible === false) return true
  return flags === undefined ? l.frozen === true : (flags & 1) !== 0
}
