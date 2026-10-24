// The person's order and pins in a Garden's Agents widget (0.46.4; Baden
// via Appa 2026-10-22; prd-garden-column-collapse-and-agent-list-v1 §3, Q3).
//
// In any Agents widget the person can drag agents into their own order (or
// move a row with Alt+Up / Alt+Down) and pin or unpin an agent (the row's
// menu: right-click, Shift+F10 or the context-menu key). It is the person's
// view state, per Garden, on this computer. Agents may read it but never set
// it, so the canonical copy is the LOCAL daemon's (`agents-order.json`,
// `GET /cli/zen/agents/order`; the set route takes the owner token only),
// never this window's localStorage:
//   - the widget reads it on mount and on every `zen_agents_order_changed`
//     from this computer's daemon (every window stays in step; Zen already
//     holds the local app socket open), and again when the window gets focus;
//   - a change is drawn at once (optimistic) and sent; writes go out one at a
//     time, in order; a refused write re-reads the daemon's copy.
//
// What it means when drawing (pure helpers below):
//   - `order` (full row addresses as `agents.list()` gives them) sits on top
//     of the widget's own order (the Home's, the Agents view's, or a
//     `list-from` list's): listed rows first, in that order, then the rest in
//     the widget's order. The person's order beats the Board's (Q3).
//   - `pins: null` = each agent's workspace pin (`row.pinned`), as before.
//     Once the person pins or unpins in a Garden, that Garden's own list
//     alone decides its pin grid (`[]` = no pins there).
//   - Neither ever adds a row: an address the widget doesn't draw is ignored.
//
// An older daemon (no route: 404 `unknown zen route`) leaves the widget as it
// was: no drag, no pin menu.

import { create } from 'zustand'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { scopeForHost } from '@/kessel/server-scope'
import { onZenAgentsOrderChanged } from '@/stores/session-events'

/** Addresses one list may hold (the daemon refuses more). */
export const ZEN_AGENTS_ORDER_MAX = 200

/** One Garden's view: `null` = never set by the person. */
export interface ZenAgentsView {
  order: string[] | null
  pins: string[] | null
}

export const ZEN_AGENTS_VIEW_UNSET: ZenAgentsView = Object.freeze({ order: null, pins: null }) as ZenAgentsView

export class ZenAgentsOrderParseError extends Error {}

function addressList(v: unknown, what: string): string[] | null {
  if (v === null || v === undefined) return null
  if (!Array.isArray(v) || v.some((a) => typeof a !== 'string')) {
    throw new ZenAgentsOrderParseError(`zen/agents/order: ${what} is not a list of addresses`)
  }
  return v as string[]
}

/** `GET /cli/zen/agents/order` → Garden id → view. Bad shapes throw. */
export function parseZenAgentsOrder(raw: unknown): Record<string, ZenAgentsView> {
  if (!raw || typeof raw !== 'object' || !Array.isArray((raw as { gardens?: unknown }).gardens)) {
    throw new ZenAgentsOrderParseError('zen/agents/order: no gardens list')
  }
  const out: Record<string, ZenAgentsView> = {}
  for (const g of (raw as { gardens: unknown[] }).gardens) {
    if (!g || typeof g !== 'object' || typeof (g as { id?: unknown }).id !== 'string') {
      throw new ZenAgentsOrderParseError('zen/agents/order: a Garden row without an id')
    }
    const row = g as { id: string; order?: unknown; pins?: unknown }
    out[row.id] = { order: addressList(row.order, `${row.id}.order`), pins: addressList(row.pins, `${row.id}.pins`) }
  }
  return out
}

/** The `garden` row of a set's answer. */
function parseSetAnswer(raw: unknown): { id: string; view: ZenAgentsView } {
  const g = raw && typeof raw === 'object' ? (raw as { garden?: unknown }).garden : null
  const parsed = parseZenAgentsOrder({ gardens: [g] })
  const id = Object.keys(parsed)[0]
  return { id, view: parsed[id] }
}

// ── Pure helpers ──────────────────────────────────────────────────────

/** A row address as the lists compare it: trimmed, lower case. */
export function zenAgentsKey(address: string): string {
  return address.trim().toLowerCase()
}

/** Rows in the person's order: the listed rows first, in that order, then
 *  every other row in its own order. Unknown addresses are ignored, so it
 *  never adds a row. `null` (never dragged): the rows as they are. */
export function orderZenAgentRows<T extends { address: string }>(rows: readonly T[], order: readonly string[] | null): T[] {
  if (!order || order.length === 0) return [...rows]
  const rank = new Map<string, number>()
  order.forEach((a, i) => {
    const k = zenAgentsKey(a)
    if (!rank.has(k)) rank.set(k, i)
  })
  const listed: Array<{ row: T; at: number }> = []
  const rest: T[] = []
  for (const row of rows) {
    const at = rank.get(zenAgentsKey(row.address))
    if (at === undefined) rest.push(row)
    else listed.push({ row, at })
  }
  listed.sort((a, b) => a.at - b.at)
  return [...listed.map((l) => l.row), ...rest]
}

