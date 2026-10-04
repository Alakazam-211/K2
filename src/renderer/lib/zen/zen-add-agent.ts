// Zen "Add agent" (Rosson 2026-10-04): the bridge verb `agents.add` opens
// K2's own Add agent picker for the CURRENT Home — the same searchable
// picker the regular Home uses (`AddAgentPicker`: This server / From a
// server, one list with a section per server, search, images). The human
// picks; the picker's own add path writes the row (and caches a remote
// row's picture), so the new row shows in the Agents widget at once (it
// reads the selected Home's rows).
//
// A widget only OPENS the picker. It never writes Home rows itself, so a v2
// user widget with `agents:add` can offer the same button and nothing more.
//
//   bridge.call('agents.add')                          open, bottom-left
//   bridge.call('agents.add', { anchor: el })          open above `el`
//   bridge.call('agents.add', { anchor: el, toggle: true })  close when open
//
// Returns whether the picker is open after the call. The picker closes on
// Esc, a click outside it, a Home switch, and when Zen leaves the window
// (`ZenAddAgentPicker` unmounts).

import { create } from 'zustand'
import { registerZenVerb, ZenBridgeError } from './zen-bridge'
import { closeZenOverlays } from './zen-theme-switch'

export interface ZenAddAgentState {
  open: boolean
  /** The element the picker opens above (the widget's button), if any. */
  anchor: HTMLElement | null
}

export const useZenAddAgentStore = create<ZenAddAgentState>(() => ({ open: false, anchor: null }))

export function openZenAddAgent(anchor: HTMLElement | null = null): void {
  // One K2 overlay at a time: the theme picker and the cheat sheet close.
  closeZenOverlays()
  useZenAddAgentStore.setState({ open: true, anchor })
}

export function closeZenAddAgent(): void {
  if (!useZenAddAgentStore.getState().open && useZenAddAgentStore.getState().anchor === null) return
  useZenAddAgentStore.setState({ open: false, anchor: null })
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
export function zenAgentsAdd(raw?: unknown): boolean {
  const opts = parseOptions(raw)
  if (opts.toggle && useZenAddAgentStore.getState().open) {
    closeZenAddAgent()
    return false
  }
  openZenAddAgent(opts.anchor ?? null)
  return true
}

/** Register `agents.add` on the bridge. Returns the uninstall (which also
 *  closes the picker). */
export function installZenAddAgentVerb(): () => void {
  const off = registerZenVerb('agents.add', (_ctx, opts) => zenAgentsAdd(opts))
  return () => {
    off()
    closeZenAddAgent()
  }
}
