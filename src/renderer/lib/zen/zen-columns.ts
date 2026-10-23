// prd-garden-column-collapse-and-agent-list-v1 A1 (CA10, CA15) — where a
// Garden's collapsible columns are: folded to an edge tab, or open.
//
// The Garden file says which columns may fold and where they start
// (`layout.folds`, from `[[layout.column]] collapsible / collapsed / label`).
// Where a column is NOW is view state, not the Garden's structure (the
// canonical-vs-per-client rule): it lives on this computer, per Garden, in
// localStorage `k2.zen.columns.v1` =
// `{version: 1, folds: {"<gardenId>/<ncols>/<index>": true | false}}`,
// shared by every window through the `storage` event, so a second window on
// the same Garden follows. The column count is in the key: a Garden whose
// layout changes to another count ignores the old keys.
//
// What decides a column, strongest first:
//   1. a widget's own fold (`k2.layout.set` / `toggle`) on this page: held
//      in memory only, never saved (Q4: no unprompted folds);
//   2. the person's click on K2's chevron or tab: saved here;
//   3. the file's `collapsed`.
// Then the narrow-window fold (a ResizeObserver measures the row): while the
// open columns' min widths don't fit, the rightmost open collapsible column
// folds, never the last open one. It is never saved. A column the person
// opens while the window is narrow stays open until the window is wide again.
//
// The last open column never folds: the person's chevron is disabled there
// and a widget's call is refused (`last_open`), so a page can never have
// every column folded at runtime.
//
// Storage can throw: reads and writes go through try/catch and fall back to
// memory. A stored value that isn't ours is not overwritten until a click.

import { createContext, useCallback, useEffect, useMemo, useRef, useState, type RefObject } from 'react'
import { create, type StoreApi, type UseBoundStore } from 'zustand'
import type { ZenColumnFold, ZenLayout } from './zen-page'
import { ZEN_COLUMN_TAB_PX } from './zen-overflow'

export const ZEN_COLUMNS_KEY = 'k2.zen.columns.v1'

export interface ZenColumnsDoc {
  version: 1
  folds: Record<string, boolean>
}

export interface KeyValueStorage {
  getItem(key: string): string | null
  setItem(key: string, value: string): void
}

/** The saved key of one column of one Garden with `ncols` columns. */
export function zenColumnKey(gardenId: string, ncols: number, index: number): string {
  return `${gardenId}/${ncols}/${index}`
}

/** What the tab and K2 call a column: its label, else "Column N". */
export function zenColumnLabel(folds: readonly ZenColumnFold[] | undefined, index: number): string {
  return folds?.[index]?.label ?? `Column ${index + 1}`
}

export function parseZenColumnsDoc(raw: string | null): ZenColumnsDoc | null {
  if (raw === null) return null
  let v: unknown
  try {
    v = JSON.parse(raw)
  } catch {
    return null
  }
  if (!v || typeof v !== 'object' || Array.isArray(v)) return null
  const d = v as Record<string, unknown>
  if (d.version !== 1 || !d.folds || typeof d.folds !== 'object' || Array.isArray(d.folds)) return null
  const folds: Record<string, boolean> = {}
  for (const [k, val] of Object.entries(d.folds as Record<string, unknown>)) {
    if (typeof val === 'boolean' && k.length > 0) folds[k] = val
  }
  return { version: 1, folds }
}

export interface ZenColumnsState {
  /** The person's saved folds, by `zenColumnKey`. */
  folds: Record<string, boolean>
  /** The person's click: save it. */
  save(key: string, collapsed: boolean): void
  /** Another window saved `raw` (the `storage` event). */
  applyExternal(raw: string | null): void
}

function safeGet(kv: KeyValueStorage | null): string | null {
  if (!kv) return null
  try {
    return kv.getItem(ZEN_COLUMNS_KEY)
  } catch (err) {
    console.warn('[zen] column fold storage read failed:', err)
    return null
  }
}

function safeSet(kv: KeyValueStorage | null, value: string): void {
  if (!kv) return
  try {
    kv.setItem(ZEN_COLUMNS_KEY, value)
  } catch (err) {
    console.warn('[zen] column fold storage write failed:', err)
  }
}

export function createZenColumnsStore(local: KeyValueStorage | null): UseBoundStore<StoreApi<ZenColumnsState>> {
  const initial = parseZenColumnsDoc(safeGet(local))
  return create<ZenColumnsState>((set, get) => ({
    folds: initial?.folds ?? {},
    save(key, collapsed) {
      if (!key || get().folds[key] === collapsed) return
      const folds = { ...get().folds, [key]: collapsed }
      set({ folds })
      safeSet(local, JSON.stringify({ version: 1, folds } satisfies ZenColumnsDoc))
    },
    applyExternal(raw) {
      set({ folds: parseZenColumnsDoc(raw)?.folds ?? {} })
    },
  }))
}

