// Projects V1 P5 → §6.8 (prd-projects-v1) — pure logic for the
// dashboard's canonical layout blob (`project_group_dashboards.
// layout_json`). No React/Tauri imports so every rule here is
// unit-testable in isolation (dashboard-layout.test.ts):
//
//   - the §6.8 v2 TREE model: `{version:2, root:<node>}` where a node
//     is a split (row|col, children with percent sizes) or a pane
//     (§6.3 PaneSpec — `kind` is the discriminator; UNKNOWN kinds
//     round-trip untouched and render an inert placeholder),
//   - v1 blobs (`{"version":1,"columns":[…]}`) parse and CONVERT
//     (columns → one row-split) on adopt; saves always write v2,
//   - the untouched-vs-emptied distinction that drives the PoC seed:
//     the daemon creates 'Main' as `{"version":1,"panes":[]}` (no
//     `columns`/`root` key — project_groups.rs EMPTY_LAYOUT_V1), which
//     renders as the PoC's canonical pane; a layout SAVED with
//     `root: null` (or v1 `columns: []`) was deliberately emptied and
//     stays empty,
//   - tree ops: insertSplit (drop-to-split 50/50), insertEdge (the
//     dashboard far-edge full-span insert), movePane (an existing pane
//     MOVES, never duplicates), swapPanes (center drop of a pane),
//     removePane (collapse single-child splits, renormalize),
//     resizeDivider (live divider drag, ~10% floor), normalize (merge
//     nested same-dir splits, sizes sum 100), tileIntoPreset (the
//     presets menu — reading-order re-tile),
//   - geometry (tree → percent rects + divider positions) and the
//     5-zone drop hit-test (left/right/top/bottom/center + container
//     edge bands) with the drop POLICY (`applyDrop`): duplicate
//     member/doc = focus, existing pane = move/swap,
//   - apply-on-open freshness: `layout-changed` revisions only mark an
//     OPEN dashboard stale (never live-rearrange), with the echo guard
//     that keeps a client's own save from flagging its own view,
//   - the trailing-window coalesced saver (every change saves
//     canonically; N rapid changes → one POST — the house 300ms idiom).

// ── The blob shapes (§6.3 panes, §6.8 tree) ───────────────────────────────

export const LAYOUT_VERSION = 2

/** Trailing coalesce window for layout saves (the FeedbackItemView /
 *  project-groups 300ms house idiom). */
export const LAYOUT_SAVE_COALESCE_MS = 300

/** Resize floor — a pane can't be dragged below ~10% of its split
 *  (§6.8.3). When a pair's combined share is already under 2×10, the
 *  floor degrades to an even split of the pair so resize never
 *  wedges. */
export const MIN_PANE_PCT = 10

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

// The §6.8 tree — a node is a split or a pane. Sizes are percents
// summing ~100 per split.

export type SplitDir = 'row' | 'col'
export type Side = 'left' | 'right' | 'top' | 'bottom'

export interface SplitChild {
  size: number
  node: LayoutNode
}

export interface SplitNode {
  type: 'split'
  dir: SplitDir
  children: SplitChild[]
}

export interface PaneNode {
  type: 'pane'
  pane: PaneSpec
}

export type LayoutNode = SplitNode | PaneNode

/** The v1 single-row column model — parsed only to CONVERT (§6.8.1). */
export interface DashboardColumn {
  widthPct: number
  pane: PaneSpec
}

// ── Pane identity ─────────────────────────────────────────────────────────
//
// Panes are identified by their SPEC (kind + workspaceId + filePath),
// not a stored id — the blob stays exactly the §6.3/§6.8 shape.
// Duplicate specs (possible only for unknown kinds / hand-written
// blobs) disambiguate by reading-order occurrence (`#n`).

export function paneKey(pane: PaneSpec): string {
  if (isTerminalPane(pane)) return `t:${pane.workspaceId}`
  if (isHtmlDocPane(pane)) return `h:${pane.workspaceId}:${pane.filePath}`
  return `u:${pane.kind}`
}

export interface PaneEntry {
  /** paneKey, `#n`-suffixed for duplicate specs (reading order). */
  paneId: string
  pane: PaneSpec
  /** Child indices from the root. */
  path: number[]
}

