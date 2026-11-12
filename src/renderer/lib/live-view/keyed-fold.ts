// The view's immutable fold over the kit's keyed watch. The reducer keeps
// every state it is given whole (a reducer may run twice, and a change keeps
// its book before and after for "was / now"), while the kit's `KeyedWatch`
// keeps one mutable state. So each step here replays frames into a fresh
// `KeyedWatch` and reads its state: the kit's own rules apply every page and
// op (nothing of them is copied into K2), and nothing the caller holds is
// changed.
//
// - `foldSnapshot`: a snapshot's head and the pages so far → the keyed state
//   and the view index's units.
// - `checkPage`: one page against the snapshot so far (its root's kind, its
//   one body, a units page's `from`), without building the rest.
// - `foldOps`: a change's ops onto a state → the state after. Only the roots
//   the ops touch are replayed (as pages of their own); the rest are shared.
//
// A frame that doesn't fit (an op on what isn't there, a page for a root not
// in the schema) throws `KeyedMisfit` with the watch's own words: the caller
// asks for a fresh snapshot, as `KeyedWatch` does. It never guesses.

import {
  KEYED_WATCH_FEATURE,
  KeyedWatch,
  type KeyedPageFrame,
  type KeyedRootState,
  type KeyedSnapshotFrame,
  type KeyedState,
  type OpsChangeFrame,
  type ServerFrame,
  type Transport,
  type ViewUnit,
  type Welcome,
} from './kit'

/** A frame didn't fit the state: the watch would ask for a fresh snapshot. */
export class KeyedMisfit extends Error {}

const WELCOME: Welcome = { root: '', version: 0, client: 0, self: '', rights: 'read', features: [KEYED_WATCH_FEATURE] }

/** `frames` into a fresh watch, in order. Throws `KeyedMisfit` when the watch asked again. */
function replay(frames: readonly ServerFrame[]): KeyedWatch {
  const listeners = new Set<(f: ServerFrame) => void>()
  const t: Transport = {
    send: () => {},
    onFrame: (cb) => {
      listeners.add(cb)
      return () => listeners.delete(cb)
    },
  }
  const w = new KeyedWatch(t, WELCOME)
  let misfit: string | null = null
  w.onResync((why, detail) => {
    misfit ??= `${why}: ${detail}`
  })
  for (const f of frames) for (const l of [...listeners]) l(f)
  w.close()
  if (misfit !== null) throw new KeyedMisfit(misfit)
  return w
}

/** The head and pages renumbered as a whole snapshot of `pages.length` pages (pages so far). */
function asWhole(head: KeyedSnapshotFrame, pages: readonly KeyedPageFrame[]): ServerFrame[] {
  const n = pages.length
  return [{ ...head, seq: 1, pages: n }, ...pages.map((p, i): KeyedPageFrame => ({ ...p, seq: i + 2, page: i + 1, pages: n }))]
}

/** A snapshot's head and its pages so far → the keyed state, and each container's units when the head has a view. */
export function foldSnapshot(head: KeyedSnapshotFrame, pages: readonly KeyedPageFrame[]): { state: KeyedState; units: Map<string, ViewUnit[]> } {
  const w = replay(asWhole(head, pages))
  const state = w.state
  if (!state) throw new KeyedMisfit('the snapshot built no state')
  const units = new Map<string, ViewUnit[]>()
  for (const c of w.view?.containers ?? []) units.set(c.key, [...w.units(c.key)])
  return { state, units }
}

/**
 * One page against the snapshot so far: replayed alone under its root (and,
 * for a units page, after that container's earlier units pages). Throws
 * `KeyedMisfit` when it doesn't fit.
 */
export function checkPage(head: KeyedSnapshotFrame, before: readonly KeyedPageFrame[], page: KeyedPageFrame): void {
  if (page.units) {
    const container = page.units.container
    const earlier = before.filter((p) => p.units?.container === container)
    const view = head.view ? { ...head.view, containers: head.view.containers.filter((c) => c.key === container) } : null
    replay(asWhole({ ...head, view }, [...earlier, page]))
    return
  }
  const root = head.schema.roots.find((r) => r.name === page.root)
  if (!root) throw new KeyedMisfit(`inconsistent: page for a root not in the schema: ${page.root}`)
  replay(asWhole({ ...head, schema: { ...head.schema, roots: [root] }, view: null }, [page]))
}

/** One root's content as the page that would carry it whole. */
function pagesOf(name: string, r: KeyedRootState, upto: number): KeyedPageFrame[] {
  const page = (body: Partial<KeyedPageFrame>): KeyedPageFrame => ({ type: 'keyed_page', seq: 0, upto, page: 0, pages: 0, root: name, ...body })
  switch (r.kind) {
    case 'list':
      return [page({ live: r.live.map(([k, v]) => [k, v]) }), page({ hidden: [...r.hidden] })]
    case 'map':
      // A map key is whole here (`<key>\x1f<field>` for a composite one), so its field is null.
      return [page({ entries: [...r.entries].map(([k, v]) => [k, null, v]) })]
    case 'text':
      return [page({ text: r.text })]
  }
}

/**
 * A change's ops onto `state` (at `rev`): the state after, as a new object.
 * The roots the ops touch are replayed into a fresh watch as a snapshot of
 * their own, then the change; the others are shared with `state`.
 */
export function foldOps(state: KeyedState, rev: number, change: OpsChangeFrame): KeyedState {
  const touched = [...new Set(change.ops.map((o) => o.root))]
  const roots = touched.map((name) => state.schema.roots.find((r) => r.name === name) ?? { name, kind: 'missing' })
  const pages = touched.flatMap((name) => {
    const r = state.roots.get(name)
    return r ? pagesOf(name, r, rev) : []
  })
  const head: KeyedSnapshotFrame = { type: 'keyed_snapshot', seq: 1, upto: rev, schema: { ...state.schema, roots }, view: null, pages: pages.length }
  const whole = asWhole(head, pages)
  const w = replay([...whole, { ...change, seq: whole.length + 1 }])
  const after = w.state
  if (!after) throw new KeyedMisfit('the change built no state')
  const next = new Map(state.roots)
  for (const name of touched) {
    const r = after.roots.get(name)
    if (r) next.set(name, r)
  }
  return { schema: state.schema, roots: next }
}
