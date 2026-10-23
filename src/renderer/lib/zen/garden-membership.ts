// Which widgets and data sources each Garden uses, for Settings → Gardens
// (Rosson 2026-10-23: a Garden's pane shows its Local Data and External
// Data; what no Garden uses sits under "Not in a Garden").
//
// Thin client: the facts come from this computer's daemon, one
// `GET /cli/zen/get?garden=<id>` per Garden (the resolved page: each custom
// placement's `widget` and the `sourceAddresses` the daemon fills from the
// placement's config or the widget's manifest). Nothing here is stored; the
// page re-reads it with the Garden list.
//
// A source address is `name` (this computer) or `name::host`. The External
// Data list is the window's server's (`primaryScope()`), so an address
// counts there only when it names that server: a bare name while the window
// is on this computer, or `name::<host>` whose host is the window's server.

import { daemonCliGet } from '@/lib/daemon-cli'
import { canonicalHostKey } from '@/lib/host-key'
import { zenLocalScope } from './zen-api'

/** What one Garden's resolved page uses. */
export interface GardenUses {
  /** Custom widget names placed on the page (built-ins included). */
  widgets: string[]
  /** Source addresses its cards read (`name` or `name::host`). */
  sources: string[]
}

export interface GardenMembership {
  widgets: Map<string, Set<string>>
  /** Source NAMES on the window's server, per Garden. */
  sources: Map<string, Set<string>>
  placedWidgets: Set<string>
  usedSources: Set<string>
}

function obj(v: unknown): Record<string, unknown> | null {
  return v !== null && typeof v === 'object' && !Array.isArray(v) ? (v as Record<string, unknown>) : null
}

/** The widgets and source addresses on one `zen/get` answer (pure). */
export function gardenPageUses(raw: unknown): GardenUses {
  const o = obj(raw)
  const page = obj(o?.page) ?? o
  const out: GardenUses = { widgets: [], sources: [] }
  const widgets = page?.widgets
  if (!Array.isArray(widgets)) return out
  for (const w of widgets) {
    const r = obj(w)
    if (!r || r.kind !== 'custom') continue
    if (typeof r.widget === 'string' && r.widget !== '' && !out.widgets.includes(r.widget)) out.widgets.push(r.widget)
    if (Array.isArray(r.sourceAddresses)) {
      for (const a of r.sourceAddresses) {
        if (typeof a === 'string' && a !== '' && !out.sources.includes(a)) out.sources.push(a)
      }
    }
  }
  return out
}

/** The source name an address names on the server `hostKey` (the window's
 *  `primaryScope().hostKey`; `local` for this computer), or null when it
 *  lives on another server. */
export function sourceNameOn(address: string, hostKey: string): string | null {
  const at = address.indexOf('::')
  if (at < 0) return hostKey === 'local' ? address : null
  const name = address.slice(0, at)
  let host: string
  try {
    host = canonicalHostKey(address.slice(at + 2))
  } catch {
    return null
  }
  let mine: string
  try {
    mine = canonicalHostKey(hostKey)
  } catch {
    return null
  }
  return host === mine ? name : null
}

/** Per-Garden widgets and window-server source names (pure). */
export function buildGardenMembership(uses: ReadonlyMap<string, GardenUses>, hostKey: string): GardenMembership {
  const m: GardenMembership = { widgets: new Map(), sources: new Map(), placedWidgets: new Set(), usedSources: new Set() }
  for (const [garden, u] of uses) {
    const ws = new Set(u.widgets)
    const ss = new Set<string>()
    for (const a of u.sources) {
      const n = sourceNameOn(a, hostKey)
      if (n !== null) ss.add(n)
    }
    m.widgets.set(garden, ws)
    m.sources.set(garden, ss)
    ws.forEach((w) => m.placedWidgets.add(w))
    ss.forEach((s) => m.usedSources.add(s))
  }
  return m
}

/** Read every Garden's page from this computer's daemon. */
export async function fetchGardenUses(gardenIds: readonly string[]): Promise<Map<string, GardenUses>> {
  const pages = await Promise.all(
    gardenIds.map(async (id) => [id, gardenPageUses(await daemonCliGet<unknown>(zenLocalScope(), 'zen/get', { garden: id }))] as const),
  )
  return new Map(pages)
}
