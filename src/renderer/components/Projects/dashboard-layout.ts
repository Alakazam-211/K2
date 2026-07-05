// Projects V1 P5 (prd-projects-v1 §6.2/§6.3) — pure logic for the
// dashboard's canonical layout blob (`project_group_dashboards.
// layout_json`). No React/Tauri imports so every rule here is
// unit-testable in isolation (dashboard-layout.test.ts):
//
//   - parse/serialize of the versioned v1 blob
//     `{"version":1,"columns":[{"widthPct",pane:{kind,…}}]}` — `kind`
//     is the discriminator; UNKNOWN kinds round-trip untouched and
//     render an inert placeholder (§6.3 forward-compat),
//   - the untouched-vs-emptied distinction that drives the PoC seed:
//     the daemon creates 'Main' as `{"version":1,"panes":[]}` (no
//     `columns` key — project_groups.rs EMPTY_LAYOUT_V1), which
//     renders as the PoC's canonical pane; a layout SAVED with
//     `columns: []` was deliberately emptied and stays empty,
//   - percent-width math (proportional insert rebalance, remove
//     renormalize, pairwise resize with the ~15% floor),
//   - drop-target resolution (insertion boundaries + replace zones,
//     §6.2) and the one-terminal-pane-per-agent drop policy,
//   - apply-on-open freshness: `layout-changed` revisions only mark an
//     OPEN dashboard stale (never live-rearrange), with the echo guard
//     that keeps a client's own save from flagging its own view,
//   - the trailing-window coalesced saver (every change saves
//     canonically; N rapid changes → one POST — the house 300ms idiom).

// ── The v1 blob shape (§6.3) ──────────────────────────────────────────────

export const LAYOUT_VERSION = 1

/** Trailing coalesce window for layout saves (the FeedbackItemView /
 *  project-groups 300ms house idiom). */
export const LAYOUT_SAVE_COALESCE_MS = 300

/** Resize floor — a column can't be dragged below ~15% (§6.2). When a
 *  pair's combined share is already under 2×15 (7+ columns), the floor
 *  degrades to an even split of the pair so resize never wedges. */
export const MIN_COLUMN_PCT = 15

export interface TerminalPaneSpec {
  kind: 'terminal'
  /** `projects.id` — the workspace whose CANONICAL session renders. */
  workspaceId: string
}

export interface HtmlDocPaneSpec {
  kind: 'htmlDoc'
  workspaceId: string
  /** Absolute path — exactly how #587 addresses pinned files. */
  filePath: string
}

/** A pane whose `kind` this build doesn't know (or a known kind with a
 *  malformed body). Rendered as an inert placeholder and preserved
 *  byte-for-byte across saves — forward-compat per §6.3. */
export type UnknownPaneSpec = { kind: string } & Record<string, unknown>

export type PaneSpec = TerminalPaneSpec | HtmlDocPaneSpec | UnknownPaneSpec

export interface DashboardColumn {
  widthPct: number
  pane: PaneSpec
}

export function isTerminalPane(pane: PaneSpec): pane is TerminalPaneSpec {
  return pane.kind === 'terminal' && typeof (pane as TerminalPaneSpec).workspaceId === 'string'
}

export function isHtmlDocPane(pane: PaneSpec): pane is HtmlDocPaneSpec {
  return (
    pane.kind === 'htmlDoc' &&
    typeof (pane as HtmlDocPaneSpec).workspaceId === 'string' &&
    typeof (pane as HtmlDocPaneSpec).filePath === 'string'
  )
}

// ── Parse / serialize ─────────────────────────────────────────────────────

export type ParsedLayout =
  /** No `columns` key was ever written — the daemon's EMPTY_LAYOUT_V1
   *  seed (or an unparseable blob). Renders as the PoC seed (§6.2). */
  | { kind: 'untouched' }
  /** A renderer-written layout — `columns: []` means deliberately
   *  emptied, NOT fresh. */
  | { kind: 'layout'; columns: DashboardColumn[] }

/** Normalize widths to sum 100, preserving proportions. Non-positive /
 *  non-finite widths degrade to an equal split (never break a layout
 *  over bad numbers). */