/** Is the row pinned in this Garden: the Garden's own pins once the person
 *  set them, else the agent's workspace pin. */
export function zenRowPinned(row: { address: string; pinned?: boolean }, pins: readonly string[] | null): boolean {
  if (pins === null) return row.pinned === true
  const k = zenAgentsKey(row.address)
  return pins.some((p) => zenAgentsKey(p) === k)
}

/** Where a moved row goes: just before `before`, or just after `after`. */
export type ZenAgentsMoveTarget = { before: string } | { after: string }

/** `sequence` (every drawn address, in drawn order) with `moved` taken out
 *  and put before / after the target. Null when nothing moves (an unknown
 *  address, or the row is already there). */
export function moveZenAddress(sequence: readonly string[], moved: string, target: ZenAgentsMoveTarget): string[] | null {
  const mk = zenAgentsKey(moved)
  const from = sequence.findIndex((a) => zenAgentsKey(a) === mk)
  if (from < 0) return null
  const anchor = 'before' in target ? target.before : target.after
  const ak = zenAgentsKey(anchor)
  if (ak === mk) return null
  const without = sequence.filter((_, i) => i !== from)
  const at = without.findIndex((a) => zenAgentsKey(a) === ak)
  if (at < 0) return null
  const to = 'before' in target ? at : at + 1
  const next = [...without.slice(0, to), sequence[from], ...without.slice(to)]
  return next.every((a, i) => a === sequence[i]) ? null : next
}

/**
 * The order to store after a move: `drawnAfter` (every drawn address in its
 * new order), with the addresses of the old `stored` order that aren't drawn
 * now (an agent whose server is offline, a Home row not shown) kept right
 * after the drawn address they followed, so they come back in place. At most
 * {@link ZEN_AGENTS_ORDER_MAX}: hidden addresses go first, then the tail.
 */
export function mergeZenAgentsOrder(stored: readonly string[] | null, drawnAfter: readonly string[]): string[] {
  const drawn = new Set(drawnAfter.map(zenAgentsKey))
  const out = [...drawnAfter]
  if (stored) {
    // Hidden addresses, grouped under the drawn address before them (or the start).
    const after = new Map<string, string[]>()
    const head: string[] = []
    let prev: string | null = null
    const seen = new Set<string>()
    for (const a of stored) {
      const k = zenAgentsKey(a)
      if (seen.has(k)) continue
      seen.add(k)
      if (drawn.has(k)) {
        prev = k
        continue
      }
      if (prev === null) head.push(a)
      else after.set(prev, [...(after.get(prev) ?? []), a])
    }
    const merged: string[] = [...head]
    for (const a of out) {
      merged.push(a)
      merged.push(...(after.get(zenAgentsKey(a)) ?? []))
    }
    if (merged.length <= ZEN_AGENTS_ORDER_MAX) return merged
  }
  return out.slice(0, ZEN_AGENTS_ORDER_MAX)
}

/** The Garden's pins after the person pins or unpins `address`. Before the
 *  Garden had its own pins, they start as the workspace pins of the rows in
 *  the widget (`rows`), so nothing else changes on screen. */
export function toggleZenPin(
  pins: readonly string[] | null,
  rows: readonly { address: string; pinned?: boolean }[],
  address: string,
): string[] {
  const base = pins !== null ? [...pins] : rows.filter((r) => r.pinned === true).map((r) => r.address)
  const k = zenAgentsKey(address)
  const at = base.findIndex((p) => zenAgentsKey(p) === k)
  if (at >= 0) return base.filter((_, i) => i !== at)
  return [...base, address].slice(-ZEN_AGENTS_ORDER_MAX)
}

/** The gap (0..cells) a pointer at (x, y) is over in a grid of cells, in
 *  reading order: after every cell that ends above the pointer's row, or sits
 *  on its row with its middle to the left of the pointer. */
export function zenGridGapAt(rects: readonly { top: number; bottom: number; left: number; width: number }[], x: number, y: number): number {
  let gap = 0
  for (let i = 0; i < rects.length; i++) {
    const r = rects[i]
    if (y > r.bottom || (y >= r.top && x > r.left + r.width / 2)) gap = i + 1
  }
  return gap
}

// ── The store (this computer's daemon is the truth) ──────────────────

export type ZenAgentsOrderStatus = 'idle' | 'loading' | 'ready' | 'unsupported' | 'failed'

export interface ZenAgentsOrderState {
  status: ZenAgentsOrderStatus
  byGarden: Record<string, ZenAgentsView>
  error: string | null
}

export const useZenAgentsOrderStore = create<ZenAgentsOrderState>(() => ({ status: 'idle', byGarden: {}, error: null }))

function localScope() {
  return scopeForHost('local')
}

