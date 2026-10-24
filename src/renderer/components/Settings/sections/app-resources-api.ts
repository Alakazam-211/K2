// Settings → Apps → This server → Box usage (prd-app-resources-disk-guard-v1
// Slice B, 0.46.4). Typed bindings for the box usage routes and the pure
// helpers the panel draws with. Thin client: the daemon owns the
// measurements, the guard's lines, the trends and what clean may free; this
// file reads that truth and sends the person's gestures (rescan, clean) to
// the window's server (`primaryScope()`).
//
//   GET  /cli/app/resources                    the view (k2 app resources)
//   GET  /cli/app/resources/trends?window=24h  CPU and memory over time
//   POST /cli/app/resources/rescan {}          a fresh folder scan
//   POST /cli/app/resources/clean {dryRun}     what K2 made and can make again

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { primaryScope } from '@/kessel/server-scope'

export type Level = 'ok' | 'warn' | 'refuse' | 'critical'

export interface Lines {
  warnPct: number
  warnFreeBytes: number
  refusePct: number
  reserveBytes: number
  criticalPct: number
  criticalFreeBytes: number
}

export interface Disk {
  id: string
  mount: string
  totalBytes: number
  usedBytes: number
  freeBytes: number
  usedPct: number
  level: Level
  holds: string[]
  notMeasuredBytes: number | null
}

export interface UsageRow {
  kind: 'app' | 'service' | 'database' | 'workspace' | 'shared'
  id: string
  owner: string | null
  ownerAddress: string | null
  diskBytes: number | null
  memBytes: number | null
  budgetBytes: number | null
  /** The absolute folder (person callers only: the owner, an Admin). */
  folder: string | null
}

export interface OwnerTotal {
  handle: string | null
  address: string | null
  diskBytes: number
}

export interface BoxUsage {
  host: string | null
  mode: 'on' | 'warn' | 'off'
  modeSource: string
  lines: Lines
  disks: Disk[]
  memTotalBytes: number | null
  memAvailableBytes: number | null
  load5: number | null
  cores: number | null
  rows: UsageRow[]
  owners: OwnerTotal[]
  k2WillClearBytes: number | null
  scannedAt: number | null
  scanRunning: boolean
}

export interface TrendPoint {
  at: number
  cpuCores: number | null
  memBytes: number | null
}

export interface Trends {
  window: string
  cores: number | null
  memTotalBytes: number | null
  box: TrendPoint[]
  apps: { app: string; owner: string | null; memBudgetBytes: number | null; memPeakBytes: number | null; cpuAvgCores: number | null }[]
}

export interface CleanItem {
  kind: string
  id: string
  app: string | null
  bytes: number | null
  removed: boolean
  error: string | null
}

export interface CleanResult {
  dryRun: boolean
  scope: 'box' | 'workspace'
  items: CleanItem[]
  totalBytes: number
  freedBytes: number
  never: string
  note: string | null
}

const num = (v: unknown): number | null => (typeof v === 'number' && Number.isFinite(v) ? v : null)
const str = (v: unknown): string | null => (typeof v === 'string' && v ? v : null)
const obj = (v: unknown): Record<string, unknown> => (v && typeof v === 'object' ? (v as Record<string, unknown>) : {})
const arr = (v: unknown): unknown[] => (Array.isArray(v) ? v : [])
const LEVELS: readonly Level[] = ['ok', 'warn', 'refuse', 'critical']

function parseLines(v: unknown): Lines {
  const l = obj(v)
  return {
    warnPct: num(l.warnPct) ?? 85,
    warnFreeBytes: num(l.warnFreeBytes) ?? 0,
    refusePct: num(l.refusePct) ?? 95,
    reserveBytes: num(l.reserveBytes) ?? 0,
    criticalPct: num(l.criticalPct) ?? 98,
    criticalFreeBytes: num(l.criticalFreeBytes) ?? 0,
  }
}

/** `GET /cli/app/resources` → what the panel draws. */
export function parseUsage(raw: unknown): BoxUsage {
  const v = obj(raw)
  const g = obj(v.guard)
  const mode = g.mode === 'on' || g.mode === 'warn' || g.mode === 'off' ? g.mode : 'off'
  const disks: Disk[] = arr(v.disks).map((d0) => {
    const d = obj(d0)
    const level = LEVELS.includes(d.level as Level) ? (d.level as Level) : 'ok'
    return {
      id: str(d.id) ?? '',
      mount: str(d.mount) ?? str(d.id) ?? '?',
      totalBytes: num(d.totalBytes) ?? 0,
      usedBytes: num(d.usedBytes) ?? 0,
      freeBytes: num(d.freeBytes) ?? 0,
      usedPct: num(d.usedPct) ?? 0,
      level,
      holds: arr(d.holds).filter((h): h is string => typeof h === 'string'),
      notMeasuredBytes: num(d.notMeasuredBytes),
    }
  })
  const rows: UsageRow[] = arr(v.consumers).map((c0) => {
    const c = obj(c0)
    const o = obj(c.owner)
    const kind = (['app', 'service', 'database', 'workspace', 'shared'] as const).find((k) => k === c.kind) ?? 'shared'
    return {
      kind,
      id: str(c.id) ?? '?',
      owner: str(o.handle),
      ownerAddress: str(o.address),
      diskBytes: num(c.diskBytes),
      memBytes: num(c.memBytes),
      budgetBytes: num(c.budgetBytes),
      folder: str(c.folder),
    }
  })
  const owners: OwnerTotal[] = arr(v.byOwner).map((t0) => {
    const t = obj(t0)
    const o = obj(t.owner)
    return { handle: str(o.handle), address: str(o.address), diskBytes: num(t.diskBytes) ?? 0 }
  })
  const mem = obj(v.mem)
  const box = obj(v.box)
  const scan = obj(v.scan)
  return {
    host: str(box.host),
    mode,
    modeSource: str(g.modeSource) ?? 'default',
    lines: parseLines(g.lines),
    disks,
    memTotalBytes: num(mem.totalBytes),
    memAvailableBytes: num(mem.availableBytes),
    load5: num(obj(v.cpu).load5),
    cores: num(box.cores),
    rows,
    owners,
    k2WillClearBytes: num(obj(v.k2WillClear).seedsBytes),
    scannedAt: num(scan.workspacesAt),
    scanRunning: scan.running === true,
  }
}