/** All panes in READING ORDER (depth-first, children in order — the
 *  order `tileIntoPreset` re-tiles into a shape, §6.8.4). */
export function layoutPanes(root: LayoutNode | null): PaneEntry[] {
  const entries: PaneEntry[] = []
  if (!root) return entries
  const seen = new Map<string, number>()
  const walk = (node: LayoutNode, path: number[]): void => {
    if (node.type === 'pane') {
      const base = paneKey(node.pane)
      const n = seen.get(base) ?? 0
      seen.set(base, n + 1)
      entries.push({ paneId: n === 0 ? base : `${base}#${n}`, pane: node.pane, path })
      return
    }
    node.children.forEach((child, i) => walk(child.node, [...path, i]))
  }
  walk(root, [])
  return entries
}

export function readingOrder(root: LayoutNode | null): PaneSpec[] {
  return layoutPanes(root).map((e) => e.pane)
}

/** paneId of the terminal pane for `workspaceId`, or null (one
 *  terminal pane per agent per dashboard — §6.3). */
export function findTerminalPaneId(root: LayoutNode | null, workspaceId: string): string | null {
  const id = `t:${workspaceId}`
  return layoutPanes(root).some((e) => e.paneId === id) ? id : null
}

// ── Normalize ─────────────────────────────────────────────────────────────

/** Normalize sizes to sum 100, preserving proportions. Non-positive /
 *  non-finite sizes degrade to an equal split (never break a layout
 *  over bad numbers). */
function normalizeSizes(children: SplitChild[]): SplitChild[] {
  if (children.length === 0) return children
  const sane = children.every((c) => Number.isFinite(c.size) && c.size > 0)
  if (!sane) {
    const equal = 100 / children.length
    return children.map((c) => ({ ...c, size: equal }))
  }
  const sum = children.reduce((acc, c) => acc + c.size, 0)
  if (Math.abs(sum - 100) < 0.01) return children
  return children.map((c) => ({ ...c, size: (c.size / sum) * 100 }))
}

/** Canonical form (§6.8.1): nested same-dir splits merge into their
 *  parent (child sizes scale by the child split's share), single-child
 *  splits collapse to the child, and every split's sizes sum 100. */
export function normalizeNode(node: LayoutNode): LayoutNode {
  if (node.type === 'pane') return node
  const flat: SplitChild[] = []
  for (const child of normalizeSizes(node.children)) {
    const inner = normalizeNode(child.node)
    if (inner.type === 'split' && inner.dir === node.dir) {
      for (const grand of inner.children) {
        flat.push({ size: (child.size * grand.size) / 100, node: grand.node })
      }
    } else {
      flat.push({ size: child.size, node: inner })
    }
  }
  if (flat.length === 1) return flat[0].node
  return { type: 'split', dir: node.dir, children: normalizeSizes(flat) }
}

// ── Parse / serialize ─────────────────────────────────────────────────────

export type ParsedLayout =
  /** No `root`/`columns` key was ever written — the daemon's
   *  EMPTY_LAYOUT_V1 seed (or an unparseable blob). Renders as the
   *  PoC seed (§6.2). */
  | { kind: 'untouched' }
  /** A renderer-written layout — `root: null` (or v1 `columns: []`)
   *  means deliberately emptied, NOT fresh. */
  | { kind: 'layout'; root: LayoutNode | null }

function isPlainObject(v: unknown): v is Record<string, unknown> {
  return v !== null && typeof v === 'object' && !Array.isArray(v)
}

function parsePaneSpec(raw: unknown): PaneSpec | null {
  if (!isPlainObject(raw) || typeof raw.kind !== 'string') return null
  return raw as PaneSpec
}

/** Parse a v2 node; anything unusable reads as null. Malformed
 *  children are skipped; empty splits vanish; malformed pane bodies
 *  survive as unknown panes (inert, preserved on save). */
