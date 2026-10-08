// prd-zen-gardens-v1 item 17 (Rosson's smoke round 3, 2026-10-04) —
// Settings → Gardens: manage this computer's Gardens outside Zen.
//
// Same rules as the rest of Zen: Gardens live in `~/.k2/zen`, owned by THIS
// computer's daemon, so every request goes to `zenLocalScope()`
// (`scopeForHost('local')`, owner token), never `primaryScope()`.
//
//   GET  /cli/zen/gardens                       the list (`setUp:false` = not set up)
//   GET  /cli/zen/status                        the folder path (works before setup)
//   GET  /cli/zen/theme/list                    themes + the global pick (needs setup)
//   POST /cli/zen/setup {}                      "Set up Gardens" (never automatic here)
//   POST /cli/zen/garden/new {name, template}   template "texting" | "blank"
//   POST /cli/zen/garden/rename {garden, name}
//   POST /cli/zen/garden/reorder {garden, to}   `to` is 1-based
//   POST /cli/zen/garden/delete {garden}        the page file moves to history
//   POST /cli/zen/theme/set {name} | {name, garden} | {garden, clear:true}
//
// The section re-reads on every local `zen_changed` (`watchLocalZenChanged`),
// the same signal the Garden switcher uses, so a change made here shows in an
// open Zen window and the reverse.
//
// Where it shows: exactly where the Zen toggle shows (`zenAvailable()`: the
// desktop app, not the web client, not Windows until G-Win, not Focus or
// ticket windows), minus a computer whose daemon has no Gardens routes. The
// local scope reports every feature as supported (docs/zen-contract.md,
// renderer note 1), so `zen-gardens-v1` is read the way the rest of Zen reads
// it: `{error: "unknown zen route"}` on `GET /cli/zen/gardens` = no Gardens.

import { create } from 'zustand'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { zenAvailable } from './zen-platform'
import { zenLocalScope, zenLoadFailure, type ZenLoadFailure } from './zen-api'
import { parseZenGardens, zenGardenClashText, type ZenGarden } from './zen-gardens'
import { ZenPageParseError } from './zen-page'
import { loadZenTemplates, zenTemplateName } from './zen-templates'

/** One Garden as Settings shows it: the list entry plus its own theme pick. */
export interface ZenSettingsGarden extends ZenGarden {
  /** The Garden's own theme, or null when it follows the global theme. */
  theme: string | null
}

export interface ZenSettingsTheme {
  name: string
  builtin: boolean
  summary: string
}

export type ZenSettingsState = {
  status: 'idle' | 'loading' | 'ready' | 'failed'
  setUp: boolean
  gardens: ZenSettingsGarden[]
  themes: ZenSettingsTheme[]
  /** The computer-wide theme (`theme/list` `global`). */
  globalTheme: string | null
  /** `~/.k2/zen` as the daemon resolved it. */
  path: string | null
  failure: ZenLoadFailure | null
}

const EMPTY: ZenSettingsState = {
  status: 'idle',
  setUp: false,
  gardens: [],
  themes: [],
  globalTheme: null,
  path: null,
  failure: null,
}

export const useZenSettingsStore = create<ZenSettingsState>(() => ({ ...EMPTY }))

/** Pure: the Settings "Gardens" item shows where Zen exists (`available`,
 *  the Zen toggle's own predicate) and the local daemon has Gardens. */
export function zenGardensSettingsShownFor(available: boolean, failure: ZenLoadFailure | null): boolean {
  return available && failure?.kind !== 'outdated'
}

/** Is Settings → Gardens shown in this window right now? */
export function zenGardensSettingsShown(): boolean {
  return zenGardensSettingsShownFor(zenAvailable(), useZenSettingsStore.getState().failure)
}

/** React: is Settings → Gardens shown in this window? */
export function useZenGardensSettingsShown(): boolean {
  const failure = useZenSettingsStore((s) => s.failure)
  return zenGardensSettingsShownFor(zenAvailable(), failure)
}

