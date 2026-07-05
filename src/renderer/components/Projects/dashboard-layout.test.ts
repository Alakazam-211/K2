// Projects V1 P5 — the dashboard layout blob's pure rules (§6.2/§6.3):
// parse/serialize round-trips, the untouched-vs-emptied PoC seed,
// percent-width math, drop targets + the one-pane-per-agent policy,
// apply-on-open staleness with the own-save echo guard, and the
// trailing-window coalesced saver.

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'

import {
  MIN_COLUMN_PCT,
  adoptLayout,
  adoptRevision,
  computeDropTarget,
  computeInsertBoundary,
  createLayoutSaver,
  dropMember,
  findTerminalColumn,
  initialFreshness,
  insertColumnAt,
  isHtmlDocPane,
  isTerminalPane,
  moveColumn,
  normalizeWidths,
  observeOwnSave,
  observeRevision,
  parseDashboardLayout,
  removeColumnAt,
  replaceColumnPane,
  resizePair,
  seedColumns,
  serializeDashboardLayout,
  type DashboardColumn,
} from './dashboard-layout'

const term = (workspaceId: string): DashboardColumn['pane'] => ({ kind: 'terminal', workspaceId })
const doc = (workspaceId: string, filePath: string): DashboardColumn['pane'] => ({
  kind: 'htmlDoc',
  workspaceId,
  filePath,
})

// ── parse / serialize ─────────────────────────────────────────────────────

describe('parse/serialize round-trip', () => {
  it('round-trips the §6.3 v1 shape exactly', () => {
    const columns: DashboardColumn[] = [
      { widthPct: 40, pane: term('ws-a') },
      { widthPct: 60, pane: doc('ws-b', '/abs/status.html') },
    ]
    const parsed = parseDashboardLayout(serializeDashboardLayout(columns))
    expect(parsed).toEqual({ kind: 'layout', columns })
  })

  it('preserves UNKNOWN pane kinds byte-for-byte across a round-trip (§6.3 forward-compat)', () => {
    const alien = { kind: 'hologram', wavelength: 42, nested: { a: [1, 2] } }
    const columns: DashboardColumn[] = [
      { widthPct: 50, pane: term('ws-a') },
      { widthPct: 50, pane: alien },
    ]
    const parsed = parseDashboardLayout(serializeDashboardLayout(columns))
    expect(parsed.kind).toBe('layout')
    if (parsed.kind !== 'layout') return
    expect(parsed.columns[1].pane).toEqual(alien)
    // And it re-serializes identically (a save never strips it).
    expect(JSON.parse(serializeDashboardLayout(parsed.columns))).toEqual(
      JSON.parse(serializeDashboardLayout(columns)),
    )
  })

  it('normalizes widths that do not sum to 100', () => {
    const parsed = parseDashboardLayout(
      JSON.stringify({
        version: 1,
        columns: [
          { widthPct: 1, pane: { kind: 'terminal', workspaceId: 'a' } },
          { widthPct: 3, pane: { kind: 'terminal', workspaceId: 'b' } },
        ],
      }),
    )
    expect(parsed.kind).toBe('layout')
    if (parsed.kind !== 'layout') return
    expect(parsed.columns.map((c) => c.widthPct)).toEqual([25, 75])
  })

  it('degrades missing/garbage widths to an equal split instead of breaking', () => {
    const parsed = parseDashboardLayout(
      JSON.stringify({
        version: 1,
        columns: [
          { pane: { kind: 'terminal', workspaceId: 'a' } },
          { widthPct: -5, pane: { kind: 'terminal', workspaceId: 'b' } },
        ],
      }),
    )
    expect(parsed.kind).toBe('layout')
    if (parsed.kind !== 'layout') return
    expect(parsed.columns.map((c) => c.widthPct)).toEqual([50, 50])
  })

  it('drops column entries with no usable pane but keeps the rest', () => {
    const parsed = parseDashboardLayout(
      JSON.stringify({
        version: 1,
        columns: [
          { widthPct: 50, pane: { kind: 'terminal', workspaceId: 'a' } },
          { widthPct: 50 }, // no pane
          { widthPct: 50, pane: 'nonsense' },
        ],
      }),
    )
    expect(parsed).toEqual({
      kind: 'layout',
      columns: [{ widthPct: 100, pane: { kind: 'terminal', workspaceId: 'a' } }],
    })
  })

  it('treats a malformed known-kind pane as unknown (inert), never dropped', () => {
    const broken = { kind: 'terminal' } // no workspaceId
    const parsed = parseDashboardLayout(
      JSON.stringify({ version: 1, columns: [{ widthPct: 100, pane: broken }] }),
    )
    expect(parsed.kind).toBe('layout')
    if (parsed.kind !== 'layout') return
    expect(isTerminalPane(parsed.columns[0].pane)).toBe(false)
    expect(parsed.columns[0].pane).toEqual(broken)
  })
})