function parseNode(raw: unknown): LayoutNode | null {
  if (!isPlainObject(raw)) return null
  if (raw.type === 'pane') {
    const pane = parsePaneSpec(raw.pane)
    return pane ? { type: 'pane', pane } : null
  }
  if (raw.type === 'split') {
    const dir = raw.dir
    if (dir !== 'row' && dir !== 'col') return null
    if (!Array.isArray(raw.children)) return null
    const children: SplitChild[] = []
    for (const entry of raw.children) {
      if (!isPlainObject(entry)) continue
      const node = parseNode(entry.node)
      if (!node) continue
      children.push({ size: typeof entry.size === 'number' ? entry.size : NaN, node })
    }
    if (children.length === 0) return null
    return { type: 'split', dir, children }
  }
  return null
}

/** v1 → v2 (§6.8.1): columns become ONE row-split (a single column is
 *  just its pane; no columns is the deliberately-emptied null root). */
function convertV1Columns(cols: unknown[]): LayoutNode | null {
  const columns: DashboardColumn[] = []
  for (const entry of cols) {
    if (!isPlainObject(entry)) continue
    const pane = parsePaneSpec(entry.pane)
    if (!pane) continue
    columns.push({
      widthPct: typeof entry.widthPct === 'number' ? entry.widthPct : NaN,
      pane,
    })
  }
  if (columns.length === 0) return null
  if (columns.length === 1) return { type: 'pane', pane: columns[0].pane }
  return normalizeNode({
    type: 'split',
    dir: 'row',
    children: columns.map((c) => ({ size: c.widthPct, node: { type: 'pane', pane: c.pane } })),
  })
}

/** Parse a stored layout_json blob (v2 or v1). Never throws; anything
 *  that isn't a `root`/`columns`-bearing object reads as `untouched`. */
export function parseDashboardLayout(layoutJson: string): ParsedLayout {
  let raw: unknown
  try {
    raw = JSON.parse(layoutJson)
  } catch {
    return { kind: 'untouched' }
  }
  if (!isPlainObject(raw)) return { kind: 'untouched' }
  if ('root' in raw) {
    if (raw.root === null) return { kind: 'layout', root: null }
    const node = parseNode(raw.root)
    // An unreadable root reads as untouched — never break a dashboard
    // over a mangled blob (the §6.3 never-break discipline).
    return node ? { kind: 'layout', root: normalizeNode(node) } : { kind: 'untouched' }
  }
  if (Array.isArray(raw.columns)) {
    return { kind: 'layout', root: convertV1Columns(raw.columns) }
  }
  return { kind: 'untouched' }
}

const round2 = (n: number): number => Math.round(n * 100) / 100

function serializeNode(node: LayoutNode): unknown {
  if (node.type === 'pane') return { type: 'pane', pane: node.pane }
  return {
    type: 'split',
    dir: node.dir,
    children: node.children.map((c) => ({ size: round2(c.size), node: serializeNode(c.node) })),
  }
}

/** Serialize the tree as the versioned v2 blob (§6.8.1 — saves always
 *  write v2). Sizes round to 2 decimals (stable diffs; sub-percent
 *  precision is invisible). */
export function serializeDashboardLayout(root: LayoutNode | null): string {
  return JSON.stringify({ version: LAYOUT_VERSION, root: root ? serializeNode(root) : null })
}

// ── Seed / adopt (§6.2 initialization + §6.3 apply-on-open) ──────────────

/** The default a never-touched dashboard renders: ONLY the PoC's
 *  canonical terminal pane. Memberless group → no panes. */
export function seedRoot(pocWorkspaceId: string | null): LayoutNode | null {
  if (!pocWorkspaceId) return null
  return { type: 'pane', pane: { kind: 'terminal', workspaceId: pocWorkspaceId } }
}

/** Apply-on-open: what an opening dashboard renders from its stored
 *  blob. Untouched → the client-side PoC seed (NOT written back — the
 *  first real edit writes the canonical layout); anything saved →
 *  exactly as saved, including a deliberate `root: null`. */
export function adoptLayout(
  layoutJson: string,
  pocWorkspaceId: string | null,
): LayoutNode | null {
  const parsed = parseDashboardLayout(layoutJson)
  if (parsed.kind === 'untouched') return seedRoot(pocWorkspaceId)
  return parsed.root
}

// ── Tree mutations (pure — callers save the result) ──────────────────────
//
// Every op returns the ORIGINAL root reference on a no-op (unknown
// ids, self-targets) so callers can use identity to skip the save.

