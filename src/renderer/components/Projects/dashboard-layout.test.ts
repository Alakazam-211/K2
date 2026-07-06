// Projects V1 P5 → §6.8 — the dashboard layout blob's pure rules:
// v2 tree parse/serialize round-trips, v1 conversion, the
// untouched-vs-emptied PoC seed, tree ops (split/edge/move/swap/
// remove/resize/normalize), presets, reading order + pane identity,
// geometry + the 5-zone drop hit-test and drop policy, apply-on-open
// staleness with the own-save echo guard, and the trailing-window
// coalesced saver.

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'

import {
  CENTER_ZONE_FRAC,
  MIN_PANE_PCT,
  adoptLayout,
  adoptRevision,
  applyDrop,
  computeLayoutGeometry,
  createLayoutSaver,
  findTerminalPaneId,
  initialFreshness,
  insertEdge,
  insertSplit,
  isHtmlDocPane,
  isTerminalPane,
  layoutPanes,
  movePane,
  normalizeNode,
  observeOwnSave,
  observeRevision,
  paneKey,
  parseDashboardLayout,
  readingOrder,
  removePane,
  replacePaneSpec,
  resizeDivider,
  resolveDropZone,
  seedRoot,
  serializeDashboardLayout,
  swapPanes,
  tileIntoPreset,
  type LayoutNode,
  type PaneSpec,
  type SplitDir,
  type SplitNode,
} from './dashboard-layout'

const term = (workspaceId: string): PaneSpec => ({ kind: 'terminal', workspaceId })
const doc = (workspaceId: string, filePath: string): PaneSpec => ({
  kind: 'htmlDoc',
  workspaceId,
  filePath,
})
const pane = (p: PaneSpec): LayoutNode => ({ type: 'pane', pane: p })
const split = (dir: SplitDir, ...children: Array<[number, LayoutNode]>): LayoutNode => ({
  type: 'split',
  dir,
  children: children.map(([size, node]) => ({ size, node })),
})

/** Reading-order fingerprint: workspaceIds for known panes, kinds for
 *  unknown ones. */
const order = (root: LayoutNode | null): string[] =>
  readingOrder(root).map((p) =>
    isTerminalPane(p) || isHtmlDocPane(p) ? p.workspaceId : p.kind,
  )

/** Every §6.8 invariant a persisted tree must hold: splits have ≥2
 *  children, no same-dir nesting, sizes sum ~100 per split. */
function assertTidy(node: LayoutNode | null): void {
  if (node === null || node.type === 'pane') return
  expect(node.children.length).toBeGreaterThanOrEqual(2)
  const sum = node.children.reduce((acc, c) => acc + c.size, 0)
  expect(sum).toBeCloseTo(100, 6)
  for (const child of node.children) {
    if (child.node.type === 'split') expect(child.node.dir).not.toBe(node.dir)
    assertTidy(child.node)
  }
}

const sizesOf = (node: LayoutNode | null): number[] =>
  node && node.type === 'split' ? node.children.map((c) => c.size) : []

// ── parse / serialize (v2) ────────────────────────────────────────────────

describe('v2 parse/serialize round-trip', () => {
  it('round-trips a §6.8 split tree exactly', () => {
    const root = split(
      'row',
      [40, pane(term('ws-a'))],
      [60, split('col', [50, pane(term('ws-b'))], [50, pane(doc('ws-b', '/abs/status.html'))])],
    )
    const parsed = parseDashboardLayout(serializeDashboardLayout(root))
    expect(parsed).toEqual({ kind: 'layout', root })
  })

  it('round-trips a single-pane root (no split wrapper)', () => {
    const root = pane(term('ws-a'))
    expect(parseDashboardLayout(serializeDashboardLayout(root))).toEqual({ kind: 'layout', root })
  })

  it('`root: null` is the deliberately-emptied layout — it must NOT re-seed', () => {
    const parsed = parseDashboardLayout(serializeDashboardLayout(null))
    expect(parsed).toEqual({ kind: 'layout', root: null })
    expect(adoptLayout(serializeDashboardLayout(null), 'poc-ws')).toBeNull()
  })

  it('preserves UNKNOWN pane kinds byte-for-byte across a round-trip (§6.3 forward-compat)', () => {
    const alien = { kind: 'hologram', wavelength: 42, nested: { a: [1, 2] } }
    const root = split('row', [50, pane(term('ws-a'))], [50, pane(alien)])
    const parsed = parseDashboardLayout(serializeDashboardLayout(root))
    expect(parsed.kind).toBe('layout')
    if (parsed.kind !== 'layout') return
    expect(readingOrder(parsed.root)[1]).toEqual(alien)
    // And it re-serializes identically (a save never strips it).
    expect(JSON.parse(serializeDashboardLayout(parsed.root))).toEqual(
      JSON.parse(serializeDashboardLayout(root)),
    )
  })

  it('treats a malformed known-kind pane as unknown (inert), never dropped', () => {
    const broken = { kind: 'terminal' } // no workspaceId
    const parsed = parseDashboardLayout(
      JSON.stringify({ version: 2, root: { type: 'pane', pane: broken } }),
    )
    expect(parsed.kind).toBe('layout')
    if (parsed.kind !== 'layout') return
    const panes = readingOrder(parsed.root)
    expect(panes).toHaveLength(1)
    expect(isTerminalPane(panes[0])).toBe(false)
    expect(panes[0]).toEqual(broken)
    expect(paneKey(panes[0])).toBe('u:terminal')
  })

  it('normalizes sizes that do not sum to 100 and merges nested same-dir splits', () => {
    const parsed = parseDashboardLayout(
      JSON.stringify({
        version: 2,
        root: {
          type: 'split',
          dir: 'row',
          children: [
            { size: 1, node: { type: 'pane', pane: term('a') } },
            {
              size: 3,
              node: {
                type: 'split',
                dir: 'row',
                children: [
                  { size: 50, node: { type: 'pane', pane: term('b') } },
                  { size: 50, node: { type: 'pane', pane: term('c') } },
                ],
              },
            },
          ],
        },
      }),
    )
    expect(parsed.kind).toBe('layout')
    if (parsed.kind !== 'layout') return
    expect(order(parsed.root)).toEqual(['a', 'b', 'c'])
    expect(sizesOf(parsed.root)).toEqual([25, 37.5, 37.5])
    assertTidy(parsed.root)
  })

  it('degrades missing/garbage sizes to an equal split instead of breaking', () => {
    const parsed = parseDashboardLayout(
      JSON.stringify({
        version: 2,
        root: {
          type: 'split',
          dir: 'row',
          children: [
            { node: { type: 'pane', pane: term('a') } },
            { size: -5, node: { type: 'pane', pane: term('b') } },
          ],
        },
      }),
    )
    expect(parsed.kind).toBe('layout')
    if (parsed.kind !== 'layout') return
    expect(sizesOf(parsed.root)).toEqual([50, 50])
  })

  it('skips unusable children and collapses a single-child split', () => {
    const parsed = parseDashboardLayout(
      JSON.stringify({
        version: 2,
        root: {
          type: 'split',
          dir: 'row',
          children: [
            { size: 50, node: { type: 'pane', pane: term('a') } },
            { size: 50 }, // no node
            { size: 50, node: 'nonsense' },
          ],
        },
      }),
    )
    expect(parsed).toEqual({ kind: 'layout', root: pane(term('a')) })
  })

  it('an unreadable root reads as untouched (never break a dashboard)', () => {
    expect(parseDashboardLayout('{"version":2,"root":"garbage"}')).toEqual({ kind: 'untouched' })
    expect(parseDashboardLayout('{"version":2,"root":{"type":"split","dir":"diagonal","children":[]}}')).toEqual(
      { kind: 'untouched' },
    )
    expect(parseDashboardLayout('{"version":2,"root":{"type":"split","dir":"row","children":[]}}')).toEqual(
      { kind: 'untouched' },
    )
  })
})

