// prd-zen-gardens-v1 G9, G13, G14, G22, G25 — the Garden list, from THIS
// computer's daemon, and which Garden this window shows.
//
// Gardens are personal canvases that live on this computer
// (`~/.k2/zen/gardens.json` + `gardens/<id>.toml`, owned by the LOCAL
// daemon). Every window shares the list; each window remembers its own pick
// (`zen-window.ts`). Requests go to `scopeForHost('local')` only:
//   GET  /cli/zen/gardens                    the list (`setUp:false` = no folder yet)
//   POST /cli/zen/setup {}                   make the folder + Default (once, on Zen-on)
//   POST /cli/zen/garden/new {name}          + New Garden (blank template)
//   POST /cli/zen/garden/rename {garden, name}
//   POST /cli/zen/garden/delete {garden}
// and the list is re-read on every local `zen_changed`.
//
// The window's Garden is its stored pick when the list has it, else the
// first Garden (written back). A Garden deleted elsewhere moves the window
// to the first Garden. Every body is parsed here, at the boundary.

import { create } from 'zustand'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { zenLocalScope, zenLoadFailure, type ZenLoadFailure } from './zen-api'
import { ZenPageParseError } from './zen-page'
import { useZenWindowStore } from './zen-window'

export interface ZenGarden {
  id: string
  name: string
  /** 1-based position. */
  index: number
  template: string
  hasFile: boolean
  /** The Home the Agents widget starts on (G27), when the Garden has one. */
  seedHome: string | null
  createdAt: string | null
}

export interface ZenGardensList {
  setUp: boolean
  gardens: ZenGarden[]
}

function isObj(v: unknown): v is Record<string, unknown> {
  return v !== null && typeof v === 'object' && !Array.isArray(v)
}

function parseGarden(raw: unknown, at: number): ZenGarden | null {
  if (!isObj(raw) || typeof raw.id !== 'string' || !raw.id) return null
  const index = typeof raw.index === 'number' && Number.isFinite(raw.index) ? raw.index : at + 1
  return {
    id: raw.id,
    name: typeof raw.name === 'string' && raw.name ? raw.name : raw.id,
    index,
    template: typeof raw.template === 'string' ? raw.template : '',
    hasFile: raw.hasFile === true,
    seedHome: typeof raw.seedHome === 'string' && raw.seedHome ? raw.seedHome : null,
    createdAt: typeof raw.createdAt === 'string' ? raw.createdAt : null,
  }
}

/** Parse `GET /cli/zen/gardens` (and the list in `POST setup`). Throws
 *  `ZenPageParseError` on a body that isn't one. */
export function parseZenGardens(raw: unknown): ZenGardensList {
  if (!isObj(raw)) throw new ZenPageParseError('the Garden list is not an object')
  if (!Array.isArray(raw.gardens)) {
    throw new ZenPageParseError(typeof raw.error === 'string' ? raw.error : 'no gardens in the answer')
  }
  const gardens: ZenGarden[] = []
  const seen = new Set<string>()
  raw.gardens.forEach((g, i) => {
    const parsed = parseGarden(g, i)
    if (!parsed || seen.has(parsed.id)) return
    seen.add(parsed.id)
    gardens.push(parsed)
  })
  // `setUp` missing: a list with Gardens is set up.
  const setUp = typeof raw.setUp === 'boolean' ? raw.setUp : gardens.length > 0
  return { setUp, gardens }
}

/** Parse `POST /cli/zen/garden/new`: `{ok, garden}`. */
export function parseZenGardenNew(raw: unknown): ZenGarden {
  const g = isObj(raw) ? parseGarden(raw.garden, 0) : null
  if (!g) throw new ZenPageParseError('the new Garden is missing from the answer')
  return g
}

export type ZenGardensState = {
  status: 'idle' | 'loading' | 'ready' | 'failed'
  setUp: boolean
  gardens: ZenGarden[]
  failure: ZenLoadFailure | null
}

export const useZenGardensStore = create<ZenGardensState>(() => ({
  status: 'idle',
  setUp: false,
  gardens: [],
  failure: null,
}))

/** The window's Garden: its pick when the list has it, else the first. */
export function windowGardenOf(gardens: readonly ZenGarden[], picked: string | null): ZenGarden | null {
  if (gardens.length === 0) return null
  return gardens.find((g) => g.id === picked) ?? gardens[0]
}

/** The Garden this window shows (null before the list is read). */
export function currentZenGarden(): ZenGarden | null {
  return windowGardenOf(useZenGardensStore.getState().gardens, useZenWindowStore.getState().garden)
}

export function currentZenGardenId(): string | null {
  return currentZenGarden()?.id ?? null
}

/** React: the Garden this window shows. */
export function useCurrentZenGarden(): ZenGarden | null {
  const gardens = useZenGardensStore((s) => s.gardens)
  const picked = useZenWindowStore((s) => s.garden)
  return windowGardenOf(gardens, picked)
}

/** A pick the list no longer has (deleted elsewhere, never existed) moves
 *  the window to the first Garden, written back (G22). */