function nodeAtPath(root: LayoutNode, path: number[]): LayoutNode | null {
  let node: LayoutNode = root
  for (const i of path) {
    if (node.type !== 'split') return null
    const child = node.children[i]
    if (!child) return null
    node = child.node
  }
  return node
}

function replaceAtPath(
  root: LayoutNode,
  path: number[],
  replace: (node: LayoutNode) => LayoutNode,
): LayoutNode {
  if (path.length === 0) return replace(root)
  if (root.type !== 'split') return root
  const [i, ...rest] = path
  return {
    ...root,
    children: root.children.map((c, j) =>
      j === i ? { ...c, node: replaceAtPath(c.node, rest, replace) } : c,
    ),
  }
}

/** Remove the node at `path` WITHOUT normalizing (single-child splits
 *  survive so sibling paths stay predictable — movePane relies on it).
 *  Null when the tree emptied. */
function removeAtPath(root: LayoutNode, path: number[]): LayoutNode | null {
  if (path.length === 0) return null
  if (root.type !== 'split') return root
  const [i, ...rest] = path
  const child = root.children[i]
  if (!child) return root
  const removed = removeAtPath(child.node, rest)
  const children =
    removed === null
      ? root.children.filter((_, j) => j !== i)
      : root.children.map((c, j) => (j === i ? { ...c, node: removed } : c))
  if (children.length === 0) return null
  return { ...root, children }
}

const sideDir = (side: Side): SplitDir => (side === 'left' || side === 'right' ? 'row' : 'col')
const sideBefore = (side: Side): boolean => side === 'left' || side === 'top'

/** Wrap `target` in a 50/50 split with `pane` on `side` — the §6.8.2
 *  drop-to-split. Normalization merges the new split into a same-dir
 *  parent, so the target's share simply halves. */
function splitAround(target: LayoutNode, side: Side, pane: PaneSpec): SplitNode {
  const fresh: LayoutNode = { type: 'pane', pane }
  const pair: SplitChild[] = sideBefore(side)
    ? [
        { size: 50, node: fresh },
        { size: 50, node: target },
      ]
    : [
        { size: 50, node: target },
        { size: 50, node: fresh },
      ]
  return { type: 'split', dir: sideDir(side), children: pair }
}

/** Split the pane `targetPaneId` 50/50 on `side`, the new `pane`
 *  taking the near half (§6.8.2). Unknown target → identity no-op. */
export function insertSplit(
  root: LayoutNode | null,
  targetPaneId: string,
  side: Side,
  pane: PaneSpec,
): LayoutNode | null {
  if (!root) return { type: 'pane', pane }
  const target = layoutPanes(root).find((e) => e.paneId === targetPaneId)
  if (!target) return root
  return normalizeNode(replaceAtPath(root, target.path, (node) => splitAround(node, side, pane)))
}

/** Insert `pane` as a full-height/width region on a dashboard far
 *  edge (§6.8.2): a same-dir root split takes it as an equal-share
 *  child at that end; anything else wraps 50/50. */
export function insertEdge(
  root: LayoutNode | null,
  side: Side,
  pane: PaneSpec,
): LayoutNode {
  if (!root) return { type: 'pane', pane }
  const dir = sideDir(side)
  if (root.type === 'split' && root.dir === dir) {
    const share = 100 / (root.children.length + 1)
    const scaled = root.children.map((c) => ({ ...c, size: c.size * ((100 - share) / 100) }))
    const fresh: SplitChild = { size: share, node: { type: 'pane', pane } }
    const children = sideBefore(side) ? [fresh, ...scaled] : [...scaled, fresh]
    return normalizeNode({ ...root, children })
  }
  return normalizeNode(splitAround(root, side, pane))
}

/** Remove the pane `paneId`: its split renormalizes proportionally and
 *  single-child splits collapse. Null when the last pane went. */
export function removePane(root: LayoutNode | null, paneId: string): LayoutNode | null {
  if (!root) return root
  const target = layoutPanes(root).find((e) => e.paneId === paneId)
  if (!target) return root
  const removed = removeAtPath(root, target.path)
  return removed === null ? null : normalizeNode(removed)
}

