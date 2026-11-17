// A drawing that edits answer to, for tests and shots only: the model's
// header and pages, held in memory, with the edit verbs' rules played out
// on them the way the CAD pack answers (`mpc.result/1` and `cad.change/1`).
// A move shifts every point an entity has; an attribute set changes its
// value; a locked layer, a kind the verbs don't change, the wrong unit, a
// stale `--expect` or an overlap since `--base` is refused with the pack's
// id. A person is never refused by a hold. Each landed call makes a version.

import type { CadChangeSource } from '../cad-change-source'
import { fixtureDrawingSource, type DrawingModelSource } from '../model/source'
import type { ModelHeader, ModelPage, ModelRow } from '../model/types'
import type { EditSource, Hold } from './edit-source'
import { editReason } from './selection'
import { parseToolResult } from './tool-result'

type Box4 = [number, number, number, number]
const flag = (argv: readonly string[], name: string): string | null => {
  const i = argv.indexOf(name)
  return i >= 0 ? (argv[i + 1] ?? null) : null
}

const POINT_KEYS = new Set(['p1', 'p2', 'p', 'centre', 'anchor', 'from', 'to', 'at', 'point'])
const LIST_KEYS = new Set(['corners', 'quad', 'path', 'ctrl', 'fit', 'mask', 'clip'])

const isXY = (v: unknown): v is number[] => Array.isArray(v) && v.length >= 2 && typeof v[0] === 'number' && typeof v[1] === 'number'
const shiftXY = (v: number[], dx: number, dy: number): number[] => [v[0] + dx, v[1] + dy, ...v.slice(2)]

/** Every point an entity's geometry has, moved by dx, dy. */
export function shiftGeom(g: unknown, dx: number, dy: number): unknown {
  if (Array.isArray(g)) return g.map((x) => shiftGeom(x, dx, dy))
  if (!g || typeof g !== 'object') return g
  const out: Record<string, unknown> = {}
  for (const [k, v] of Object.entries(g as Record<string, unknown>)) {
    if (POINT_KEYS.has(k) && isXY(v)) out[k] = shiftXY(v, dx, dy)
    else if (LIST_KEYS.has(k) && Array.isArray(v)) out[k] = v.map((p) => (isXY(p) ? shiftXY(p, dx, dy) : p))
    else if (k === 'm' && Array.isArray(v) && v.length === 6) out[k] = [v[0], v[1], v[2], v[3], v[4] + dx, v[5] + dy]
    else if (k === 'verts' || k === 'attribs' || k === 'text' || k === 'loops' || k === 'edges') out[k] = shiftGeom(v, dx, dy)
    else out[k] = v
  }
  return out
}

const shiftBox = (b: Box4 | null, dx: number, dy: number): Box4 | null => (b ? [b[0] + dx, b[1] + dy, b[2] + dx, b[3] + dy] : null)

export type FakeWorld = {
  model: DrawingModelSource
  edits: EditSource & { calls: string[][]; setHolds(h: Hold[]): void }
  changes: CadChangeSource
  /** Another writer's version: the rows it changes, as an agent would through the verbs. */
  agentMove(handles: readonly string[], dx: number, dy: number): void
  version(): number
}