// ── v1 → v2 conversion (§6.8.1) ──────────────────────────────────────────

describe('v1 blob conversion', () => {
  it('columns become ONE row-split, widths preserved', () => {
    const parsed = parseDashboardLayout(
      JSON.stringify({
        version: 1,
        columns: [
          { widthPct: 40, pane: term('a') },
          { widthPct: 60, pane: doc('b', '/s.html') },
        ],
      }),
    )
    expect(parsed).toEqual({
      kind: 'layout',
      root: split('row', [40, pane(term('a'))], [60, pane(doc('b', '/s.html'))]),
    })
  })

  it('a single v1 column converts to a bare pane node', () => {
    const parsed = parseDashboardLayout(
      JSON.stringify({ version: 1, columns: [{ widthPct: 100, pane: term('a') }] }),
    )
    expect(parsed).toEqual({ kind: 'layout', root: pane(term('a')) })
  })

  it('v1 `columns: []` was deliberately emptied → null root (not untouched)', () => {
    expect(parseDashboardLayout('{"version":1,"columns":[]}')).toEqual({
      kind: 'layout',
      root: null,
    })
    expect(adoptLayout('{"version":1,"columns":[]}', 'poc-ws')).toBeNull()
  })

  it('v1 garbage widths degrade to an equal split; unusable columns are skipped', () => {
    const parsed = parseDashboardLayout(
      JSON.stringify({
        version: 1,
        columns: [
          { pane: term('a') }, // no width
          { widthPct: -5, pane: term('b') },
          { widthPct: 50 }, // no pane — dropped
        ],
      }),
    )
    expect(parsed.kind).toBe('layout')
    if (parsed.kind !== 'layout') return
    expect(order(parsed.root)).toEqual(['a', 'b'])
    expect(sizesOf(parsed.root)).toEqual([50, 50])
  })

  it('a converted v1 layout re-saves as v2 (saves always write v2)', () => {
    const v1 = JSON.stringify({
      version: 1,
      columns: [
        { widthPct: 50, pane: term('a') },
        { widthPct: 50, pane: term('b') },
      ],
    })
    const saved = JSON.parse(serializeDashboardLayout(adoptLayout(v1, null))) as {
      version: number
      root: unknown
    }
    expect(saved.version).toBe(2)
    expect(saved.root).toBeTruthy()
  })
})

// ── seed / adopt (§6.2 init, apply-on-open) ──────────────────────────────

describe('seed with PoC (untouched vs deliberately emptied)', () => {
  it("the daemon's EMPTY_LAYOUT_V1 seed (no columns/root key) reads as untouched", () => {
    // project_groups.rs EMPTY_LAYOUT_V1 — the auto-created 'Main'.
    expect(parseDashboardLayout('{"version":1,"panes":[]}')).toEqual({ kind: 'untouched' })
  })

  it('unparseable / non-object blobs read as untouched (never break)', () => {
    expect(parseDashboardLayout('')).toEqual({ kind: 'untouched' })
    expect(parseDashboardLayout('not json')).toEqual({ kind: 'untouched' })
    expect(parseDashboardLayout('[]')).toEqual({ kind: 'untouched' })
    expect(parseDashboardLayout('null')).toEqual({ kind: 'untouched' })
  })

  it("an untouched 'Main' adopts as ONLY the PoC's canonical pane", () => {
    expect(adoptLayout('{"version":1,"panes":[]}', 'poc-ws')).toEqual(pane(term('poc-ws')))
    expect(seedRoot('poc-ws')).toEqual(pane(term('poc-ws')))
  })

  it('a memberless group (no PoC) adopts as no panes', () => {
    expect(adoptLayout('{"version":1,"panes":[]}', null)).toBeNull()
    expect(seedRoot(null)).toBeNull()
  })

  it('a saved layout adopts exactly as stored (PoC plays no part)', () => {
    const stored = serializeDashboardLayout(pane(term('other-ws')))
    expect(adoptLayout(stored, 'poc-ws')).toEqual(pane(term('other-ws')))
  })
})