/** Keep `store` in step with other windows. Returns the unsubscribe. */
export function attachZenColumnsStorageSync(
  store: UseBoundStore<StoreApi<ZenColumnsState>>,
  target: Pick<EventTarget, 'addEventListener' | 'removeEventListener'>,
  local: KeyValueStorage | null,
): () => void {
  const onStorage = (e: Event): void => {
    const se = e as Event & { key?: string | null; newValue?: string | null }
    if (se.key === ZEN_COLUMNS_KEY) store.getState().applyExternal(se.newValue ?? null)
    else if (se.key === null) store.getState().applyExternal(safeGet(local))
  }
  target.addEventListener('storage', onStorage)
  return () => target.removeEventListener('storage', onStorage)
}

function localOrNull(): KeyValueStorage | null {
  try {
    return globalThis.localStorage ?? null
  } catch {
    return null
  }
}

const localKv = localOrNull()

export const useZenColumnsStore = createZenColumnsStore(localKv)

if (typeof window !== 'undefined' && typeof window.addEventListener === 'function') {
  attachZenColumnsStorageSync(useZenColumnsStore, window, localKv)
}

// ── Where each column is (pure) ─────────────────────────────────────────

export interface ZenColumnFoldInput {
  gardenId: string
  /** The page's `layout.folds` (undefined: nothing folds). */
  folds: readonly ZenColumnFold[] | undefined
  minWidths: readonly number[]
  /** The person's saved folds (`useZenColumnsStore`). */
  saved: Readonly<Record<string, boolean>>
  /** Widget folds on this page (never saved), by column. */
  transient: ReadonlyMap<number, boolean>
  /** Columns the person opened while the window was narrow. */
  keepOpen: ReadonlySet<number>
  /** The row's inner width in px (null: not measured yet, no narrow fold). */
  width: number | null
  /** px between columns. */
  gap: number
}

export interface ZenColumnFoldState {
  /** Folded now, per column. */
  collapsed: boolean[]
  /** Folded by the narrow window only (not saved), per column. */
  auto: boolean[]
}

/** Folded before the narrow-window fold: widget > person > file. */
function chosen(input: ZenColumnFoldInput, ncols: number): boolean[] {
  const folds = input.folds
  return Array.from({ length: ncols }, (_, i) => {
    const f = folds?.[i]
    if (!f?.collapsible) return false
    const t = input.transient.get(i)
    if (t !== undefined) return t
    const s = input.saved[zenColumnKey(input.gardenId, ncols, i)]
    return s ?? f.collapsed
  })
}

/** Does a row of `width` px hold the open columns' min widths plus tabs? */
function fits(minWidths: readonly number[], collapsed: readonly boolean[], width: number, gap: number): boolean {
  let need = Math.max(0, minWidths.length - 1) * gap
  for (let i = 0; i < minWidths.length; i++) need += collapsed[i] ? ZEN_COLUMN_TAB_PX : Math.max(0, minWidths[i] ?? 0)
  return need <= width
}

/** The narrow-window fold: rightmost open collapsible column first, never
 *  the last open one, never one in `keepOpen`. */
export function zenAutoFolds(
  minWidths: readonly number[],
  folds: readonly ZenColumnFold[] | undefined,
  base: readonly boolean[],
  keepOpen: ReadonlySet<number>,
  width: number | null,
  gap: number,
): boolean[] {
  const auto = base.map(() => false)
  if (width === null || !folds || width <= 0) return auto
  const now = [...base]
  for (let i = now.length - 1; i >= 0; i--) {
    if (fits(minWidths, now, width, gap)) break
    if (now[i] || !folds[i]?.collapsible || keepOpen.has(i)) continue
    if (now.filter((c) => !c).length <= 1) break
    now[i] = true
    auto[i] = true
  }
  return auto
}

/** Where every column is now. Never every column folded: if the saved and
 *  file states would fold them all, the first one opens. */
export function zenColumnFolds(input: ZenColumnFoldInput): ZenColumnFoldState {
  const ncols = input.minWidths.length
  const base = chosen(input, ncols)
  if (ncols > 0 && base.every(Boolean)) base[0] = false
  const auto = zenAutoFolds(input.minWidths, input.folds, base, input.keepOpen, input.width, input.gap)
  return { collapsed: base.map((c, i) => c || auto[i]), auto }
}

/** Why a fold can't happen (K2's chevron is disabled; a widget's call is
 *  refused): `bad_index` (no such column), `fixed` (not collapsible),
 *  `last_open` (the only open column). Null when it can. */
export type ZenColumnFoldRefusal = 'bad_index' | 'fixed' | 'last_open'

