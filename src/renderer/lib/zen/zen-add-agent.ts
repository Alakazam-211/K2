// Zen "Add agent" (Rosson 2026-10-04; prd-zen-gardens-v1 G33, G55): the
// bridge verb `agents.add` opens K2's own Add agent picker for the Home the
// calling Agents widget shows (its own Home picker's pick, G27) — never the
// window's selected Home. It is the same searchable picker the regular Home
// uses (`AddAgentPicker`: This server / From a server, one list with a
// section per server, search, images). The human picks; the picker's own
// add path writes the row into that Home (and caches a remote row's
// picture), so the new row shows in the Agents widget at once. The Home
// page's own selection never moves. The template's Add agent button acts
// for the page's first Agents widget.
//
// A widget only OPENS the picker. It never writes Home rows itself, so a v2
// user widget with `agents:add` can offer the same button and nothing more.
//
//   bridge.call('agents.add')                          open, bottom-left
//   bridge.call('agents.add', { anchor: el })          open above `el`
//   bridge.call('agents.add', { anchor: el, toggle: true })  close when open
//
// Returns whether the picker is open after the call. The picker closes on
// Esc, a click outside it, that widget's Home changing, a Garden switch,
// when Zen leaves the window (`ZenAddAgentPicker` unmounts), and when the
// person adds an agent: Zen then opens that agent's conversation with the
// caret in its box (Rosson 2026-10-04; `added`, read by the Agents widget).

import { create } from 'zustand'
import { registerZenVerb, ZenBridgeError, type ZenVerbCtx } from './zen-bridge'
import { closeZenOverlays } from './zen-theme-switch'
import { zenAddTargetForCtx } from './zen-data'

export interface ZenAddAgentState {
  open: boolean
  /** The element the picker opens above (the widget's button), if any. */
  anchor: HTMLElement | null
  /** The Home the rows go into (the calling widget's Home). */
  homeId: string | null
  /** The Garden it was opened in. */
  gardenId: string | null
  /** The Agents widget's Home-pick key (`<gardenId>/<widgetId>`). */
  viewKey: string | null
  /** The last agent the picker added, for the widget that opened it. */
  added: { viewKey: string; address: string; seq: number } | null
}

const CLOSED = { open: false, anchor: null, homeId: null, gardenId: null, viewKey: null } as const

export const useZenAddAgentStore = create<ZenAddAgentState>(() => ({ ...CLOSED, added: null }))

export function openZenAddAgent(
  target: { homeId: string; gardenId: string; viewKey: string },
  anchor: HTMLElement | null = null,
): void {
  // One K2 overlay at a time: the theme picker and the cheat sheet close.
  closeZenOverlays()
  useZenAddAgentStore.setState({ open: true, anchor, ...target })
}

export function closeZenAddAgent(): void {
  const s = useZenAddAgentStore.getState()
  if (!s.open && s.anchor === null && s.homeId === null) return
  useZenAddAgentStore.setState({ ...CLOSED })
}

let addSeq = 0

/** The picker added `address` to its Home: close it, and tell the widget
 *  that opened it (it opens the conversation and focuses the box). */
export function noteZenAgentAdded(address: string): void {
  const viewKey = useZenAddAgentStore.getState().viewKey
  closeZenAddAgent()
  if (viewKey) useZenAddAgentStore.setState({ added: { viewKey, address, seq: ++addSeq } })
}

export interface ZenAgentsAddOptions {
  anchor?: HTMLElement | null
  toggle?: boolean
}

function parseOptions(raw: unknown): ZenAgentsAddOptions {
  if (raw === undefined || raw === null) return {}
  if (typeof raw !== 'object' || Array.isArray(raw)) {
    throw new ZenBridgeError('unknown_verb', 'agents.add', 'options must be an object')
  }
  const o = raw as Record<string, unknown>
  if (o.anchor !== undefined && o.anchor !== null && !(o.anchor instanceof HTMLElement)) {
    throw new ZenBridgeError('unknown_verb', 'agents.add', 'anchor must be an element')
  }
  if (o.toggle !== undefined && typeof o.toggle !== 'boolean') {
    throw new ZenBridgeError('unknown_verb', 'agents.add', 'toggle must be a boolean')
  }
  return { anchor: (o.anchor as HTMLElement | null | undefined) ?? null, toggle: o.toggle as boolean | undefined }
}

/** The `agents.add` verb. Returns whether the picker is open afterwards. */
export function zenAgentsAdd(ctx: ZenVerbCtx, raw?: unknown): boolean {
  const opts = parseOptions(raw)
  if (opts.toggle && useZenAddAgentStore.getState().open) {
    closeZenAddAgent()
    return false
  }
  const view = zenAddTargetForCtx(ctx)
  if (!view || !view.homeId) throw new ZenBridgeError('verb_unavailable', 'agents.add', 'this page has no Agents widget')
  openZenAddAgent({ homeId: view.homeId, gardenId: view.gardenId, viewKey: view.key }, opts.anchor ?? null)
  return true
}

/** Register `agents.add` on the bridge. Returns the uninstall (which also
 *  closes the picker). */
export function installZenAddAgentVerb(): () => void {
  const off = registerZenVerb('agents.add', (ctx, opts) => zenAgentsAdd(ctx, opts))
  return () => {
    off()
    closeZenAddAgent()
  }
}