// ── pane identity + reading order ─────────────────────────────────────────

describe('layoutPanes / readingOrder / findTerminalPaneId', () => {
  it('reading order is depth-first, children in order (row-major for a grid)', () => {
    const grid = split(
      'col',
      [50, split('row', [50, pane(term('a'))], [50, pane(term('b'))])],
      [50, split('row', [50, pane(term('c'))], [50, pane(term('d'))])],
    )
    expect(order(grid)).toEqual(['a', 'b', 'c', 'd'])
  })

  it('paneIds key by spec, `#n`-suffixed for duplicate specs in reading order', () => {
    const ghost1 = { kind: 'ghost', tag: 1 }
    const ghost2 = { kind: 'ghost', tag: 2 }
    const root = split(
      'row',
      [30, pane(ghost1)],
      [30, pane(ghost2)],
      [40, pane(doc('w', '/x.html'))],
    )
    expect(layoutPanes(root).map((e) => e.paneId)).toEqual([
      'u:ghost',
      'u:ghost#1',
      'h:w:/x.html',
    ])
  })

  it('findTerminalPaneId finds only terminal panes (one per agent)', () => {
    const root = split('row', [50, pane(term('a'))], [50, pane(doc('b', '/x.html'))])
    expect(findTerminalPaneId(root, 'a')).toBe('t:a')
    expect(findTerminalPaneId(root, 'b')).toBeNull() // htmlDoc never matches
    expect(findTerminalPaneId(null, 'a')).toBeNull()
  })
})

// ── normalize ─────────────────────────────────────────────────────────────

describe('normalizeNode', () => {
  it('merges nested same-dir splits and scales their sizes', () => {
    const messy = split(
      'row',
      [50, pane(term('a'))],
      [50, split('row', [50, pane(term('b'))], [50, pane(term('c'))])],
    )
    const tidy = normalizeNode(messy)
    expect(order(tidy)).toEqual(['a', 'b', 'c'])
    expect(sizesOf(tidy)).toEqual([50, 25, 25])
    assertTidy(tidy)
  })

  it('collapses single-child splits to the child', () => {
    const messy: LayoutNode = {
      type: 'split',
      dir: 'col',
      children: [{ size: 100, node: pane(term('a')) }],
    }
    expect(normalizeNode(messy)).toEqual(pane(term('a')))
  })

  it('degrades garbage sizes to an equal split', () => {
    const messy: LayoutNode = {
      type: 'split',
      dir: 'row',
      children: [
        { size: NaN, node: pane(term('a')) },
        { size: -3, node: pane(term('b')) },
      ],
    }
    expect(sizesOf(normalizeNode(messy))).toEqual([50, 50])
  })
})

// ── insertSplit (§6.8.2 drop-to-split) ───────────────────────────────────

describe('insertSplit', () => {
  it('splits the target 50/50 in the drop direction (each side/dir)', () => {
    const a = pane(term('a'))
    expect(insertSplit(a, 't:a', 'right', term('b'))).toEqual(
      split('row', [50, a], [50, pane(term('b'))]),
    )
    expect(insertSplit(a, 't:a', 'left', term('b'))).toEqual(
      split('row', [50, pane(term('b'))], [50, a]),
    )
    expect(insertSplit(a, 't:a', 'bottom', term('b'))).toEqual(
      split('col', [50, a], [50, pane(term('b'))]),
    )
    expect(insertSplit(a, 't:a', 'top', term('b'))).toEqual(
      split('col', [50, pane(term('b'))], [50, a]),
    )
  })

  it('a same-dir split merges into the parent — the target share halves', () => {
    const root = split('row', [50, pane(term('a'))], [50, pane(term('b'))])
    const next = insertSplit(root, 't:b', 'right', term('c'))
    expect(order(next)).toEqual(['a', 'b', 'c'])
    expect(sizesOf(next)).toEqual([50, 25, 25])
    assertTidy(next)
  })

  it('a cross-dir split nests', () => {
    const root = split('row', [50, pane(term('a'))], [50, pane(term('b'))])
    const next = insertSplit(root, 't:b', 'bottom', term('c'))
    expect(next).toEqual(
      split(
        'row',
        [50, pane(term('a'))],
        [50, split('col', [50, pane(term('b'))], [50, pane(term('c'))])],
      ),
    )
  })

  it('unknown target → identity no-op (callers skip the save)', () => {
    const root = split('row', [50, pane(term('a'))], [50, pane(term('b'))])
    expect(insertSplit(root, 't:nope', 'right', term('c'))).toBe(root)
  })

  it('null root → the pane becomes the whole dashboard', () => {
    expect(insertSplit(null, 't:whatever', 'right', term('a'))).toEqual(pane(term('a')))
  })
})

// ── insertEdge (§6.8.2 far-edge full-span insert) ────────────────────────