function routeMissing(err: unknown): boolean {
  const msg = err instanceof Error ? err.message : String(err)
  return /unknown zen route/i.test(msg)
}

let loadSeq = 0
let writesInFlight = 0
let reloadAfterWrites = false
let writeChain: Promise<void> = Promise.resolve()

/** Read every Garden's view from this computer's daemon. A newer call wins;
 *  an answer that lands while a change is on its way is dropped and read
 *  again after it. */
export async function loadZenAgentsOrder(): Promise<void> {
  const seq = ++loadSeq
  if (useZenAgentsOrderStore.getState().status === 'idle') useZenAgentsOrderStore.setState({ status: 'loading' })
  let next: Partial<ZenAgentsOrderState>
  try {
    const byGarden = parseZenAgentsOrder(await daemonCliGet<unknown>(localScope(), 'zen/agents/order'))
    next = { status: 'ready', byGarden, error: null }
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err)
    if (routeMissing(err)) {
      next = { status: 'unsupported', error: null }
    } else {
      console.warn('[zen] reading the Agents order failed:', err)
      // A view we already have stays drawn; editing waits for a good read.
      next = { status: 'failed', error: message }
    }
  }
  if (seq !== loadSeq) return
  if (writesInFlight > 0) {
    reloadAfterWrites = true
    return
  }
  useZenAgentsOrderStore.setState(next)
}

/** Change one Garden's view: drawn at once, then sent (one write at a time,
 *  in order). `order` / `pins`: a list, or `null` to go back to the default;
 *  a key left out is kept. A refused write re-reads the daemon's copy. */
export function setZenAgentsView(gardenId: string, change: { order?: string[] | null; pins?: string[] | null }): Promise<void> {
  if (!gardenId) throw new Error('zen: setZenAgentsView needs a Garden id')
  if (!('order' in change) && !('pins' in change)) throw new Error('zen: setZenAgentsView needs order or pins')
  const s = useZenAgentsOrderStore.getState()
  const prev = s.byGarden[gardenId] ?? ZEN_AGENTS_VIEW_UNSET
  const view: ZenAgentsView = {
    order: 'order' in change ? (change.order ?? null) : prev.order,
    pins: 'pins' in change ? (change.pins ?? null) : prev.pins,
  }
  useZenAgentsOrderStore.setState({ byGarden: { ...s.byGarden, [gardenId]: view } })
  const body: Record<string, unknown> = { garden: gardenId }
  if ('order' in change) body.order = change.order ?? null
  if ('pins' in change) body.pins = change.pins ?? null
  writesInFlight += 1
  const run = async (): Promise<void> => {
    let failed = false
    try {
      const { id, view: saved } = parseSetAnswer(await daemonCliPost<unknown>(localScope(), 'zen/agents/order/set', body))
      // The daemon's copy, unless a newer change of this Garden is queued.
      if (writesInFlight === 1) {
        const cur = useZenAgentsOrderStore.getState()
        useZenAgentsOrderStore.setState({ byGarden: { ...cur.byGarden, [id]: saved } })
      }
    } catch (err) {
      failed = true
      console.warn('[zen] saving the Agents order failed; reading it back:', err)
    } finally {
      writesInFlight -= 1
    }
    if (writesInFlight === 0 && (failed || reloadAfterWrites)) {
      reloadAfterWrites = false
      await loadZenAgentsOrder()
    }
  }
  writeChain = writeChain.then(run, run)
  return writeChain
}

let liveUsers = 0
let liveOff: (() => void) | null = null

/** Keep the store live while any Agents widget is mounted: read now, then
 *  on every `zen_agents_order_changed` from this computer's daemon and
 *  whenever this window gets focus. It only registers on the local app bus:
 *  Zen already holds that socket open while a Garden shows (`ZenRoot`'s
 *  `watchLocalZenChanged`). Returns the release. */
export function retainZenAgentsOrderLive(): () => void {
  liveUsers += 1
  if (liveUsers === 1) {
    void loadZenAgentsOrder()
    const offEvent = onZenAgentsOrderChanged(localScope(), () => void loadZenAgentsOrder())
    const onFocus = (): void => void loadZenAgentsOrder()
    window.addEventListener('focus', onFocus)
    liveOff = () => {
      offEvent()
      window.removeEventListener('focus', onFocus)
    }
  }
  let released = false
  return () => {
    if (released) return
    released = true
    liveUsers -= 1
    if (liveUsers === 0 && liveOff) {
      liveOff()
      liveOff = null
    }
  }
}

/** Tests only. */
export async function __settleZenAgentsOrderForTests(): Promise<void> {
  await writeChain
}

/** Tests only. */
export function __resetZenAgentsOrderForTests(): void {
  loadSeq = 0
  writesInFlight = 0
  reloadAfterWrites = false
  writeChain = Promise.resolve()
  liveOff?.()
  liveOff = null
  liveUsers = 0
  useZenAgentsOrderStore.setState({ status: 'idle', byGarden: {}, error: null })
}