// ── seed / adopt (§6.2 init, apply-on-open) ──────────────────────────────

describe('seed with PoC (untouched vs deliberately emptied)', () => {
  it("the daemon's EMPTY_LAYOUT_V1 seed (no columns key) reads as untouched", () => {
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
    expect(adoptLayout('{"version":1,"panes":[]}', 'poc-ws')).toEqual([
      { widthPct: 100, pane: { kind: 'terminal', workspaceId: 'poc-ws' } },
    ])
  })

  it('a memberless group (no PoC) adopts as no panes', () => {
    expect(adoptLayout('{"version":1,"panes":[]}', null)).toEqual([])
    expect(seedColumns(null)).toEqual([])
  })

  it('a layout SAVED as columns:[] was deliberately emptied — it must NOT re-seed', () => {
    expect(adoptLayout('{"version":1,"columns":[]}', 'poc-ws')).toEqual([])
  })

  it('a saved layout adopts exactly as stored (PoC plays no part)', () => {
    const stored = serializeDashboardLayout([{ widthPct: 100, pane: term('other-ws') }])
    expect(adoptLayout(stored, 'poc-ws')).toEqual([{ widthPct: 100, pane: term('other-ws') }])
  })
})

// ── width math ────────────────────────────────────────────────────────────

describe('column width math', () => {
  it('insert takes an equal share and rebalances the rest proportionally', () => {
    const cols = [
      { widthPct: 75, pane: term('a') },
      { widthPct: 25, pane: term('b') },
    ]
    const next = insertColumnAt(cols, 1, term('c'))
    expect(next[0].widthPct).toBeCloseTo(50, 6)
    expect(next[1].widthPct).toBeCloseTo(100 / 3, 6)
    expect(next[2].widthPct).toBeCloseTo(100 / 6, 6)
    expect(next.reduce((acc, c) => acc + c.widthPct, 0)).toBeCloseTo(100, 6)
    expect(next[1].pane).toEqual(term('c'))
  })

  it('insert into empty takes 100%', () => {
    expect(insertColumnAt([], 0, term('a'))).toEqual([{ widthPct: 100, pane: term('a') }])
  })

  it('remove renormalizes the survivors proportionally', () => {
    const cols = [
      { widthPct: 50, pane: term('a') },
      { widthPct: 25, pane: term('b') },
      { widthPct: 25, pane: term('c') },
    ]
    expect(removeColumnAt(cols, 0).map((c) => c.widthPct)).toEqual([50, 50])
  })

  it('resizePair conserves the pair and floors at MIN_COLUMN_PCT', () => {
    const cols = [
      { widthPct: 50, pane: term('a') },
      { widthPct: 50, pane: term('b') },
    ]
    const next = resizePair(cols, 0, +100) // drag far right
    expect(next[0].widthPct).toBe(100 - MIN_COLUMN_PCT)
    expect(next[1].widthPct).toBe(MIN_COLUMN_PCT)
    expect(next[0].widthPct + next[1].widthPct).toBe(100)
  })

  it('resizePair degrades the floor to an even split when the pair is already tight', () => {
    // 7+ columns → pair share < 2×15: the floor must not wedge.
    const cols = [
      { widthPct: 10, pane: term('a') },
      { widthPct: 10, pane: term('b') },
      { widthPct: 80, pane: term('c') },
    ]
    const next = resizePair(cols, 0, -100)
    expect(next[0].widthPct).toBe(10) // floor = pair/2
    expect(next[1].widthPct).toBe(10)
  })

  it('normalizeWidths leaves a well-formed set alone (same proportions)', () => {
    const cols = [
      { widthPct: 60, pane: term('a') },
      { widthPct: 40, pane: term('b') },
    ]
    expect(normalizeWidths(cols)).toBe(cols)
  })
})