export type MoveTarget =
  | { kind: 'pane'; targetPaneId: string; side: Side }
  | { kind: 'edge'; side: Side }

/** After removing the child at `removedPath`, the same-parent sibling
 *  indices above it shift down one — adjust a path that survived. */
function adjustPathAfterRemove(path: number[], removedPath: number[]): number[] {
  const depth = removedPath.length - 1
  if (path.length <= depth) return path
  for (let i = 0; i < depth; i++) {
    if (path[i] !== removedPath[i]) return path
  }
  if (path[depth] > removedPath[depth]) {
    const next = [...path]
    next[depth] -= 1
    return next
  }
  return path
}

/** Move an EXISTING pane (drop of a pane = move, never duplicate —
 *  §6.8.2): remove it, then split around the target pane / insert at
 *  the edge. Self-target → identity no-op. */
export function movePane(
  root: LayoutNode | null,
  sourcePaneId: string,
  target: MoveTarget,
): LayoutNode | null {
  if (!root) return root
  const entries = layoutPanes(root)
  const source = entries.find((e) => e.paneId === sourcePaneId)
  if (!source) return root
  if (target.kind === 'pane' && target.targetPaneId === sourcePaneId) return root

  if (target.kind === 'edge') {
    const removed = removeAtPath(root, source.path)
    return insertEdge(removed === null ? null : normalizeNode(removed), target.side, source.pane)
  }

  const dest = entries.find((e) => e.paneId === target.targetPaneId)
  if (!dest) return root
  // Remove RAW (no normalize) so the destination's path only shifts by
  // the one predictable sibling-index decrement, then split around it
  // and normalize once.
  const removed = removeAtPath(root, source.path)
  if (removed === null) return root // source was the only pane — dest can't exist
  const destPath = adjustPathAfterRemove(dest.path, source.path)
  return normalizeNode(
    replaceAtPath(removed, destPath, (node) => splitAround(node, target.side, source.pane)),
  )
}

/** Swap two panes' specs in place (center drop of an existing pane —
 *  §6.8.2 "move/swap"); slots and sizes stay put. */
export function swapPanes(
  root: LayoutNode | null,
  aPaneId: string,
  bPaneId: string,
): LayoutNode | null {
  if (!root || aPaneId === bPaneId) return root
  const entries = layoutPanes(root)
  const a = entries.find((e) => e.paneId === aPaneId)
  const b = entries.find((e) => e.paneId === bPaneId)
  if (!a || !b) return root
  const swapped = replaceAtPath(root, a.path, () => ({ type: 'pane', pane: b.pane }))
  return replaceAtPath(swapped, b.path, () => ({ type: 'pane', pane: a.pane }))
}

/** Swap the pane's SPEC in place (a fresh drop on a pane's center
 *  replaces it — the §6.2 replace policy carried to v2); the slot
 *  keeps its size. */
export function replacePaneSpec(
  root: LayoutNode | null,
  paneId: string,
  pane: PaneSpec,
): LayoutNode | null {
  if (!root) return root
  const target = layoutPanes(root).find((e) => e.paneId === paneId)
  if (!target) return root
  return replaceAtPath(root, target.path, () => ({ type: 'pane', pane }))
}

/** Drag-resize the divider between children `index` and `index+1` of
 *  the split at `splitPath` by `deltaPct` (percent of the SPLIT's own
 *  span, from the drag-START tree). The pair's combined share is
 *  conserved; each side floors at `minPct` (degrading to an even split
 *  when the pair is already tighter). */
export function resizeDivider(
  root: LayoutNode | null,
  splitPath: number[],
  index: number,
  deltaPct: number,
  minPct: number = MIN_PANE_PCT,
): LayoutNode | null {
  if (!root) return root
  const split = nodeAtPath(root, splitPath)
  if (!split || split.type !== 'split') return root
  if (index < 0 || index + 1 >= split.children.length) return root
  const left = split.children[index]
  const right = split.children[index + 1]
  const pair = left.size + right.size
  const floor = Math.min(minPct, pair / 2)
  const newLeft = Math.max(floor, Math.min(left.size + deltaPct, pair - floor))
  return replaceAtPath(root, splitPath, (node) => {
    if (node.type !== 'split') return node
    return {
      ...node,
      children: node.children.map((c, i) => {
        if (i === index) return { ...c, size: newLeft }
        if (i === index + 1) return { ...c, size: pair - newLeft }
        return c
      }),
    }
  })
}