describe('insertEdge', () => {
  it('a same-dir root takes the pane as an equal-share child at that end', () => {
    const root = split('row', [50, pane(term('a'))], [50, pane(term('b'))])
    const right = insertEdge(root, 'right', term('c'))
    expect(order(right)).toEqual(['a', 'b', 'c'])
    expect(sizesOf(right).map((s) => Math.round(s * 100) / 100)).toEqual([33.33, 33.33, 33.33])
    const left = insertEdge(root, 'left', term('c'))
    expect(order(left)).toEqual(['c', 'a', 'b'])
    assertTidy(right)
  })

  it('a cross-dir edge wraps the whole dashboard 50/50', () => {
    const root = split('row', [50, pane(term('a'))], [50, pane(term('b'))])
    expect(insertEdge(root, 'bottom', term('c'))).toEqual(
      split('col', [50, root], [50, pane(term('c'))]),
    )
    expect(insertEdge(root, 'top', term('c'))).toEqual(
      split('col', [50, pane(term('c'))], [50, root]),
    )
  })

  it('a single-pane root wraps 50/50; a null root becomes the pane', () => {
    expect(insertEdge(pane(term('a')), 'right', term('b'))).toEqual(
      split('row', [50, pane(term('a'))], [50, pane(term('b'))]),
    )
    expect(insertEdge(null, 'left', term('a'))).toEqual(pane(term('a')))
  })
})

// ── removePane ────────────────────────────────────────────────────────────

describe('removePane', () => {
  it('renormalizes the survivors proportionally', () => {
    const root = split('row', [25, pane(term('a'))], [25, pane(term('b'))], [50, pane(term('c'))])
    const next = removePane(root, 't:a')
    expect(order(next)).toEqual(['b', 'c'])
    const sizes = sizesOf(next)
    expect(sizes[0]).toBeCloseTo(100 / 3, 6)
    expect(sizes[1]).toBeCloseTo(200 / 3, 6)
  })

  it('collapses a single-child split after the removal', () => {
    const root = split(
      'row',
      [50, pane(term('a'))],
      [50, split('col', [50, pane(term('b'))], [50, pane(term('c'))])],
    )
    expect(removePane(root, 't:c')).toEqual(
      split('row', [50, pane(term('a'))], [50, pane(term('b'))]),
    )
  })

  it('a chain collapse merges the surviving split into a same-dir parent', () => {
    const root = split(
      'row',
      [50, pane(term('a'))],
      [50, split('col', [50, pane(term('b'))], [50, split('row', [50, pane(term('c'))], [50, pane(term('d'))])])],
    )
    const next = removePane(root, 't:b')
    expect(order(next)).toEqual(['a', 'c', 'd'])
    expect(sizesOf(next)).toEqual([50, 25, 25])
    assertTidy(next)
  })

  it('removing the last pane empties the tree (null — persists as root:null)', () => {
    expect(removePane(pane(term('a')), 't:a')).toBeNull()
  })

  it('unknown id / null root → identity no-op', () => {
    const root = pane(term('a'))
    expect(removePane(root, 't:nope')).toBe(root)
    expect(removePane(null, 't:a')).toBeNull()
  })
})

// ── movePane / swapPanes / replacePaneSpec (§6.8.2 move semantics) ───────

describe('movePane', () => {
  it('moves a pane next to a target — never duplicates', () => {
    const root = split('row', [30, pane(term('a'))], [30, pane(term('b'))], [40, pane(term('c'))])
    const next = movePane(root, 't:a', { kind: 'pane', targetPaneId: 't:c', side: 'right' })
    expect(order(next)).toEqual(['b', 'c', 'a'])
    expect(layoutPanes(next).filter((e) => e.paneId.startsWith('t:a'))).toHaveLength(1)
    assertTidy(next)
  })

  it('moves across subtrees (structure re-normalizes)', () => {
    const root = split(
      'col',
      [50, split('row', [50, pane(term('a'))], [50, pane(term('b'))])],
      [50, pane(term('c'))],
    )
    const next = movePane(root, 't:c', { kind: 'pane', targetPaneId: 't:a', side: 'left' })
    expect(order(next)).toEqual(['c', 'a', 'b'])
    expect(sizesOf(next)).toEqual([25, 25, 50])
    assertTidy(next)
  })

  it('self-target → identity no-op', () => {
    const root = split('row', [50, pane(term('a'))], [50, pane(term('b'))])
    expect(movePane(root, 't:a', { kind: 'pane', targetPaneId: 't:a', side: 'left' })).toBe(root)
  })

  it('moves to a dashboard edge (full-span region)', () => {
    const root = split('row', [50, pane(term('a'))], [50, pane(term('b'))])
    const next = movePane(root, 't:a', { kind: 'edge', side: 'bottom' })
    expect(next).toEqual(split('col', [50, pane(term('b'))], [50, pane(term('a'))]))
  })

  it('duplicate-spec panes stay distinct through a move (sibling index shift)', () => {
    const ghost1 = { kind: 'ghost', tag: 1 }
    const ghost2 = { kind: 'ghost', tag: 2 }
    const root = split('row', [30, pane(ghost1)], [30, pane(ghost2)], [40, pane(term('a'))])
    // Move the FIRST ghost (id 'u:ghost') to the right of the terminal:
    // the target's path shifts down one when the source is removed.
    const next = movePane(root, 'u:ghost', { kind: 'pane', targetPaneId: 't:a', side: 'right' })
    const panes = readingOrder(next)
    expect(panes.map((p) => (p as { tag?: number }).tag ?? 'a')).toEqual([2, 'a', 1])
    assertTidy(next)
  })

  it('unknown source / unknown target → identity no-op', () => {
    const root = split('row', [50, pane(term('a'))], [50, pane(term('b'))])
    expect(movePane(root, 't:nope', { kind: 'edge', side: 'left' })).toBe(root)
    expect(movePane(root, 't:a', { kind: 'pane', targetPaneId: 't:nope', side: 'left' })).toBe(root)
  })
})

