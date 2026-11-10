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
// workspace whose strip holds it (vs-live R22), and every Desktop tab on
// that server shows the same `console` desktop.
//
// Pure: no React, no DOM.

import type { SerializedBrowserItem } from '@/stores/tabs'

/** Bumped only if the `desktop` marker itself grows a breaking field. */
export const DESKTOP_MARKER_VERSION = 1

export type DesktopFit = 'fit' | '1:1'

/** The `desktop` item's data. */
export interface DesktopItemData {
  /** `console` in v1 (the logged-in user's screen). */
  desktopId: 'console'
  /** The display this tab last picked; the daemon's list is the truth. */
  displayId?: string
  fit?: DesktopFit
  /** This viewer asked to watch only. */
  viewOnly?: boolean
}

export interface DesktopMarker {
  v: number
  desktopId: 'console'
  displayId?: string
  fit?: DesktopFit
  viewOnly?: boolean
}

/** What a Desktop item saves as: an empty Browser item plus the marker. */
export interface SerializedDesktopItem extends SerializedBrowserItem {
  desktop: DesktopMarker
}

export function serializeDesktopItem(id: string, data: DesktopItemData): SerializedDesktopItem {
  return {
    id,
    type: 'browser',
    url: '',
    title: 'Desktop',
    desktop: {
      v: DESKTOP_MARKER_VERSION,
      desktopId: 'console',
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
 *  null (restore it as a Browser, Design or Mobile item). */
export function desktopDataFromSerializedBrowser(si: unknown): DesktopItemData | null {
  if (!isSerializedDesktop(si)) return null
  const m = si.desktop
  const out: DesktopItemData = { desktopId: 'console' }
  if (typeof m.displayId === 'string' && m.displayId) out.displayId = m.displayId
  if (m.fit === 'fit' || m.fit === '1:1') out.fit = m.fit
  if (m.viewOnly === true) out.viewOnly = true
  return out
}