// ── Presets (§6.8.4) ──────────────────────────────────────────────────────

export type PresetShape = 'single' | 'cols2' | 'cols3' | 'grid2x2' | 'mainStack'

const paneNode = (pane: PaneSpec): PaneNode => ({ type: 'pane', pane })

const equalSplit = (dir: SplitDir, nodes: LayoutNode[]): LayoutNode => {
  if (nodes.length === 1) return nodes[0]
  const size = 100 / nodes.length
  return { type: 'split', dir, children: nodes.map((node) => ({ size, node })) }
}

/** A region holding several panes stacks them vertically. */
const stackRegion = (panes: PaneSpec[]): LayoutNode => equalSplit('col', panes.map(paneNode))

/** First k-1 regions take one pane each (reading order); EXTRA panes
 *  append into the LAST region; missing panes → fewer regions (empty
 *  slots are never persisted — §6.8.4). */
function chunkRegions(panes: PaneSpec[], k: number): PaneSpec[][] {
  const regions: PaneSpec[][] = []
  for (let i = 0; i < k - 1 && i < panes.length; i++) regions.push([panes[i]])
  const rest = panes.slice(Math.min(k - 1, panes.length))
  if (rest.length > 0) regions.push(rest)
  return regions
}

/** Re-tile `panes` (reading order) into a preset shape (§6.8.4). The
 *  shape collapses to what exists; no empty slots; no panes → null. */
export function tileIntoPreset(shape: PresetShape, panes: PaneSpec[]): LayoutNode | null {
  if (panes.length === 0) return null
  switch (shape) {
    case 'single':
      return normalizeNode(stackRegion(panes))
    case 'cols2':
    case 'cols3': {
      const regions = chunkRegions(panes, shape === 'cols2' ? 2 : 3)
      return normalizeNode(equalSplit('row', regions.map(stackRegion)))
    }
    case 'grid2x2': {
      const regions = chunkRegions(panes, 4)
      const rows: LayoutNode[] = []
      const top = regions.slice(0, 2).map(stackRegion)
      const bottom = regions.slice(2).map(stackRegion)
      if (top.length > 0) rows.push(equalSplit('row', top))
      if (bottom.length > 0) rows.push(equalSplit('row', bottom))
      return normalizeNode(equalSplit('col', rows))
    }
    case 'mainStack': {
      if (panes.length === 1) return paneNode(panes[0])
      return normalizeNode(
        equalSplit('row', [paneNode(panes[0]), stackRegion(panes.slice(1))]),
      )
    }
  }
}

// ── Geometry (tree → percent rects; view-side positioning) ───────────────

export interface PaneGeom {
  paneId: string
  pane: PaneSpec
  /** Percents of the dashboard container (0–100). */
  x: number
  y: number
  w: number
  h: number
}

export interface DividerGeom {
  splitPath: number[]
  /** Between children `index` and `index+1`. */
  index: number
  dir: SplitDir
  /** Boundary line: for a row split a VERTICAL line at (x, y..y+length);
   *  for a col split a HORIZONTAL line at (x..x+length, y). */
  x: number
  y: number
  length: number
  /** The split's span along its resize axis (percent of the container)
   *  — pointer deltas convert against it. */
  span: number
}

export interface LayoutGeometry {
  panes: PaneGeom[]
  dividers: DividerGeom[]
}

/** Flatten the tree into absolute percent rects + divider positions.
 *  Pane order (and id dedupe) matches `layoutPanes`. */
