// Mixed-version guard for Desktop tabs (prd-remote-desktop-tab-v1 §8), a
// copy of the Mobile tab's (mobile-serialize.ts, M-A7).
//
// A Desktop item is saved as a Browser item (`type: 'browser'`, `url: ''`)
// with an extra `desktop` field. A new client reads the field and restores
// a Desktop item; an older client ignores it and shows an empty Browser tab,
// and if it re-saves, the item drops back to an empty Browser tab
// (harmless). `LAYOUT_SCHEMA_VERSION` does not change.
//
// The machine is never here: the tab dials the server that owns the
// workspace whose strip holds it (vs-live R22). Which desktop on that server
// it shows is (prd-linux-virtual-desktop-v1 §6, LD-35): This server
// (`console`, or `virtual` on a Linux server with no screen) or an agent's
// (`virtual:<workspace id>`). An older marker always said `console` and
// restores as This server.
//
// Pure: no React, no DOM.

import type { SerializedBrowserItem } from '@/stores/tabs'

/** Bumped only if the `desktop` marker itself grows a breaking field. */
export const DESKTOP_MARKER_VERSION = 1

export type DesktopFit = 'fit' | '1:1'

/** This server's own desktop id in an older marker, and what a tab that
 *  never picked saves as. */
export const SERVER_DESKTOP_ID = 'console'

/** The daemon's desktop id grammar (§3.1): `[a-z0-9:_-]`, 1–64 chars. */
const DESKTOP_ID_RE = /^[a-z0-9:_-]{1,64}$/

/** The `desktop` item's data. */
export interface DesktopItemData {
  /** The desktop this tab shows. Missing on a new tab until the pane picks
   *  the first one (its workspace's agent desktop, else This server). */
  desktopId?: string
  /** The agent's name, for an agent's desktop (the tab title). */
  name?: string
  /** The display this tab last picked; the daemon's list is the truth. */
  displayId?: string
  fit?: DesktopFit
  /** This viewer asked to watch only. */
  viewOnly?: boolean
}

export interface DesktopMarker {
  v: number
  desktopId: string
  name?: string
  displayId?: string
  fit?: DesktopFit
  viewOnly?: boolean
}

/** What a Desktop item saves as: an empty Browser item plus the marker. */
export interface SerializedDesktopItem extends SerializedBrowserItem {
  desktop: DesktopMarker
}

/** This server's desktop: `console` (a real screen) or `virtual` (a Linux
 *  server with none); a tab that never picked shows it too. A server has
 *  only one of the two, so they name the same desktop. */
export function isServerDesktop(id: string | undefined): boolean {
  return !id || id === 'console' || id === 'virtual'
}

/** Do two desktop ids name the same desktop on one server? */
export function sameDesktop(a: string | undefined, b: string | undefined): boolean {
  if (isServerDesktop(a) || isServerDesktop(b)) return isServerDesktop(a) && isServerDesktop(b)
  return a === b
}

/** The tab's title: "Desktop" for This server, "Desktop: <agent>" for an
 *  agent's (§6). */
export function desktopTabTitle(data: Pick<DesktopItemData, 'desktopId' | 'name'>): string {
  if (isServerDesktop(data.desktopId) || !data.name) return 'Desktop'
  return `Desktop: ${data.name}`
}

export function serializeDesktopItem(id: string, data: DesktopItemData): SerializedDesktopItem {
  return {
    id,
    type: 'browser',
    url: '',
    title: desktopTabTitle(data),
    desktop: {
      v: DESKTOP_MARKER_VERSION,
      desktopId: data.desktopId || SERVER_DESKTOP_ID,
      ...(data.name && !isServerDesktop(data.desktopId) ? { name: data.name } : {}),
      ...(data.displayId ? { displayId: data.displayId } : {}),
      ...(data.fit ? { fit: data.fit } : {}),
      ...(data.viewOnly ? { viewOnly: true } : {}),
    },
  }
}

/** True when a saved Browser item is really a Desktop item. */
export function isSerializedDesktop(si: unknown): si is SerializedDesktopItem {
  if (!si || typeof si !== 'object') return false
  const o = si as { type?: unknown; desktop?: unknown }
  if (o.type !== 'browser') return false
  const m = o.desktop
  return !!m && typeof m === 'object' && typeof (m as { v?: unknown }).v === 'number'
}

/** The Desktop data for a saved Browser item carrying the marker, else
 *  null (restore it as a Browser, Design or Mobile item). An id that isn't
 *  a desktop id restores as This server. */
export function desktopDataFromSerializedBrowser(si: unknown): DesktopItemData | null {
  if (!isSerializedDesktop(si)) return null
  const m = si.desktop as unknown as Record<string, unknown>
  const desktopId = typeof m.desktopId === 'string' && DESKTOP_ID_RE.test(m.desktopId) ? m.desktopId : SERVER_DESKTOP_ID
  const out: DesktopItemData = { desktopId }
  if (typeof m.name === 'string' && m.name && !isServerDesktop(desktopId)) out.name = m.name
  if (typeof m.displayId === 'string' && m.displayId) out.displayId = m.displayId
  if (m.fit === 'fit' || m.fit === '1:1') out.fit = m.fit
  if (m.viewOnly === true) out.viewOnly = true
  return out
}