export function fakeCadWorld(
  fx: { header: ModelHeader; pages: Record<string, ModelPage[]> },
  /** `locked`: layers locked in the file since the viewer read it (the header shown doesn't say). */
  opts: { holds?: Hold[]; locked?: string[] } = {},
): FakeWorld {
  let header = structuredClone(fx.header)
  const pages = structuredClone(fx.pages)
  const model = (): ModelRow[] => pages.model?.[0]?.entities ?? []
  /** Which version last changed each entity's fields. */
  const touched = new Map<string, { version: number; fields: string[] }>()
  let holds = opts.holds ?? []
  const calls: string[][] = []
  const src = fixtureDrawingSource({ header, pages })
  let lastDry: string | null = null

  const bump = (rows: ModelRow[], fields: string[]): number => {
    header = { ...header, version: header.version + 1 }
    for (const p of Object.values(pages)) for (const pg of p) pg.version = header.version
    for (const r of rows) touched.set(r.h, { version: header.version, fields })
    src.setHeader(header)
    return header.version
  }

  const result = (verb: string, base: number, over: Partial<Record<string, unknown>>): unknown => ({
    schema: 'mpc.result/1', verb, kind: 'edit', ok: true, exit: 0,
    version: { base, after: null, if_version: null, base_given: base },
    changed: null, data: null, data_schema: null, error: null, conflicts: [], notes: [], warnings: [], view: null, dry_run: false, stdout: '',
    ...over,
  })
  const refuse = (verb: string, base: number, id: string, message: string, exit = 2, details: Record<string, unknown> = {}) =>
    result(verb, base, { ok: false, exit, error: { id, exit, message, next: '', details } })

  const run = (argv: readonly string[]): unknown => {
    calls.push([...argv])
    const verb = `${argv[0]} ${argv[1]}`
    const base = Number(flag(argv, '--base') ?? header.version)
    const dry = argv.includes('--dry-run')
    const expect = flag(argv, '--expect')
    const addrs = argv.slice(2).filter((a) => a.startsWith('@h:'))
    const rows = addrs.map((a) => model().find((r) => r.h.toUpperCase() === a.toUpperCase()))
    const missing = addrs.find((_a, i) => !rows[i])
    if (missing) return refuse(verb, base, 'CAD-E010', `${missing} names no entity`, 4)
    const found = rows as ModelRow[]
    const fields = verb === 'attrib set' ? ['text'] : ['geom']
    for (const r of found) {
      const why = editReason(r, header)
      if (why === 'locked' || opts.locked?.includes(r.layer)) return refuse(verb, base, 'DXF-E045', `${r.h} is on a locked layer`)
      if (why) return refuse(verb, base, 'DXF-E021', `${r.h} isn't changed by the edit verbs (${why})`)
      const t = touched.get(r.h)
      if (t && t.version > base && t.fields.some((f) => fields.includes(f)))
        return refuse(verb, base, 'MPC-E723', 'changed since', 1, { conflicts: [{ address: r.h, field: fields[0], by: 'Someone', change: t.version }] })
    }
    let next: { rows: ModelRow[]; ents: unknown[] }
    if (verb === 'entity move') {
      if (flag(argv, '--unit') !== (header.units?.word || 'drawing')) return refuse(verb, base, 'DXF-E022', 'not the drawing’s unit')
      const [dx, dy] = (flag(argv, '--by') ?? '0,0').split(',').map(Number)
      next = {
        rows: found.map((r) => ({ ...r, bbox: shiftBox(r.bbox, dx, dy), geom: shiftGeom(r.geom, dx, dy) as ModelRow['geom'] })),
        ents: found.map((r) => ({ address: r.h, op: 'changed', kind: r.kind, layer: r.layer, owner: 'model', fields, bbox_before: r.bbox, bbox_after: shiftBox(r.bbox, dx, dy), bbox_q: r.bbox_q ?? 'exact', version: null, by: null, identity: null })),
      }
    } else if (verb === 'attrib set') {
      const r = found[0]
      const tag = (flag(argv, '--tag') ?? '').toUpperCase()
      const nth = Number(flag(argv, '--nth') ?? 1)
      const value = flag(argv, '--value') ?? ''
      const attribs = (Array.isArray(r.geom?.attribs) ? r.geom.attribs : []) as { tag: string; value: string; text?: Record<string, unknown> }[]
      const same = attribs.filter((a) => a.tag.toUpperCase() === tag)
      const a = same[nth - 1]
      if (!a) return refuse(verb, base, 'DXF-E047', 'no attribute with that tag')
      if (a.value === value) return result(verb, base, { dry_run: dry, stdout: `no change: ${tag} is already that` })
      const nextAttribs = attribs.map((x) => (x === a ? { ...x, value, text: x.text ? { ...x.text, plain: value, runs: [{ text: value }] } : x.text } : x))
      next = {
        rows: [{ ...r, geom: { ...r.geom, attribs: nextAttribs } }],
        ents: [{ address: r.h, op: 'changed', kind: r.kind, layer: r.layer, owner: 'model', fields, bbox_before: r.bbox, bbox_after: r.bbox, bbox_q: r.bbox_q ?? 'exact', version: null, by: null, identity: null }],
      }
    } else {
      return refuse(verb, base, 'CAD-E004', 'not a verb this drawing knows')
    }
    const want = `sha256:fake-${header.version}-${calls.length}`
    const data = { schema: 'cad.change/1', format: 'dxf', from: null, to: null, units: header.units ?? null, pairing: 'kept', versions: [], entities: next.ents, entities_more: 0, tables: [], blocks: [], verb, word: verb === 'entity move' ? 'moved' : 'set' }
    const bytes = (d: string) => ({ word: data.word, counts: [], units: [], lines: null, bytes: { size_before: 0, size_after: 0, spans: 0, removed: 0, inserted: 0, after_digest: d } })
    if (dry) {
      lastDry = want
      return result(verb, base, { dry_run: true, changed: bytes(want), data })
    }
    if (expect !== lastDry) return refuse(verb, base, 'MPC-E729', 'not the dry run’s bytes')
    const page = pages.model[0]
    page.entities = page.entities.map((r) => next.rows.find((n) => n.h === r.h) ?? r)
    const after = bump(next.rows, fields)
    return result(verb, after - 1, { version: { base: after - 1, after, if_version: null, base_given: base }, changed: bytes(expect ?? ''), data })
  }

  return {
    model: src,
    edits: {
      canWrite: true,
      calls,
      setHolds: (h) => void (holds = h),
      run: async (_p, argv) => parseToolResult(run(argv))!,
      holds: async () => holds,
    },
    changes: {
      async change() {
        return null
      },
      async revert() {
        return { ok: true }
      },
    },
    agentMove(handles, dx, dy) {
      const page = pages.model[0]
      const moved = page.entities.filter((r) => handles.includes(r.h)).map((r) => ({ ...r, bbox: shiftBox(r.bbox, dx, dy), geom: shiftGeom(r.geom, dx, dy) as ModelRow['geom'] }))
      page.entities = page.entities.map((r) => moved.find((m) => m.h === r.h) ?? r)
      bump(moved, ['geom'])
    },
    version: () => header.version,
  }
}