export function computeLayoutGeometry(root: LayoutNode | null): LayoutGeometry {
  const panes: PaneGeom[] = []
  const dividers: DividerGeom[] = []
  if (!root) return { panes, dividers }
  const seen = new Map<string, number>()
  const walk = (
    node: LayoutNode,
    path: number[],
    x: number,
    y: number,
    w: number,
    h: number,
  ): void => {
    if (node.type === 'pane') {
      const base = paneKey(node.pane)
      const n = seen.get(base) ?? 0
      seen.set(base, n + 1)
      panes.push({ paneId: n === 0 ? base : `${base}#${n}`, pane: node.pane, x, y, w, h })
      return
    }
    let offset = 0
    node.children.forEach((child, i) => {
      const frac = child.size / 100
      if (node.dir === 'row') {
        const cw = w * frac
        walk(child.node, [...path, i], x + offset, y, cw, h)
        if (i < node.children.length - 1) {
          dividers.push({ splitPath: path, index: i, dir: 'row', x: x + offset + cw, y, length: h, span: w })
        }
        offset += cw
      } else {
        const ch = h * frac
        walk(child.node, [...path, i], x, y + offset, w, ch)
        if (i < node.children.length - 1) {
          dividers.push({ splitPath: path, index: i, dir: 'col', x, y: y + offset + ch, length: w, span: h })
        }
        offset += ch
      }
    })
  }
  walk(root, [], 0, 0, 100, 100)
  return { panes, dividers }
}

// ── Drop zones (§6.8.2 — 5-zone hit-test + edge bands) ───────────────────

/** Container edge bands (~24px) — full-span insert zones. */
export const EDGE_BAND_PX = 24

/** A pointer inside a pane's central box (this fraction from each
 *  edge inward) reads as CENTER; nearer an edge reads as that side. */
export const CENTER_ZONE_FRAC = 0.3

export interface PxRect {
  left: number
  top: number
  right: number
  bottom: number
}

export type DropZone =
  /** Hovering a pane: split on a side half, or move/swap/replace on
   *  center. */
  | { type: 'pane'; paneId: string; region: 'center' | Side }
  /** A dashboard far-edge band: full-span insert. */
  | { type: 'edge'; side: Side }

/** Resolve where a drag at (x, y) lands. Outside the container (or in
 *  a divider gap) → null. Edge bands win over pane zones; an empty
 *  dashboard is one big first-pane target. */
export function resolveDropZone(
  x: number,
  y: number,
  container: PxRect,
  panes: Array<{ paneId: string } & PxRect>,
  edgePx: number = EDGE_BAND_PX,
): DropZone | null {
  if (x < container.left || x > container.right || y < container.top || y > container.bottom) {
    return null
  }
  if (panes.length === 0) return { type: 'edge', side: 'right' }
  // Edge bands (clamped to a quarter of the container so they never
  // swallow a small dashboard — the v1 computeDropTarget discipline).
  const bandX = Math.min(edgePx, (container.right - container.left) / 4)
  const bandY = Math.min(edgePx, (container.bottom - container.top) / 4)
  const edges: Array<{ side: Side; d: number; band: number }> = [
    { side: 'left', d: x - container.left, band: bandX },
    { side: 'right', d: container.right - x, band: bandX },
    { side: 'top', d: y - container.top, band: bandY },
    { side: 'bottom', d: container.bottom - y, band: bandY },
  ]
  const inBand = edges.filter((e) => e.d <= e.band)
  if (inBand.length > 0) {
    inBand.sort((a, b) => a.d - b.d)
    return { type: 'edge', side: inBand[0].side }
  }
  for (const p of panes) {
    if (x < p.left || x > p.right || y < p.top || y > p.bottom) continue
    const w = Math.max(1, p.right - p.left)
    const h = Math.max(1, p.bottom - p.top)
    const rx = (x - p.left) / w
    const ry = (y - p.top) / h
    const dists: Array<{ region: Side; d: number }> = [
      { region: 'left', d: rx },
      { region: 'right', d: 1 - rx },
      { region: 'top', d: ry },
      { region: 'bottom', d: 1 - ry },
    ]
    dists.sort((a, b) => a.d - b.d)
    if (dists[0].d >= CENTER_ZONE_FRAC) return { type: 'pane', paneId: p.paneId, region: 'center' }
    return { type: 'pane', paneId: p.paneId, region: dists[0].region }
  }
  return null
}

// ── Drop policy (§6.8.2 + the §6.2 duplicate policy) ─────────────────────