/** What a template is called in Settings. */
export function zenTemplateLabel(template: string): string {
  // One list for every template name (UWB23): the starts, then the catalog.
  return zenTemplateName(template)
}

function isObj(v: unknown): v is Record<string, unknown> {
  return v !== null && typeof v === 'object' && !Array.isArray(v)
}

/** `GET /cli/zen/gardens` with each Garden's `theme` kept. */
export function parseZenSettingsGardens(raw: unknown): { setUp: boolean; gardens: ZenSettingsGarden[] } {
  const list = parseZenGardens(raw)
  const themes = new Map<string, string | null>()
  if (isObj(raw) && Array.isArray(raw.gardens)) {
    for (const g of raw.gardens) {
      if (isObj(g) && typeof g.id === 'string') {
        themes.set(g.id, typeof g.theme === 'string' && g.theme ? g.theme : null)
      }
    }
  }
  return { setUp: list.setUp, gardens: list.gardens.map((g) => ({ ...g, theme: themes.get(g.id) ?? null })) }
}

/** `GET /cli/zen/theme/list`: the theme names (daemon order) and the global pick. */
export function parseZenThemeList(raw: unknown): { themes: ZenSettingsTheme[]; global: string | null } {
  if (!isObj(raw) || !Array.isArray(raw.themes)) throw new ZenPageParseError('no themes in the answer')
  const themes: ZenSettingsTheme[] = []
  const seen = new Set<string>()
  for (const t of raw.themes) {
    if (!isObj(t) || typeof t.name !== 'string' || !t.name || seen.has(t.name)) continue
    seen.add(t.name)
    themes.push({
      name: t.name,
      builtin: t.builtin === true,
      summary: typeof t.summary === 'string' ? t.summary : '',
    })
  }
  return { themes, global: typeof raw.global === 'string' && raw.global ? raw.global : null }
}

function parseStatusPath(raw: unknown): string | null {
  return isObj(raw) && typeof raw.path === 'string' && raw.path ? raw.path : null
}

let loadSeq = 0

/**
 * Read the list, the folder path and (once set up) the themes into
 * `useZenSettingsStore`. Never sets Zen up. A newer call wins.
 */
export async function loadZenSettings(): Promise<ZenSettingsState> {
  const seq = ++loadSeq
  useZenSettingsStore.setState({ status: 'loading' })
  let next: ZenSettingsState
  try {
    const scope = zenLocalScope()
    const [gardensRaw, statusRaw] = await Promise.all([
      daemonCliGet<unknown>(scope, 'zen/gardens'),
      daemonCliGet<unknown>(scope, 'zen/status'),
    ])
    const list = parseZenSettingsGardens(gardensRaw)
    let themes: ZenSettingsTheme[] = []
    let globalTheme: string | null = null
    if (list.setUp) {
      const [themeRaw] = await Promise.all([
        daemonCliGet<unknown>(scope, 'zen/theme/list'),
        // The Garden catalog never fails the section.
        loadZenTemplates(),
      ])
      const t = parseZenThemeList(themeRaw)
      themes = t.themes
      globalTheme = t.global
    }
    next = {
      status: 'ready',
      setUp: list.setUp,
      gardens: list.gardens,
      themes,
      globalTheme,
      path: parseStatusPath(statusRaw),
      failure: null,
    }
  } catch (err) {
    next = { ...useZenSettingsStore.getState(), status: 'failed', failure: zenLoadFailure(err) }
  }
  if (seq === loadSeq) useZenSettingsStore.setState(next)
  return next
}