describe('swapPanes / replacePaneSpec', () => {
  it('swap exchanges the SPECS; slots and sizes stay put', () => {
    const root = split('row', [30, pane(term('a'))], [30, pane(term('b'))], [40, pane(doc('w', '/x.html'))])
    const next = swapPanes(root, 't:a', 'h:w:/x.html')
    expect(order(next)).toEqual(['w', 'b', 'a'])
    expect(sizesOf(next)).toEqual([30, 30, 40])
  })

  it('swap with self / unknown ids → identity no-op', () => {
    const root = split('row', [50, pane(term('a'))], [50, pane(term('b'))])
    expect(swapPanes(root, 't:a', 't:a')).toBe(root)
    expect(swapPanes(root, 't:a', 't:nope')).toBe(root)
  })

  it('replacePaneSpec swaps in place, size preserved', () => {
    const root = split('row', [70, pane(term('a'))], [30, pane(term('b'))])
    const next = replacePaneSpec(root, 't:b', doc('w', '/x.html'))
    expect(next).toEqual(split('row', [70, pane(term('a'))], [30, pane(doc('w', '/x.html'))]))
  })
})

// ── resizeDivider (§6.8.3) ───────────────────────────────────────────────

describe('resizeDivider', () => {
  it('conserves the pair and floors at MIN_PANE_PCT', () => {
    const root = split('row', [50, pane(term('a'))], [50, pane(term('b'))])
    const next = resizeDivider(root, [], 0, +100) // drag far right
    expect(sizesOf(next)).toEqual([100 - MIN_PANE_PCT, MIN_PANE_PCT])
  })

  it('leaves siblings outside the pair untouched', () => {
    const root = split('row', [20, pane(term('a'))], [30, pane(term('b'))], [50, pane(term('c'))])
    const next = resizeDivider(root, [], 1, +10)
    expect(sizesOf(next)).toEqual([20, 40, 40])
  })

  it('degrades the floor to an even split when the pair is already tight', () => {
    const root = split('row', [8, pane(term('a'))], [8, pane(term('b'))], [84, pane(term('c'))])
    const next = resizeDivider(root, [], 0, -100)
    expect(sizesOf(next)).toEqual([8, 8, 84]) // floor = pair/2
  })

  it('resizes a NESTED split by path (deltas are percent of that split)', () => {
    const root = split(
      'row',
      [50, pane(term('a'))],
      [50, split('col', [50, pane(term('b'))], [50, pane(term('c'))])],
    )
    const next = resizeDivider(root, [1], 0, +30)
    expect(next).toEqual(
      split(
        'row',
        [50, pane(term('a'))],
        [50, split('col', [80, pane(term('b'))], [20, pane(term('c'))])],
      ),
    )
  })

  it('bad path / index → identity no-op', () => {
    const root = split('row', [50, pane(term('a'))], [50, pane(term('b'))])
    expect(resizeDivider(root, [0], 0, +10)).toBe(root) // path lands on a pane
    expect(resizeDivider(root, [], 1, +10)).toBe(root) // no right sibling
    expect(resizeDivider(null, [], 0, +10)).toBeNull()
  })
})

// ── presets (§6.8.4) ──────────────────────────────────────────────────────

describe('tileIntoPreset', () => {
  const p = (n: number): PaneSpec => term(`w${n}`)
  const panes = (n: number): PaneSpec[] => Array.from({ length: n }, (_, i) => p(i + 1))

  it('cols2 with exactly 2 panes → a 50/50 row', () => {
    expect(tileIntoPreset('cols2', panes(2))).toEqual(
      split('row', [50, pane(p(1))], [50, pane(p(2))]),
    )
  })

  it('EXTRA panes stack into the LAST region (cols2 with 5)', () => {
    const next = tileIntoPreset('cols2', panes(5))
    expect(next).toEqual(
      split(
        'row',
        [50, pane(p(1))],
        [
          50,
          split('col', [25, pane(p(2))], [25, pane(p(3))], [25, pane(p(4))], [25, pane(p(5))]),
        ],
      ),
    )
    assertTidy(next)
  })

  it('cols3 tiles thirds; FEWER panes collapse the shape (no empty slots)', () => {
    const thirds = tileIntoPreset('cols3', panes(3)) as SplitNode
    expect(order(thirds)).toEqual(['w1', 'w2', 'w3'])
    expect(sizesOf(thirds).every((s) => Math.abs(s - 100 / 3) < 0.001)).toBe(true)
    expect(tileIntoPreset('cols3', panes(2))).toEqual(
      split('row', [50, pane(p(1))], [50, pane(p(2))]),
    )
    expect(tileIntoPreset('cols3', panes(1))).toEqual(pane(p(1)))
  })

  it('grid2x2 with 4 → two rows of two (reading order row-major)', () => {
    expect(tileIntoPreset('grid2x2', panes(4))).toEqual(
      split(
        'col',
        [50, split('row', [50, pane(p(1))], [50, pane(p(2))])],
        [50, split('row', [50, pane(p(3))], [50, pane(p(4))])],
      ),
    )
  })

  it('grid2x2 with 3 collapses the missing cell; with 2 collapses to a row', () => {
    expect(tileIntoPreset('grid2x2', panes(3))).toEqual(
      split('col', [50, split('row', [50, pane(p(1))], [50, pane(p(2))])], [50, pane(p(3))]),
    )
    expect(tileIntoPreset('grid2x2', panes(2))).toEqual(
      split('row', [50, pane(p(1))], [50, pane(p(2))]),
    )
  })

  it('grid2x2 with 6 stacks the extras into the last cell', () => {
    const next = tileIntoPreset('grid2x2', panes(6))
    expect(next).toEqual(
      split(
        'col',
        [50, split('row', [50, pane(p(1))], [50, pane(p(2))])],
        [
          50,
          split(
            'row',
            [50, pane(p(3))],
            [50, split('col', [100 / 3, pane(p(4))], [100 / 3, pane(p(5))], [100 / 3, pane(p(6))])],
          ),
        ],
      ),
    )
    assertTidy(next)
  })

  it('single stacks everything into one region', () => {
    expect(tileIntoPreset('single', panes(1))).toEqual(pane(p(1)))
    expect(tileIntoPreset('single', panes(3))).toEqual(
      split('col', [100 / 3, pane(p(1))], [100 / 3, pane(p(2))], [100 / 3, pane(p(3))]),
    )
  })

  it('mainStack: first pane left, the rest stacked right', () => {
    expect(tileIntoPreset('mainStack', panes(1))).toEqual(pane(p(1)))
    expect(tileIntoPreset('mainStack', panes(3))).toEqual(
      split('row', [50, pane(p(1))], [50, split('col', [50, pane(p(2))], [50, pane(p(3))])]),
    )
  })

  it('no panes → null (empty slots are never persisted)', () => {
    expect(tileIntoPreset('grid2x2', [])).toBeNull()
  })

  it('re-tiling preserves reading order for every shape', () => {
    for (const shape of ['single', 'cols2', 'cols3', 'grid2x2', 'mainStack'] as const) {
      const next = tileIntoPreset(shape, panes(5))
      expect(order(next)).toEqual(['w1', 'w2', 'w3', 'w4', 'w5'])
      assertTidy(next)
    }
  })
})