// ── reorder / drop targets / duplicate policy (§6.2) ─────────────────────

describe('moveColumn (boundary-index rule)', () => {
  const cols = [
    { widthPct: 30, pane: term('a') },
    { widthPct: 30, pane: term('b') },
    { widthPct: 40, pane: term('c') },
  ]

  it('moves to a boundary and keeps widths attached to their panes', () => {
    const next = moveColumn(cols, 0, 3)
    expect(next.map((c) => (c.pane as { workspaceId: string }).workspaceId)).toEqual([
      'b',
      'c',
      'a',
    ])
    expect(next.map((c) => c.widthPct)).toEqual([30, 40, 30])
  })

  it('both boundaries of the source slot are identity no-ops (skip the save)', () => {
    expect(moveColumn(cols, 1, 1)).toBe(cols)
    expect(moveColumn(cols, 1, 2)).toBe(cols)
  })
})

describe('computeDropTarget / computeInsertBoundary', () => {
  const rects = [
    { left: 0, right: 300 },
    { left: 300, right: 600 },
  ]

  it('edges insert, inner body replaces', () => {
    expect(computeDropTarget(10, rects)).toEqual({ type: 'insert', index: 0 })
    expect(computeDropTarget(150, rects)).toEqual({ type: 'replace', index: 0 })
    expect(computeDropTarget(299, rects)).toEqual({ type: 'insert', index: 1 })
    expect(computeDropTarget(301, rects)).toEqual({ type: 'insert', index: 1 })
    expect(computeDropTarget(450, rects)).toEqual({ type: 'replace', index: 1 })
    expect(computeDropTarget(599, rects)).toEqual({ type: 'insert', index: 2 })
  })

  it('an empty grid is one big insert-at-0', () => {
    expect(computeDropTarget(123, [])).toEqual({ type: 'insert', index: 0 })
  })

  it('a narrow column keeps a replace zone (edge shrinks to a quarter)', () => {
    const narrow = [{ left: 0, right: 40 }]
    expect(computeDropTarget(20, narrow)).toEqual({ type: 'replace', index: 0 })
    expect(computeDropTarget(5, narrow)).toEqual({ type: 'insert', index: 0 })
  })

  it('computeInsertBoundary follows the Sidebar midpoint rule', () => {
    expect(computeInsertBoundary(100, rects)).toBe(0)
    expect(computeInsertBoundary(200, rects)).toBe(1)
    expect(computeInsertBoundary(500, rects)).toBe(2)
  })
})