/** A daemon refusal, in words the person reads (shown inline). */
export function zenSettingsErrorText(err: unknown, name?: string): string {
  const msg = err instanceof Error ? err.message : String(err)
  if (/garden_exists/.test(msg)) return name ? zenGardenClashText(name) : 'You already have a Garden with that name.'
  if (/last_garden/.test(msg)) return 'That’s your last Garden. You need at least one.'
  if (/zen_not_set_up/.test(msg)) return 'Gardens aren’t set up on this computer yet.'
  if (/unknown_garden/.test(msg)) return 'That Garden is gone. It may have been deleted somewhere else.'
  if (/unknown_theme/.test(msg)) return 'That theme is gone. Pick another one.'
  if (/unknown zen route/i.test(msg)) return 'K2 on this computer is older than this app. Update it to use Gardens.'
  return msg || 'Something went wrong.'
}

function cleanName(name: string): string {
  // eslint-disable-next-line no-control-regex
  return name.replace(/[\u0000-\u001f\u007f]/g, '').trim()
}

/** Check a Garden name before posting (the daemon checks again). Returns
 *  the cleaned name, or throws an `Error` with the inline text. */
export function checkZenGardenName(name: string, gardens: readonly ZenGarden[], selfId?: string): string {
  const clean = cleanName(name)
  if (clean.length === 0 || [...clean].length > 60) throw new Error('A Garden name is 1 to 60 characters.')
  const lower = clean.toLocaleLowerCase()
  if (gardens.some((g) => g.id !== selfId && g.name.toLocaleLowerCase() === lower)) {
    throw new Error(zenGardenClashText(clean))
  }
  return clean
}

/** The first free "Garden N" name, for a New Garden left unnamed. */
export function nextZenGardenName(gardens: readonly ZenGarden[]): string {
  const taken = new Set(gardens.map((g) => g.name.toLocaleLowerCase()))
  for (let n = gardens.length + 1; ; n++) {
    const name = `Garden ${n}`
    if (!taken.has(name.toLocaleLowerCase())) return name
  }
}

async function post(route: string, body: Record<string, unknown>, name?: string): Promise<void> {
  try {
    await daemonCliPost(zenLocalScope(), route, body)
  } catch (err) {
    throw new Error(zenSettingsErrorText(err, name))
  }
  // `zen_changed` follows every real change; re-read now as well so this
  // section never waits on the socket.
  await loadZenSettings()
}

/** "Set up Gardens": `POST /cli/zen/setup {}`. */
export function setupZenGardens(): Promise<void> {
  return post('zen/setup', {})
}

/** A template's short name: `texting`, `blank` or a catalog Garden's (UWB23). */
export type ZenNewGardenTemplate = 'texting' | 'blank' | (string & {})

/** + New Garden. `texting` = "Start with the default", `blank` = "Start
 *  empty and ask my agent", or a ready-made Garden from the catalog (its
 *  widgets work at once: no permissions). Does not switch any Zen window
 *  to it. */
export function createZenSettingsGarden(name: string, template: ZenNewGardenTemplate): Promise<void> {
  return post('zen/garden/new', { name, template }, name)
}

export function renameZenSettingsGarden(id: string, name: string): Promise<void> {
  return post('zen/garden/rename', { garden: id, name }, name)
}

/** Move Garden `id` to 1-based position `to`. */
export function moveZenSettingsGarden(id: string, to: number): Promise<void> {
  return post('zen/garden/reorder', { garden: id, to })
}

export function deleteZenSettingsGarden(id: string): Promise<void> {
  return post('zen/garden/delete', { garden: id })
}

/** The computer-wide theme. */
export function setZenGlobalTheme(name: string): Promise<void> {
  return post('zen/theme/set', { name })
}

/** One Garden's own theme, or `null` to follow the global theme. */
export function setZenGardenTheme(id: string, name: string | null): Promise<void> {
  return post('zen/theme/set', name === null ? { garden: id, clear: true } : { name, garden: id })
}

/** Tests only. */
export function __resetZenSettingsForTests(): void {
  loadSeq = 0
  useZenSettingsStore.setState({ ...EMPTY })
}