// ── geometry (tree → percent rects + dividers) ───────────────────────────

describe('computeLayoutGeometry', () => {
  it('flattens nested splits into absolute percent rects', () => {
    const root = split(
      'row',
      [30, pane(term('a'))],
      [70, split('col', [50, pane(term('b'))], [50, pane(term('c'))])],
    )
    const geo = computeLayoutGeometry(root)
    expect(geo.panes).toEqual([
      { paneId: 't:a', pane: term('a'), x: 0, y: 0, w: 30, h: 100 },
      { paneId: 't:b', pane: term('b'), x: 30, y: 0, w: 70, h: 50 },
      { paneId: 't:c', pane: term('c'), x: 30, y: 50, w: 70, h: 50 },
    ])
    expect(geo.dividers).toEqual([
      { splitPath: [], index: 0, dir: 'row', x: 30, y: 0, length: 100, span: 100 },
      { splitPath: [1], index: 0, dir: 'col', x: 30, y: 50, length: 70, span: 100 },
    ])
  })

  it('empty tree → no rects, no dividers', () => {
    expect(computeLayoutGeometry(null)).toEqual({ panes: [], dividers: [] })
  })
})

// ── drop zones (§6.8.2 — 5-zone hit-test + edge bands) ───────────────────

describe('resolveDropZone', () => {
  const container = { left: 0, top: 0, right: 400, bottom: 300 }
  const panes = [
    { paneId: 'A', left: 0, top: 0, right: 200, bottom: 300 },
    { paneId: 'B', left: 200, top: 0, right: 400, bottom: 300 },
  ]

  it('container edge bands win (full-span insert), nearest edge in a corner', () => {
    expect(resolveDropZone(2, 150, container, panes)).toEqual({ type: 'edge', side: 'left' })
    expect(resolveDropZone(398, 150, container, panes)).toEqual({ type: 'edge', side: 'right' })
    expect(resolveDropZone(100, 10, container, panes)).toEqual({ type: 'edge', side: 'top' })
    expect(resolveDropZone(100, 295, container, panes)).toEqual({ type: 'edge', side: 'bottom' })
    expect(resolveDropZone(5, 3, container, panes)).toEqual({ type: 'edge', side: 'top' })
  })

  it('pane halves resolve by nearest edge; the middle box is center', () => {
    expect(resolveDropZone(100, 150, container, panes)).toEqual({
      type: 'pane',
      paneId: 'A',
      region: 'center',
    })
    expect(resolveDropZone(30, 150, container, panes)).toEqual({
      type: 'pane',
      paneId: 'A',
      region: 'left',
    })
    expect(resolveDropZone(170, 150, container, panes)).toEqual({
      type: 'pane',
      paneId: 'A',
      region: 'right',
    })
    expect(resolveDropZone(100, 60, container, panes)).toEqual({
      type: 'pane',
      paneId: 'A',
      region: 'top',
    })
    expect(resolveDropZone(100, 250, container, panes)).toEqual({
      type: 'pane',
      paneId: 'A',
      region: 'bottom',
    })
    expect(resolveDropZone(250, 150, container, panes)).toEqual({
      type: 'pane',
      paneId: 'B',
      region: 'left',
    })
  })

  it('outside the container / in a divider gap → null', () => {
    expect(resolveDropZone(500, 100, container, panes)).toBeNull()
    const gapped = [
      { paneId: 'A', left: 0, top: 0, right: 190, bottom: 300 },
      { paneId: 'B', left: 210, top: 0, right: 400, bottom: 300 },
    ]
    expect(resolveDropZone(200, 150, container, gapped)).toBeNull()
  })

  it('an empty dashboard is one big first-pane target', () => {
    expect(resolveDropZone(100, 100, container, [])).toEqual({ type: 'edge', side: 'right' })
  })

  it('edge bands clamp to a quarter of a small container', () => {
    const small = { left: 0, top: 0, right: 60, bottom: 60 }
    const one = [{ paneId: 'A', left: 0, top: 0, right: 60, bottom: 60 }]
    // 20 > 60/4=15 → NOT an edge; lands in the pane (center box check:
    // rx=1/3 ≥ CENTER_ZONE_FRAC → center).
    expect(CENTER_ZONE_FRAC).toBeLessThanOrEqual(1 / 3)
    expect(resolveDropZone(20, 30, small, one)).toEqual({
      type: 'pane',
      paneId: 'A',
      region: 'center',
    })
  })
})

// ── drop policy (§6.8.2 + the §6.2 duplicate policy) ─────────────────────