/** `GET /cli/app/resources/trends` → the box's points and each App's summary. */
export function parseTrends(raw: unknown): Trends {
  const v = obj(raw)
  const b = obj(v.box)
  const point = (p0: unknown): TrendPoint => {
    const p = obj(p0)
    return { at: num(p.at) ?? 0, cpuCores: num(p.cpuCores), memBytes: num(p.memBytes) }
  }
  return {
    window: str(v.window) ?? '24h',
    cores: num(b.cores),
    memTotalBytes: num(b.memTotalBytes),
    box: arr(b.points).map(point),
    apps: arr(v.apps).map((a0) => {
      const a = obj(a0)
      const s = obj(a.summary)
      return {
        app: str(a.app) ?? '?',
        owner: str(obj(a.owner).handle),
        memBudgetBytes: num(a.memBudgetBytes),
        memPeakBytes: num(s.memPeakBytes),
        cpuAvgCores: num(s.cpuAvgCores),
      }
    }),
  }
}

/** `POST /cli/app/resources/clean` → the items. */
export function parseClean(raw: unknown): CleanResult {
  const v = obj(raw)
  const t = obj(v.totals)
  return {
    dryRun: v.dryRun === true,
    scope: v.scope === 'workspace' ? 'workspace' : 'box',
    items: arr(v.items).map((i0) => {
      const i = obj(i0)
      return {
        kind: str(i.kind) ?? '?',
        id: str(i.id) ?? '?',
        app: str(i.app),
        bytes: num(i.bytes),
        removed: i.removed === true,
        error: str(i.error),
      }
    }),
    totalBytes: num(t.bytes) ?? 0,
    freedBytes: num(t.freedBytes) ?? 0,
    never: str(v.never) ?? '',
    note: str(obj(v.engine).note),
  }
}

export async function fetchUsage(): Promise<BoxUsage> {
  return parseUsage(await daemonCliGet<unknown>(primaryScope(), 'app/resources'))
}

export async function fetchTrends(window = '24h'): Promise<Trends> {
  return parseTrends(await daemonCliGet<unknown>(primaryScope(), 'app/resources/trends', { window }))
}

export async function rescan(): Promise<void> {
  await daemonCliPost<unknown>(primaryScope(), 'app/resources/rescan', {})
}

export async function clean(dryRun: boolean): Promise<CleanResult> {
  return parseClean(await daemonCliPost<unknown>(primaryScope(), 'app/resources/clean', { dryRun }))
}

// ── words and shapes ─────────────────────────────────────────────────

/** Decimal sizes, as `k2 app resources` prints them: 53 GB, 1.4 GB, 50 MB. */
export function sizeLabel(b: number | null): string {
  if (b === null) return '—'
  for (const [unit, size] of [['TB', 1e12], ['GB', 1e9], ['MB', 1e6], ['KB', 1e3]] as const) {
    if (b >= size) {
      const x = b / size
      return `${x < 10 ? x.toFixed(1) : Math.round(x)} ${unit}`
    }
  }
  return `${Math.round(b)} B`
}

export const LEVEL_WORDS: Record<Level, string> = { ok: 'OK', warn: 'Warn', refuse: 'Refusing new work', critical: 'Critical' }

/** The bar colour per level (theme tokens). */
export const LEVEL_COLOR: Record<Level, string> = {
  ok: 'var(--color-status-ok)',
  warn: 'var(--color-status-warn-amber)',
  refuse: 'var(--color-status-warn)',
  critical: 'var(--color-status-error)',
}

export const CLEAN_WORDS: Record<string, string> = {
  seed: 'Unused Seed (K2 copy)',
  engineSeed: 'Unused Seed (walled copy)',
  image: 'Unused image',
  engineLeftover: 'Engine leftover',
  rotatedLog: 'Rotated log',
  deploymentLog: 'Old deployment log',
  buildTemp: 'Build temp',
  imageArchive: 'Old base image archive',
}

/** The biggest measured rows, biggest first. */
export function biggestRows(rows: UsageRow[], n: number): UsageRow[] {
  return rows
    .filter((r) => r.diskBytes !== null)
    .sort((a, b) => (b.diskBytes ?? 0) - (a.diskBytes ?? 0) || a.id.localeCompare(b.id))
    .slice(0, n)
}

/** An SVG polyline's points for a series in a `w`×`h` box (`top` = the
 *  value at the top edge). Unknown points are skipped. */
export function sparkPoints(values: (number | null)[], w: number, h: number, top: number | null): string {
  const known = values.filter((x): x is number => x !== null)
  if (known.length < 2) return ''
  const hi = top && top > 0 ? top : Math.max(...known) || 1
  const step = values.length > 1 ? w / (values.length - 1) : w
  const out: string[] = []
  values.forEach((x, i) => {
    if (x === null) return
    const y = h - Math.min(1, Math.max(0, x / hi)) * h
    out.push(`${(i * step).toFixed(1)},${y.toFixed(1)}`)
  })
  return out.join(' ')
}