export function zenColumnFoldRefusal(
  folds: readonly ZenColumnFold[] | undefined,
  collapsed: readonly boolean[],
  index: number,
  fold: boolean,
): ZenColumnFoldRefusal | null {
  if (!Number.isInteger(index) || index < 0 || index >= collapsed.length) return 'bad_index'
  if (!folds?.[index]?.collapsible) return 'fixed'
  if (fold && !collapsed[index] && collapsed.filter((c) => !c).length <= 1) return 'last_open'
  return null
}

/** One column as `k2.layout.get()` answers it. */
export interface ZenLayoutColumn {
  index: number
  label: string
  collapsible: boolean
  collapsed: boolean
}

export function zenLayoutColumns(folds: readonly ZenColumnFold[] | undefined, collapsed: readonly boolean[]): ZenLayoutColumn[] {
  return collapsed.map((c, i) => ({
    index: i,
    label: zenColumnLabel(folds, i),
    collapsible: folds?.[i]?.collapsible === true,
    collapsed: c,
  }))
}

/** What a page hands its custom widgets for `k2.layout.*` (CA10): only that
 *  page's columns, and only a widget's own (unsaved) folds. */
export interface ZenColumnsController {
  columns(): ZenLayoutColumn[]
  /** A widget's fold, never saved. Null when done (or nothing changed). */
  setByWidget(index: number, collapsed: boolean): ZenColumnFoldRefusal | null
  subscribe(fn: (columns: ZenLayoutColumn[]) => void): () => void
}

/** The page's controller, for the custom frames drawn on it (`deps.layout`).
 *  Null outside a Garden page: the verbs answer `verb_unavailable`. */
export const ZenColumnsContext = createContext<ZenColumnsController | null>(null)

// ── The page's folds (React) ────────────────────────────────────────────

/** Who moved a column (the page moves the focus and announces it). */
export type ZenColumnMover = 'person' | 'widget'

export interface ZenColumnFoldsApi extends ZenColumnFoldState {
  /** Can column `i` fold now (collapsible, open, not the last open one)? */
  canFold(i: number): boolean
  /** The person's chevron: fold and save. */
  personFold(i: number): void
  /** The person's tab: open and save (and keep it open while narrow). */
  personExpand(i: number): void
  /** The page's `k2.layout.*` controller (stable for the page's life). */
  controller: ZenColumnsController
  /** Called after every fold or open K2 or a widget made (not the narrow
   *  window's): the page moves the focus and announces it. */
  onMove(fn: ((index: number, collapsed: boolean, by: ZenColumnMover) => void) | null): void
}

const NO_TRANSIENT: ReadonlyMap<number, boolean> = new Map()
const NO_KEEP: ReadonlySet<number> = new Set()

function sameFolds(a: readonly boolean[], b: readonly boolean[]): boolean {
  return a.length === b.length && a.every((v, i) => v === b[i])
}

/** Measure the layout row (A1, CA15): its inner width and column gap, on a
 *  ResizeObserver (FC27 is pure CSS and measures nothing). Only while the
 *  page has a collapsible column. */
function useRowWidth(host: RefObject<HTMLElement | null>, on: boolean): { width: number | null; gap: number } {
  const [m, setM] = useState<{ width: number | null; gap: number }>({ width: null, gap: 0 })
  useEffect(() => {
    const el = host.current
    if (!on || !el || typeof ResizeObserver === 'undefined') return undefined
    const read = (width: number): void => {
      const gap = Number.parseFloat(getComputedStyle(el).columnGap) || 0
      const w = Math.round(width)
      setM((prev) => (prev.width === w && prev.gap === gap ? prev : { width: w, gap }))
    }
    const ro = new ResizeObserver((entries) => {
      const e = entries[entries.length - 1]
      if (e) read(e.contentRect.width)
    })
    ro.observe(el)
    return () => ro.disconnect()
  }, [host, on])
  return on ? m : { width: null, gap: 0 }
}

/**
 * Where a page's columns are, and the ways to move them. `gardenId` and the
 * column count scope the saved folds; widget folds and the narrow window's
 * "keep open" reset when either changes (a Garden switch).
 */