describe('applyDrop', () => {
  const base = (): LayoutNode =>
    split('row', [50, pane(term('a'))], [50, pane(doc('a', '/x.html'))])

  it('a fresh member SPLITS on a side zone', () => {
    const root = base()
    const r = applyDrop(root, { type: 'member', workspaceId: 'b' }, {
      type: 'pane',
      paneId: 'h:a:/x.html',
      region: 'bottom',
    })
    expect(r.changed).toBe(true)
    expect(order(r.root)).toEqual(['a', 'a', 'b'])
    expect(findTerminalPaneId(r.root, 'b')).toBe('t:b')
    expect(r.focusPaneId).toBe('t:b')
  })

  it('a fresh member REPLACES on center, keeping the slot', () => {
    const r = applyDrop(base(), { type: 'member', workspaceId: 'b' }, {
      type: 'pane',
      paneId: 'h:a:/x.html',
      region: 'center',
    })
    expect(r.changed).toBe(true)
    expect(r.root).toEqual(split('row', [50, pane(term('a'))], [50, pane(term('b'))]))
  })

  it('a fresh member INSERTS at a container edge', () => {
    const r = applyDrop(base(), { type: 'member', workspaceId: 'b' }, { type: 'edge', side: 'left' })
    expect(r.changed).toBe(true)
    expect(order(r.root)).toEqual(['b', 'a', 'a'])
  })

  it('an already-present member MOVES on a side/edge drop (never duplicates)', () => {
    const root = base()
    const r = applyDrop(root, { type: 'member', workspaceId: 'a' }, {
      type: 'pane',
      paneId: 'h:a:/x.html',
      region: 'right',
    })
    expect(r.changed).toBe(true)
    expect(layoutPanes(r.root).filter((e) => e.paneId.startsWith('t:a'))).toHaveLength(1)
    expect(layoutPanes(r.root).map((e) => e.paneId)).toEqual(['h:a:/x.html', 't:a'])
  })

  it('an already-present member on CENTER just focuses (no change, no save)', () => {
    const root = base()
    const r = applyDrop(root, { type: 'member', workspaceId: 'a' }, {
      type: 'pane',
      paneId: 'h:a:/x.html',
      region: 'center',
    })
    expect(r.changed).toBe(false)
    expect(r.root).toBe(root)
    expect(r.focusPaneId).toBe('t:a')
  })

  it('a member dropped on its own pane side → no-op focus', () => {
    const root = base()
    const r = applyDrop(root, { type: 'member', workspaceId: 'a' }, {
      type: 'pane',
      paneId: 't:a',
      region: 'left',
    })
    expect(r.changed).toBe(false)
    expect(r.root).toBe(root)
  })

  it('a duplicate htmlDoc focuses instead of duplicating (the #587 key)', () => {
    const root = base()
    const r = applyDrop(
      root,
      { type: 'htmlDoc', workspaceId: 'a', filePath: '/x.html' },
      { type: 'pane', paneId: 't:a', region: 'center' },
    )
    expect(r.changed).toBe(false)
    expect(r.focusPaneId).toBe('h:a:/x.html')
  })

  it('a pane-header drag MOVES on a side and SWAPS on center', () => {
    const root = base()
    const moved = applyDrop(root, { type: 'pane', paneId: 't:a' }, {
      type: 'pane',
      paneId: 'h:a:/x.html',
      region: 'right',
    })
    expect(layoutPanes(moved.root).map((e) => e.paneId)).toEqual(['h:a:/x.html', 't:a'])
    const swapped = applyDrop(root, { type: 'pane', paneId: 't:a' }, {
      type: 'pane',
      paneId: 'h:a:/x.html',
      region: 'center',
    })
    expect(swapped.changed).toBe(true)
    expect(layoutPanes(swapped.root).map((e) => e.paneId)).toEqual(['h:a:/x.html', 't:a'])
    expect(sizesOf(swapped.root)).toEqual([50, 50])
  })

  it('a pane dropped on its own center / an unknown pane source → no-op', () => {
    const root = base()
    const self = applyDrop(root, { type: 'pane', paneId: 't:a' }, {
      type: 'pane',
      paneId: 't:a',
      region: 'center',
    })
    expect(self.changed).toBe(false)
    expect(self.root).toBe(root)
    const ghost = applyDrop(root, { type: 'pane', paneId: 't:nope' }, { type: 'edge', side: 'left' })
    expect(ghost.changed).toBe(false)
    expect(ghost.focusPaneId).toBeNull()
  })

  it('a drop that reproduces the same layout reports unchanged (identity guard)', () => {
    // The sole pane moved to an edge is still the sole pane.
    const root = pane(term('a'))
    const r = applyDrop(root, { type: 'member', workspaceId: 'a' }, { type: 'edge', side: 'right' })
    expect(r.changed).toBe(false)
    expect(r.root).toBe(root)
  })

  it('a fresh member on an EMPTY dashboard becomes the first pane', () => {
    const r = applyDrop(null, { type: 'member', workspaceId: 'a' }, { type: 'edge', side: 'right' })
    expect(r.changed).toBe(true)
    expect(r.root).toEqual(pane(term('a')))
  })
})

// ── apply-on-open staleness + echo guard (§6.3a) ─────────────────────────