function reconcileWindowGarden(): void {
  const { gardens, status } = useZenGardensStore.getState()
  if (status !== 'ready' || gardens.length === 0) return
  const picked = useZenWindowStore.getState().garden
  if (!gardens.some((g) => g.id === picked)) useZenWindowStore.getState().setGarden(gardens[0].id)
}

let loadSeq = 0
let setupAsked = false

async function fetchGardens(): Promise<ZenGardensList> {
  return parseZenGardens(await daemonCliGet<unknown>(zenLocalScope(), 'zen/gardens'))
}

/**
 * Read the Garden list into `useZenGardensStore`. Zen being on with no
 * folder yet (`setUp:false`) calls `POST /cli/zen/setup` once, then reads
 * again (G22). A newer call wins.
 */
export async function loadZenGardens(): Promise<ZenGardensState> {
  const seq = ++loadSeq
  useZenGardensStore.setState({ status: 'loading', failure: null })
  let next: ZenGardensState
  try {
    let list = await fetchGardens()
    if (!list.setUp && !setupAsked) {
      setupAsked = true
      await daemonCliPost(zenLocalScope(), 'zen/setup', {})
      list = await fetchGardens()
    }
    next = { status: 'ready', setUp: list.setUp, gardens: list.gardens, failure: null }
  } catch (err) {
    next = { ...useZenGardensStore.getState(), status: 'failed', failure: zenLoadFailure(err) }
  }
  if (seq === loadSeq) {
    useZenGardensStore.setState(next)
    reconcileWindowGarden()
  }
  return next
}

/** Switch this window to Garden `id` (no-op for an id the list lacks). */
export function switchZenGarden(id: string): void {
  if (!useZenGardensStore.getState().gardens.some((g) => g.id === id)) {
    console.warn(`[zen] no Garden ${id}`)
    return
  }
  useZenWindowStore.getState().setGarden(id)
}

/** ⌘⌥1–9 in Zen (G26): Garden `index` (0-based); nothing past the end. */
export function switchZenGardenByIndex(index: number): void {
  const g = useZenGardensStore.getState().gardens[index]
  if (g) useZenWindowStore.getState().setGarden(g.id)
}

/** Why a Garden change was refused. */
export class ZenGardenError extends Error {
  readonly code: 'garden_exists' | 'last_garden' | 'bad_name' | 'failed'
  constructor(code: ZenGardenError['code'], message: string) {
    super(message)
    this.name = 'ZenGardenError'
    this.code = code
  }
}

export const zenGardenClashText = (name: string): string => `You already have a Garden called “${name}”.`

function cleanName(name: string): string {
  // eslint-disable-next-line no-control-regex
  return name.replace(/[\u0000-\u001f\u007f]/g, '').trim()
}

function mapError(err: unknown, name: string): ZenGardenError {
  const msg = err instanceof Error ? err.message : String(err)
  if (/garden_exists/.test(msg)) return new ZenGardenError('garden_exists', zenGardenClashText(name))
  if (/last_garden/.test(msg)) return new ZenGardenError('last_garden', 'That’s your last Garden.')
  return new ZenGardenError('failed', msg)
}

/** `+ New Garden` (G25): create a blank Garden named `name`, add it to the
 *  list at once, and switch this window to it. The `zen_changed` that
 *  follows re-reads the list. */
export async function createZenGarden(name: string): Promise<ZenGarden> {
  const clean = cleanName(name)
  if (clean.length === 0 || [...clean].length > 60) {
    throw new ZenGardenError('bad_name', 'A Garden name is 1 to 60 characters.')
  }
  const lower = clean.toLocaleLowerCase()
  if (useZenGardensStore.getState().gardens.some((g) => g.name.toLocaleLowerCase() === lower)) {
    throw new ZenGardenError('garden_exists', zenGardenClashText(clean))
  }
  let garden: ZenGarden
  try {
    garden = parseZenGardenNew(await daemonCliPost<unknown>(zenLocalScope(), 'zen/garden/new', { name: clean }))
  } catch (err) {
    throw err instanceof ZenPageParseError ? err : mapError(err, clean)
  }
  const st = useZenGardensStore.getState()
  if (!st.gardens.some((g) => g.id === garden.id)) {
    const gardens = [...st.gardens, { ...garden, index: st.gardens.length + 1 }]
    useZenGardensStore.setState({ gardens, setUp: true })
  }
  useZenWindowStore.getState().setGarden(garden.id)
  return garden
}

/** Rename (CLI and agents in this cut; the bridge verb behind `gardens:manage`). */
export async function renameZenGarden(id: string, name: string): Promise<void> {
  const clean = cleanName(name)
  try {
    await daemonCliPost(zenLocalScope(), 'zen/garden/rename', { garden: id, name: clean })
  } catch (err) {
    throw mapError(err, clean)
  }
}

/** Delete (CLI and agents in this cut; the bridge verb behind `gardens:manage`). */
export async function deleteZenGarden(id: string): Promise<void> {
  try {
    await daemonCliPost(zenLocalScope(), 'zen/garden/delete', { garden: id })
  } catch (err) {
    throw mapError(err, id)
  }
}

/** Tests only. */
export function __resetZenGardensForTests(): void {
  loadSeq = 0
  setupAsked = false
  useZenGardensStore.setState({ status: 'idle', setUp: false, gardens: [], failure: null })
}