export type DragSource =
  /** A member from the nav drawer — resolves to its canonical
   *  terminal pane (one per agent per dashboard). */
  | { type: 'member'; workspaceId: string }
  /** A pinned HTML doc (one pane per doc — the #587 idempotence key). */
  | { type: 'htmlDoc'; workspaceId: string; filePath: string }
  /** An existing pane by its header — always a MOVE. */
  | { type: 'pane'; paneId: string }

export interface DropResult {
  root: LayoutNode | null
  /** false ⇒ nothing to save (focus-only / no-op). */
  changed: boolean
  /** The pane to flash/focus after the drop (its paneId in the NEW
   *  tree), or null. */
  focusPaneId: string | null
}

/** Apply a drop honoring the §6.2/§6.8 policy: an ALREADY-PRESENT
 *  member/doc MOVES on a side/edge drop and just FOCUSES on a center
 *  drop (never a duplicate, never a surprise removal); a fresh one
 *  splits/inserts on side/edge and REPLACES on center; an existing
 *  pane (header drag) moves on side/edge and SWAPS on center. */
export function applyDrop(
  root: LayoutNode | null,
  source: DragSource,
  zone: DropZone,
): DropResult {
  const entries = layoutPanes(root)
  let spec: PaneSpec
  let existingId: string | null
  if (source.type === 'member') {
    spec = { kind: 'terminal', workspaceId: source.workspaceId }
    existingId = findTerminalPaneId(root, source.workspaceId)
  } else if (source.type === 'htmlDoc') {
    spec = { kind: 'htmlDoc', workspaceId: source.workspaceId, filePath: source.filePath }
    const base = paneKey(spec)
    existingId = entries.some((e) => e.paneId === base) ? base : null
  } else {
    const entry = entries.find((e) => e.paneId === source.paneId)
    if (!entry) return { root, changed: false, focusPaneId: null }
    spec = entry.pane
    existingId = entry.paneId
  }
  const focusId = paneKey(spec)

  const finish = (next: LayoutNode | null): DropResult => {
    const changed =
      next !== root && serializeDashboardLayout(next) !== serializeDashboardLayout(root)
    return { root: changed ? next : root, changed, focusPaneId: focusId }
  }

  if (zone.type === 'edge') {
    if (existingId !== null) {
      return finish(movePane(root, existingId, { kind: 'edge', side: zone.side }))
    }
    return finish(insertEdge(root, zone.side, spec))
  }

  if (zone.region === 'center') {
    if (source.type === 'pane') {
      if (zone.paneId === existingId) return { root, changed: false, focusPaneId: existingId }
      return finish(swapPanes(root, existingId as string, zone.paneId))
    }
    if (existingId !== null) return { root, changed: false, focusPaneId: existingId }
    return finish(replacePaneSpec(root, zone.paneId, spec))
  }

  if (existingId !== null) {
    if (zone.paneId === existingId) return { root, changed: false, focusPaneId: existingId }
    return finish(
      movePane(root, existingId, { kind: 'pane', targetPaneId: zone.paneId, side: zone.region }),
    )
  }
  return finish(insertSplit(root, zone.paneId, zone.region, spec))
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
  /** Record the latest tree and (re)arm the trailing window. */
  schedule(root: LayoutNode | null): void
  /** Fire any pending save immediately (best-effort) and stop
   *  delivering callbacks — the unmount flush. */
  dispose(): void
}

/** Every layout change saves canonically, but a burst (drag + resize +
 *  close in quick succession) coalesces on a trailing window into ONE
 *  POST carrying the LATEST tree. Saves are single-flight: a change
 *  landing mid-POST queues exactly one follow-up. */
export function createLayoutSaver(
  save: (layoutJson: string) => Promise<{ revision: number }>,
  hooks: {
    onSaved: (revision: number) => void
    onError: (err: unknown) => void
  },
  delayMs: number = LAYOUT_SAVE_COALESCE_MS,
): LayoutSaver {
  let pending: { root: LayoutNode | null } | null = null
  let timer: ReturnType<typeof setTimeout> | null = null
  let inFlight = false
  let disposed = false

  const fire = (): void => {
    if (pending === null || inFlight) return
    const { root } = pending
    pending = null
    inFlight = true
    save(serializeDashboardLayout(root))
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
    schedule(root: LayoutNode | null): void {
      if (disposed) return
      pending = { root }
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