describe('layout freshness', () => {
  it('the adopted revision and anything at/below it is fresh', () => {
    let s = initialFreshness(3)
    s = observeRevision(s, 3)
    s = observeRevision(s, 2)
    expect(s.staleRevision).toBeNull()
  })

  it('a foreign revision beyond known marks stale — and NEVER rearranges (state only)', () => {
    let s = initialFreshness(3)
    s = observeRevision(s, 4)
    expect(s.staleRevision).toBe(4)
    // a later, even-newer foreign write keeps the max
    s = observeRevision(s, 6)
    expect(s.staleRevision).toBe(6)
  })

  it('echo guard: our own save ratchets known so its event echo is not stale', () => {
    let s = initialFreshness(3)
    s = observeOwnSave(s, 4) // save response lands first
    s = observeRevision(s, 4) // then the layout-changed echo / refetch
    expect(s.staleRevision).toBeNull()
  })

  it('echo guard survives the race where the echo beats the save response', () => {
    let s = initialFreshness(3)
    s = observeRevision(s, 4) // echo arrives first → transiently stale
    expect(s.staleRevision).toBe(4)
    s = observeOwnSave(s, 4) // our response identifies it as ours
    expect(s.staleRevision).toBeNull()
  })

  it('our save also supersedes an older foreign stale flag (last-write-wins)', () => {
    let s = initialFreshness(3)
    s = observeRevision(s, 4) // foreign write
    s = observeOwnSave(s, 5) // we overwrote it — canonical is ours now
    expect(s.staleRevision).toBeNull()
    expect(s.known).toBe(5)
  })

  it('a foreign write NEWER than our save stays stale', () => {
    let s = initialFreshness(3)
    s = observeOwnSave(s, 4)
    s = observeRevision(s, 5)
    expect(s.staleRevision).toBe(5)
  })

  it('adoptRevision (apply-on-open / the stale pill) clears and ratchets', () => {
    let s = initialFreshness(3)
    s = observeRevision(s, 5)
    s = adoptRevision(s, 5)
    expect(s).toEqual({ known: 5, staleRevision: null })
  })
})

// ── coalesced saver ───────────────────────────────────────────────────────

describe('createLayoutSaver', () => {
  beforeEach(() => {
    vi.useFakeTimers()
  })
  afterEach(() => {
    vi.useRealTimers()
  })

  const rootA: LayoutNode = pane(term('a'))
  const rootB: LayoutNode = split('row', [50, pane(term('a'))], [50, pane(term('b'))])

  it('coalesces a burst into ONE save carrying the LATEST tree', async () => {
    const save = vi.fn().mockResolvedValue({ revision: 7 })
    const onSaved = vi.fn()
    const saver = createLayoutSaver(save, { onSaved, onError: vi.fn() }, 300)

    saver.schedule(rootA)
    vi.advanceTimersByTime(200) // inside the window — re-arms
    saver.schedule(rootB)
    vi.advanceTimersByTime(299)
    expect(save).not.toHaveBeenCalled() // trailing window still open
    vi.advanceTimersByTime(1)
    expect(save).toHaveBeenCalledTimes(1)
    expect(save).toHaveBeenCalledWith(serializeDashboardLayout(rootB))
    await vi.runAllTimersAsync()
    expect(onSaved).toHaveBeenCalledWith(7)
  })

  it('a change landing mid-POST queues exactly one follow-up save', async () => {
    let resolveFirst!: (v: { revision: number }) => void
    const save = vi
      .fn()
      .mockImplementationOnce(() => new Promise<{ revision: number }>((r) => (resolveFirst = r)))
      .mockResolvedValue({ revision: 9 })
    const saver = createLayoutSaver(save, { onSaved: vi.fn(), onError: vi.fn() }, 300)

    saver.schedule(rootA)
    vi.advanceTimersByTime(300)
    expect(save).toHaveBeenCalledTimes(1)

    saver.schedule(rootB) // mid-flight
    vi.advanceTimersByTime(300)
    expect(save).toHaveBeenCalledTimes(1) // still waiting on the first

    resolveFirst({ revision: 8 })
    await vi.runAllTimersAsync()
    expect(save).toHaveBeenCalledTimes(2)
    expect(save).toHaveBeenLastCalledWith(serializeDashboardLayout(rootB))
  })

  it('errors surface via onError and never throw', async () => {
    const onError = vi.fn()
    const saver = createLayoutSaver(vi.fn().mockRejectedValue(new Error('403')), {
      onSaved: vi.fn(),
      onError,
    })
    saver.schedule(rootA)
    await vi.runAllTimersAsync()
    expect(onError).toHaveBeenCalledTimes(1)
  })

  it('dispose flushes a pending save immediately and silences callbacks', async () => {
    const save = vi.fn().mockResolvedValue({ revision: 5 })
    const onSaved = vi.fn()
    const saver = createLayoutSaver(save, { onSaved, onError: vi.fn() }, 300)

    saver.schedule(rootA)
    saver.dispose() // unmount before the window elapsed
    expect(save).toHaveBeenCalledTimes(1) // flushed, not dropped
    await vi.runAllTimersAsync()
    expect(onSaved).not.toHaveBeenCalled() // component is gone

    saver.schedule(rootB) // post-dispose scheduling is inert
    vi.advanceTimersByTime(1000)
    expect(save).toHaveBeenCalledTimes(1)
  })

  it('an emptied dashboard saves root:null (deliberate empty, not untouched)', () => {
    const save = vi.fn().mockResolvedValue({ revision: 2 })
    const saver = createLayoutSaver(save, { onSaved: vi.fn(), onError: vi.fn() }, 300)
    saver.schedule(null)
    vi.advanceTimersByTime(300)
    expect(save).toHaveBeenCalledWith(serializeDashboardLayout(null))
  })
})

// ── misc guards ───────────────────────────────────────────────────────────

describe('pane type guards + keys', () => {
  it('discriminate by kind + required fields', () => {
    expect(isTerminalPane(term('a'))).toBe(true)
    expect(isTerminalPane(doc('a', '/x'))).toBe(false)
    expect(isHtmlDocPane(doc('a', '/x'))).toBe(true)
    expect(isHtmlDocPane({ kind: 'htmlDoc', workspaceId: 'a' })).toBe(false)
    expect(isHtmlDocPane({ kind: 'weird' })).toBe(false)
  })

  it('paneKey is spec identity: kind + workspaceId (+ filePath)', () => {
    expect(paneKey(term('a'))).toBe('t:a')
    expect(paneKey(doc('a', '/x.html'))).toBe('h:a:/x.html')
    expect(paneKey({ kind: 'hologram', extra: 1 })).toBe('u:hologram')
  })
})
