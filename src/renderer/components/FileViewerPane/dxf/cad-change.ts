// What changed in a CAD drawing between two versions, entity by entity, as
// the CAD pack reports it (`cad.change/1`): who made each version, which
// entities were added, removed or changed with their boxes before and after,
// which layers and blocks changed, and whether every version in the range
// kept the drawing's handles (`pairing`). The same shape serves DXF and DWG.
// Read defensively: a field that is missing or of the wrong type is dropped,
// never guessed; new fields are ignored.

/** `[x0, y0, x1, y1]` in drawing units. */
export type ChangeBox = [number, number, number, number]

export type CadAuthor = { pid: string; name: string; kind: string }

export type CadVersion = { version: number; by: CadAuthor; verb: string }

export type CadEntityRow = {
  /** `@h:<HANDLE>`. */
  address: string
  op: 'added' | 'removed' | 'changed'
  /** DXF type, upper case (`LINE`). */
  kind: string
  layer: string | null
  /** `model`, a layout name, or `@h:<block>` for an entity inside a block definition. */
  owner: string
  fields: string[]
  before: ChangeBox | null
  after: ChangeBox | null
  /** `exact`, `approx` (close, not tight) or `none` (no box). */
  quality: 'exact' | 'approx' | 'none'
  version: number | null
  /** The author's pid. */
  by: string | null
  /** Paired across a save by an app not yet shown to keep handles. */
  doubt: boolean
}

export type CadTableRow = { address: string; what: string; name: string; op: string; fields: string[]; version: number | null; by: string | null }

export type CadBlockRow = { address: string; name: string; op: string; inserts: string[]; insertsMore: number }

export type CadChange = {
  from: number
  to: number
  /** `kept`: every handle is trusted; `doubt`: an outside save is in the range; `refused`: handles were renumbered. */
  pairing: 'kept' | 'doubt' | 'refused'
  refusedWhy: string | null
  versions: CadVersion[]
  entities: CadEntityRow[]
  /** Rows past the read budget, counted but not listed. */
  entitiesMore: number
  tables: CadTableRow[]
  blocks: CadBlockRow[]
}

type Obj = Record<string, unknown>
const isObj = (v: unknown): v is Obj => typeof v === 'object' && v !== null && !Array.isArray(v)
const str = (v: unknown): string | null => (typeof v === 'string' ? v : null)
const num = (v: unknown): number | null => (typeof v === 'number' && Number.isFinite(v) ? v : null)
const list = (v: unknown): unknown[] => (Array.isArray(v) ? v : [])
const strs = (v: unknown): string[] => list(v).filter((s): s is string => typeof s === 'string')

function box(v: unknown): ChangeBox | null {
  if (!Array.isArray(v) || v.length !== 4 || !v.every((n) => typeof n === 'number' && Number.isFinite(n))) return null
  const [a, b, c, d] = v as number[]
  return [Math.min(a, c), Math.min(b, d), Math.max(a, c), Math.max(b, d)]
}

function author(v: unknown): CadAuthor | null {
  if (!isObj(v)) return null
  const pid = str(v.pid)
  return pid ? { pid, name: str(v.name) || 'Someone', kind: str(v.kind) ?? '' } : null
}

function entityRow(v: unknown): CadEntityRow | null {
  if (!isObj(v)) return null
  const address = str(v.address)
  const op = v.op
  if (!address || (op !== 'added' && op !== 'removed' && op !== 'changed')) return null
  const q = v.bbox_q
  return {
    address,
    op,
    kind: (str(v.kind) ?? '').toUpperCase(),
    layer: str(v.layer),
    owner: str(v.owner) ?? 'model',
    fields: strs(v.fields),
    before: box(v.bbox_before),
    after: box(v.bbox_after),
    quality: q === 'approx' || q === 'none' ? q : 'exact',
    version: num(v.version),
    by: str(v.by),
    doubt: v.identity === 'doubt',
  }
}

/** The answer as a change, or null when it isn't `cad.change/1`. */
export function parseCadChange(raw: unknown): CadChange | null {
  if (!isObj(raw) || raw.schema !== 'cad.change/1') return null
  const from = num(raw.from)
  const to = num(raw.to)
  if (from === null || to === null) return null
  const pairing = raw.pairing === 'doubt' || raw.pairing === 'refused' ? raw.pairing : 'kept'
  const versions: CadVersion[] = []
  for (const v of list(raw.versions)) {
    if (!isObj(v)) continue
    const version = num(v.version)
    const by = author(v.by)
    if (version !== null && by) versions.push({ version, by, verb: str(v.verb) ?? '' })
  }
  const tables: CadTableRow[] = []
  for (const t of list(raw.tables)) {
    if (!isObj(t) || !str(t.name)) continue
    tables.push({ address: str(t.address) ?? '', what: str(t.what) ?? '', name: str(t.name)!, op: str(t.op) ?? 'changed', fields: strs(t.fields), version: num(t.version), by: str(t.by) })
  }
  const blocks: CadBlockRow[] = []
  for (const b of list(raw.blocks)) {
    if (!isObj(b) || !str(b.name)) continue
    blocks.push({ address: str(b.address) ?? '', name: str(b.name)!, op: str(b.op) ?? 'changed', inserts: strs(b.affected_inserts), insertsMore: num(b.affected_more) ?? 0 })
  }
  return {
    from,
    to,
    pairing,
    refusedWhy: pairing === 'refused' ? str(raw.refused_why) : null,
    versions,
    entities: pairing === 'refused' ? [] : list(raw.entities).map(entityRow).filter((r): r is CadEntityRow => r !== null),
    entitiesMore: num(raw.entities_more) ?? 0,
    tables: pairing === 'refused' ? [] : tables,
    blocks: pairing === 'refused' ? [] : blocks,
  }
}