export function normalizeWidths(columns: DashboardColumn[]): DashboardColumn[] {
  if (columns.length === 0) return columns
  const sane = columns.every((c) => Number.isFinite(c.widthPct) && c.widthPct > 0)
  if (!sane) {
    const equal = 100 / columns.length
    return columns.map((c) => ({ ...c, widthPct: equal }))
  }
  const sum = columns.reduce((acc, c) => acc + c.widthPct, 0)
  if (Math.abs(sum - 100) < 0.01) return columns
  return columns.map((c) => ({ ...c, widthPct: (c.widthPct / sum) * 100 }))
}

/** Parse a stored layout_json blob. Never throws; anything that isn't
 *  a `columns`-bearing object reads as `untouched`. Malformed pane
 *  bodies survive as unknown panes (inert, preserved on save). */
export function parseDashboardLayout(layoutJson: string): ParsedLayout {
  let raw: unknown
  try {
    raw = JSON.parse(layoutJson)
  } catch {
    return { kind: 'untouched' }
  }
  if (raw === null || typeof raw !== 'object' || Array.isArray(raw)) return { kind: 'untouched' }
  const cols = (raw as Record<string, unknown>).columns
  if (!Array.isArray(cols)) return { kind: 'untouched' }
  const columns: DashboardColumn[] = []
  for (const entry of cols) {
    if (entry === null || typeof entry !== 'object' || Array.isArray(entry)) continue
    const pane = (entry as Record<string, unknown>).pane
    if (pane === null || typeof pane !== 'object' || Array.isArray(pane)) continue
    if (typeof (pane as Record<string, unknown>).kind !== 'string') continue
    const widthPct = (entry as Record<string, unknown>).widthPct
    columns.push({
      widthPct: typeof widthPct === 'number' ? widthPct : NaN,
      pane: pane as PaneSpec,
    })
  }
  return { kind: 'layout', columns: normalizeWidths(columns) }
}

/** Serialize columns as the versioned v1 blob. Widths round to 2
 *  decimals (stable diffs; sub-percent precision is invisible). */
export function serializeDashboardLayout(columns: DashboardColumn[]): string {
  return JSON.stringify({
    version: LAYOUT_VERSION,
    columns: columns.map((c) => ({
      widthPct: Math.round(c.widthPct * 100) / 100,
      pane: c.pane,
    })),
  })
}

// ── Seed / adopt (§6.2 initialization + §6.3 apply-on-open) ──────────────

/** The default a never-touched dashboard renders: ONLY the PoC's
 *  canonical terminal pane. Memberless group → no panes. */
export function seedColumns(pocWorkspaceId: string | null): DashboardColumn[] {
  if (!pocWorkspaceId) return []
  return [{ widthPct: 100, pane: { kind: 'terminal', workspaceId: pocWorkspaceId } }]
}

/** Apply-on-open: what an opening dashboard renders from its stored
 *  blob. Untouched → the client-side PoC seed (NOT written back — the
 *  first real edit writes the canonical layout); anything saved →
 *  exactly as saved, including a deliberate `columns: []`. */
export function adoptLayout(layoutJson: string, pocWorkspaceId: string | null): DashboardColumn[] {
  const parsed = parseDashboardLayout(layoutJson)
  if (parsed.kind === 'untouched') return seedColumns(pocWorkspaceId)
  return parsed.columns
}

// ── Column mutations (pure — callers save the result) ────────────────────

/** Insert a pane as a new column at `index` (0..len). The new column
 *  takes an equal 1/(n+1) share; existing columns rebalance
 *  proportionally (§6.2). */
export function insertColumnAt(
  columns: DashboardColumn[],
  index: number,
  pane: PaneSpec,
): DashboardColumn[] {
  const clamped = Math.max(0, Math.min(index, columns.length))
  const share = 100 / (columns.length + 1)
  const scaled = columns.map((c) => ({ ...c, widthPct: c.widthPct * ((100 - share) / 100) }))
  scaled.splice(clamped, 0, { widthPct: share, pane })
  return normalizeWidths(scaled)
}

/** Remove the column at `index`; survivors renormalize proportionally. */
export function removeColumnAt(columns: DashboardColumn[], index: number): DashboardColumn[] {
  if (index < 0 || index >= columns.length) return columns
  const next = columns.filter((_, i) => i !== index)
  return normalizeWidths(next)
}

/** Swap the pane in place; the column keeps its width (§6.2 replace). */
export function replaceColumnPane(
  columns: DashboardColumn[],
  index: number,
  pane: PaneSpec,
): DashboardColumn[] {
  if (index < 0 || index >= columns.length) return columns
  return columns.map((c, i) => (i === index ? { ...c, pane } : c))
}