describe('dropMember (one terminal pane per agent)', () => {
  const cols = [
    { widthPct: 50, pane: term('a') },
    { widthPct: 50, pane: doc('a', '/x.html') },
  ]

  it('a fresh member inserts a new column', () => {
    const r = dropMember(cols, 'b', { type: 'insert', index: 2 })
    expect(r.changed).toBe(true)
    expect(r.columns).toHaveLength(3)
    expect(findTerminalColumn(r.columns, 'b')).toBe(2)
    expect(r.focusIndex).toBe(2)
  })

  it('a fresh member replaces on a replace target, keeping the column width', () => {
    const r = dropMember(cols, 'b', { type: 'replace', index: 1 })
    expect(r.changed).toBe(true)
    expect(r.columns[1]).toEqual({ widthPct: 50, pane: term('b') })
  })

  it('an already-present member MOVES on insert (never duplicates)', () => {
    const r = dropMember(cols, 'a', { type: 'insert', index: 2 })
    expect(r.changed).toBe(true)
    expect(r.columns.filter((c) => isTerminalPane(c.pane))).toHaveLength(1)
    expect(findTerminalColumn(r.columns, 'a')).toBe(1)
    expect(r.focusIndex).toBe(1)
  })

  it('an already-present member on a replace target just focuses (no change)', () => {
    const r = dropMember(cols, 'a', { type: 'replace', index: 1 })
    expect(r.changed).toBe(false)
    expect(r.columns).toBe(cols)
    expect(r.focusIndex).toBe(0)
  })

  it('a no-op move (own boundary) reports unchanged', () => {
    const r = dropMember(cols, 'a', { type: 'insert', index: 0 })
    expect(r.changed).toBe(false)
    expect(r.columns).toBe(cols)
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

  const colsA: DashboardColumn[] = [{ widthPct: 100, pane: term('a') }]
  const colsB: DashboardColumn[] = [
    { widthPct: 50, pane: term('a') },
    { widthPct: 50, pane: term('b') },
  ]

  it('coalesces a burst into ONE save carrying the LATEST columns', async () => {
    const save = vi.fn().mockResolvedValue({ revision: 7 })
    const onSaved = vi.fn()
    const saver = createLayoutSaver(save, { onSaved, onError: vi.fn() }, 300)

    saver.schedule(colsA)
    vi.advanceTimersByTime(200) // inside the window — re-arms
    saver.schedule(colsB)
    vi.advanceTimersByTime(299)
    expect(save).not.toHaveBeenCalled() // trailing window still open
    vi.advanceTimersByTime(1)
    expect(save).toHaveBeenCalledTimes(1)
    expect(save).toHaveBeenCalledWith(serializeDashboardLayout(colsB))
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

    saver.schedule(colsA)
    vi.advanceTimersByTime(300)
    expect(save).toHaveBeenCalledTimes(1)

    saver.schedule(colsB) // mid-flight
    vi.advanceTimersByTime(300)
    expect(save).toHaveBeenCalledTimes(1) // still waiting on the first

    resolveFirst({ revision: 8 })
    await vi.runAllTimersAsync()
    expect(save).toHaveBeenCalledTimes(2)
    expect(save).toHaveBeenLastCalledWith(serializeDashboardLayout(colsB))
  })

  it('errors surface via onError and never throw', async () => {
    const onError = vi.fn()
    const saver = createLayoutSaver(vi.fn().mockRejectedValue(new Error('403')), {
      onSaved: vi.fn(),
      onError,
    })
    saver.schedule(colsA)
    await vi.runAllTimersAsync()
    expect(onError).toHaveBeenCalledTimes(1)
  })

  it('dispose flushes a pending save immediately and silences callbacks', async () => {
    const save = vi.fn().mockResolvedValue({ revision: 5 })
    const onSaved = vi.fn()
    const saver = createLayoutSaver(save, { onSaved, onError: vi.fn() }, 300)

    saver.schedule(colsA)
    saver.dispose() // unmount before the window elapsed
    expect(save).toHaveBeenCalledTimes(1) // flushed, not dropped
    await vi.runAllTimersAsync()
    expect(onSaved).not.toHaveBeenCalled() // component is gone

    saver.schedule(colsB) // post-dispose scheduling is inert
    vi.advanceTimersByTime(1000)
    expect(save).toHaveBeenCalledTimes(1)
  })
})

// ── misc guards ───────────────────────────────────────────────────────────

describe('pane type guards', () => {
  it('discriminate by kind + required fields', () => {
    expect(isTerminalPane(term('a'))).toBe(true)
    expect(isTerminalPane(doc('a', '/x'))).toBe(false)
    expect(isHtmlDocPane(doc('a', '/x'))).toBe(true)
    expect(isHtmlDocPane({ kind: 'htmlDoc', workspaceId: 'a' })).toBe(false)
    expect(isHtmlDocPane({ kind: 'weird' })).toBe(false)
  })

  it('replaceColumnPane swaps in place, width preserved', () => {
    const cols = [{ widthPct: 100, pane: term('a') }]
    expect(replaceColumnPane(cols, 0, doc('a', '/x'))).toEqual([
      { widthPct: 100, pane: doc('a', '/x') },
    ])
  })
})