export function useZenColumnFolds(
  gardenId: string,
  layout: ZenLayout,
  host: RefObject<HTMLElement | null>,
): ZenColumnFoldsApi {
  const ncols = layout.split.length
  const folds = layout.folds
  const scope = `${gardenId}/${ncols}`
  const saved = useZenColumnsStore((s) => s.folds)
  const [transient, setTransient] = useState<{ scope: string; map: ReadonlyMap<number, boolean> }>({ scope, map: NO_TRANSIENT })
  const [keepOpen, setKeepOpen] = useState<{ scope: string; set: ReadonlySet<number> }>({ scope, set: NO_KEEP })
  const t = transient.scope === scope ? transient.map : NO_TRANSIENT
  const k = keepOpen.scope === scope ? keepOpen.set : NO_KEEP
  const { width, gap } = useRowWidth(host, folds !== undefined)
  const state = useMemo(
    () => zenColumnFolds({ gardenId, folds, minWidths: layout.minWidths, saved, transient: t, keepOpen: k, width, gap }),
    [gardenId, folds, layout.minWidths, saved, t, k, width, gap],
  )
  // Wide again: the person's "keep open" has done its job.
  useEffect(() => {
    if (k.size === 0) return
    const base = state.collapsed.map((c, i) => c && !state.auto[i])
    if (!zenAutoFolds(layout.minWidths, folds, base, NO_KEEP, width, gap).some(Boolean)) setKeepOpen({ scope, set: NO_KEEP })
  }, [k, state, layout.minWidths, folds, width, gap, scope])

  const live = useRef({ state, folds, scope, gardenId, ncols })
  live.current = { state, folds, scope, gardenId, ncols }
  const mover = useRef<((index: number, collapsed: boolean, by: ZenColumnMover) => void) | null>(null)

  const keep = useCallback((i: number, on: boolean) => {
    const sc = live.current.scope
    setKeepOpen((prev) => {
      const cur = prev.scope === sc ? prev.set : NO_KEEP
      if (cur.has(i) === on) return prev
      const next = new Set(cur)
      if (on) next.add(i)
      else next.delete(i)
      return { scope: sc, set: next }
    })
  }, [])
  const setWidget = useCallback((i: number, c: boolean | null) => {
    const sc = live.current.scope
    setTransient((prev) => {
      const cur = prev.scope === sc ? prev.map : NO_TRANSIENT
      if (c === null ? !cur.has(i) : cur.get(i) === c) return prev
      const next = new Map(cur)
      if (c === null) next.delete(i)
      else next.set(i, c)
      return { scope: sc, map: next }
    })
  }, [])

  const canFold = useCallback(
    (i: number) => zenColumnFoldRefusal(folds, state.collapsed, i, true) === null && !state.collapsed[i],
    [folds, state.collapsed],
  )
  const personFold = useCallback(
    (i: number) => {
      const cur = live.current
      if (cur.state.collapsed[i] || zenColumnFoldRefusal(cur.folds, cur.state.collapsed, i, true) !== null) return
      useZenColumnsStore.getState().save(zenColumnKey(cur.gardenId, cur.ncols, i), true)
      setWidget(i, null)
      keep(i, false)
      mover.current?.(i, true, 'person')
    },
    [keep, setWidget],
  )
  const personExpand = useCallback(
    (i: number) => {
      const cur = live.current
      if (!cur.state.collapsed[i] || zenColumnFoldRefusal(cur.folds, cur.state.collapsed, i, false) !== null) return
      useZenColumnsStore.getState().save(zenColumnKey(cur.gardenId, cur.ncols, i), false)
      setWidget(i, null)
      if (cur.state.auto[i]) keep(i, true)
      mover.current?.(i, false, 'person')
    },
    [keep, setWidget],
  )

  const subscribers = useRef(new Set<(columns: ZenLayoutColumn[]) => void>())
  const controller = useMemo<ZenColumnsController>(
    () => ({
      columns: () => zenLayoutColumns(live.current.folds, live.current.state.collapsed),
      setByWidget: (i, c) => {
        const cur = live.current
        const refusal = zenColumnFoldRefusal(cur.folds, cur.state.collapsed, i, c)
        if (refusal) return refusal
        if (cur.state.collapsed[i] === c) return null
        setWidget(i, c)
        // A widget opening a column the narrow window folded keeps it open.
        if (!c && cur.state.auto[i]) keep(i, true)
        mover.current?.(i, c, 'widget')
        return null
      },
      subscribe: (fn) => {
        subscribers.current.add(fn)
        return () => {
          subscribers.current.delete(fn)
        }
      },
    }),
    [keep, setWidget],
  )
  // Tell `layout.subscribe` widgets whenever a column moves (any cause).
  const lastSent = useRef<boolean[] | null>(null)
  useEffect(() => {
    if (lastSent.current && sameFolds(lastSent.current, state.collapsed)) return
    const first = lastSent.current === null
    lastSent.current = state.collapsed
    if (first) return
    const cols = zenLayoutColumns(folds, state.collapsed)
    for (const fn of [...subscribers.current]) fn(cols)
  }, [state.collapsed, folds])

  const onMove = useCallback((fn: ((index: number, collapsed: boolean, by: ZenColumnMover) => void) | null) => {
    mover.current = fn
  }, [])

  return { ...state, canFold, personFold, personExpand, controller, onMove }
}