/** Move a column to a BOUNDARY index (0..len — the Sidebar drop-index
 *  rule): dropping on either boundary of its own slot is a no-op and
 *  returns the ORIGINAL array reference (callers use identity to skip
 *  the save). */
export function moveColumn(
  columns: DashboardColumn[],
  from: number,
  toBoundary: number,
): DashboardColumn[] {
  if (from < 0 || from >= columns.length) return columns
  const di = Math.max(0, Math.min(toBoundary, columns.length))
  if (di === from || di === from + 1) return columns
  const next = [...columns]
  const [item] = next.splice(from, 1)
  next.splice(di > from ? di - 1 : di, 0, item)
  return next
}

/** Drag-resize the boundary between columns `leftIndex` and
 *  `leftIndex+1` by `deltaPct` (from the drag-START widths). The pair's
 *  combined share is conserved; each side floors at MIN_COLUMN_PCT
 *  (degrading to an even split when the pair is already tighter). */
export function resizePair(
  columns: DashboardColumn[],
  leftIndex: number,
  deltaPct: number,
): DashboardColumn[] {
  if (leftIndex < 0 || leftIndex + 1 >= columns.length) return columns
  const left = columns[leftIndex]
  const right = columns[leftIndex + 1]
  const pair = left.widthPct + right.widthPct
  const floor = Math.min(MIN_COLUMN_PCT, pair / 2)
  const newLeft = Math.max(floor, Math.min(left.widthPct + deltaPct, pair - floor))
  return columns.map((c, i) => {
    if (i === leftIndex) return { ...c, widthPct: newLeft }
    if (i === leftIndex + 1) return { ...c, widthPct: pair - newLeft }
    return c
  })
}

/** Index of the terminal column for `workspaceId`, or -1 (one terminal
 *  pane per agent per dashboard — §6.3). */
export function findTerminalColumn(columns: DashboardColumn[], workspaceId: string): number {
  return columns.findIndex((c) => isTerminalPane(c.pane) && c.pane.workspaceId === workspaceId)
}

// ── Drop targets (§6.2) ───────────────────────────────────────────────────

export type DropTarget =
  /** Insert a new column at boundary `index` (0..len). */
  | { type: 'insert'; index: number }
  /** Replace the pane of the column at `index`. */
  | { type: 'replace'; index: number }

/** Resolve where a drag at viewport-x `x` lands, given the columns'
 *  horizontal rects (left→right order). Zones per §6.2: insertion
 *  markers at every boundary + both edges; a column's INNER body is a
 *  replace target. The edge zone is `edgePx`, shrinking to a quarter of
 *  a narrow column so its replace zone never vanishes. No columns →
 *  insert at 0. */
export function computeDropTarget(
  x: number,
  columnRects: Array<{ left: number; right: number }>,
  edgePx = 48,
): DropTarget {
  const n = columnRects.length
  if (n === 0) return { type: 'insert', index: 0 }
  if (x < columnRects[0].left) return { type: 'insert', index: 0 }
  for (let i = 0; i < n; i++) {
    const { left, right } = columnRects[i]
    if (x > right) continue
    const zone = Math.min(edgePx, (right - left) / 4)
    if (x <= left + zone) return { type: 'insert', index: i }
    if (x >= right - zone) return { type: 'insert', index: i + 1 }
    return { type: 'replace', index: i }
  }
  return { type: 'insert', index: n }
}

/** Boundary-only variant for REORDER drags (a pane header): the
 *  midpoint rule — the boundary index is the count of columns whose
 *  midpoint sits left of `x` (Sidebar's drop-index pattern). */
export function computeInsertBoundary(
  x: number,
  columnRects: Array<{ left: number; right: number }>,
): number {
  let idx = 0
  for (const rect of columnRects) {
    if (x > (rect.left + rect.right) / 2) idx++
  }
  return idx
}

/** Apply a member drop honoring the one-terminal-pane-per-agent policy:
 *  a fresh member inserts/replaces normally; an already-present member
 *  MOVES on an insert drop and just FOCUSES on a replace drop (never a
 *  duplicate, never a surprise removal). `changed === false` ⇒ nothing
 *  to save; `focusIndex` is the column to flash/focus. */
export function dropMember(
  columns: DashboardColumn[],
  workspaceId: string,
  target: DropTarget,
): { columns: DashboardColumn[]; focusIndex: number; changed: boolean } {
  const existing = findTerminalColumn(columns, workspaceId)
  const pane: TerminalPaneSpec = { kind: 'terminal', workspaceId }
  if (target.type === 'insert') {
    if (existing >= 0) {
      const next = moveColumn(columns, existing, target.index)
      return {
        columns: next,
        focusIndex: findTerminalColumn(next, workspaceId),
        changed: next !== columns,
      }
    }
    return {
      columns: insertColumnAt(columns, target.index, pane),
      focusIndex: Math.max(0, Math.min(target.index, columns.length)),
      changed: true,
    }
  }
  if (existing >= 0) return { columns, focusIndex: existing, changed: false }
  return { columns: replaceColumnPane(columns, target.index, pane), focusIndex: target.index, changed: true }
}

// ── Apply-on-open freshness + echo guard (§6.3) ──────────────────────────
//
// `known` = the highest revision this client has RENDERED or WRITTEN.
// Incoming revisions (layout-changed events / show refetches) beyond it
// mark the open dashboard STALE — they never rearrange it live. A save
// response ratchets `known` FORWARD, so the echo of our own write
// (event → refetch → same revision) can never flag our own view; it
// also clears a stale flag our write just superseded (last-write-wins:
// the canonical layout is now ours).

export interface FreshnessState {
  known: number
  /** Newest revision seen beyond `known`, or null when fresh. */
  staleRevision: number | null
}

export function initialFreshness(revision: number): FreshnessState {
  return { known: revision, staleRevision: null }
}

/** An incoming revision observed (event payload or refetched row). */
export function observeRevision(s: FreshnessState, incoming: number): FreshnessState {
  if (incoming <= s.known) return s
  return { ...s, staleRevision: Math.max(incoming, s.staleRevision ?? 0) }
}

/** Our own save-layout response (echo guard + supersede). */
export function observeOwnSave(s: FreshnessState, saved: number): FreshnessState {
  const known = Math.max(s.known, saved)
  const staleRevision = s.staleRevision !== null && s.staleRevision > known ? s.staleRevision : null
  return { known, staleRevision }
}

/** The stale layout was applied (open/switch or the stale pill). */
export function adoptRevision(s: FreshnessState, revision: number): FreshnessState {
  return { known: Math.max(s.known, revision), staleRevision: null }
}

// ── Coalesced canonical saver (§6.3a) ─────────────────────────────────────

export interface LayoutSaver {
  /** Record the latest columns and (re)arm the trailing window. */
  schedule(columns: DashboardColumn[]): void
  /** Fire any pending save immediately (best-effort) and stop
   *  delivering callbacks — the unmount flush. */
  dispose(): void
}

/** Every layout change saves canonically, but a burst (drag + resize +
 *  close in quick succession) coalesces on a trailing window into ONE
 *  POST carrying the LATEST columns. Saves are single-flight: a change
 *  landing mid-POST queues exactly one follow-up. */
export function createLayoutSaver(
  save: (layoutJson: string) => Promise<{ revision: number }>,
  hooks: {
    onSaved: (revision: number) => void
    onError: (err: unknown) => void
  },
  delayMs: number = LAYOUT_SAVE_COALESCE_MS,
): LayoutSaver {
  let pending: DashboardColumn[] | null = null
  let timer: ReturnType<typeof setTimeout> | null = null
  let inFlight = false
  let disposed = false

  const fire = (): void => {
    if (pending === null || inFlight) return
    const cols = pending
    pending = null
    inFlight = true
    save(serializeDashboardLayout(cols))
      .then((r) => {
        if (!disposed) hooks.onSaved(r.revision)
      })
      .catch((err) => {
        if (!disposed) hooks.onError(err)
      })
      .finally(() => {
        inFlight = false
        // A change landed while we were posting — save it too (it is
        // the newest layout; last-write-wins makes this convergent).
        if (pending !== null) fire()
      })
  }

  return {
    schedule(columns: DashboardColumn[]): void {
      if (disposed) return
      pending = columns
      if (timer !== null) clearTimeout(timer)
      timer = setTimeout(() => {
        timer = null
        fire()
      }, delayMs)
    },
    dispose(): void {
      if (timer !== null) clearTimeout(timer)
      timer = null
      fire()
      disposed = true
    },
  }
}
