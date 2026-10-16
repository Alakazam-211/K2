// Mixed-version guard for Design tabs (vs-live D-A4).
//
// An unknown serialized item type restores as an empty file-viewer on an
// older client, and an older client re-saving the shared layout would turn
// a Design tab into a broken file tab. So a Design item is saved as a
// Browser item (`type: 'browser'`, same url/title/icon) with an extra
// `design` field. A new client reads the field and restores a Design item;
// an older client ignores it and shows a Browser tab at the same URL. If the
// older client re-saves, the item drops back to a Browser tab (harmless,
// Q-F3). `LAYOUT_SCHEMA_VERSION` does not change.
//
// Notes drafts are never here: they are per viewer (useDesignNotes).

import type { SerializedBrowserItem } from '@/stores/tabs'

/** Bumped only if the `design` marker itself grows a breaking field. */
export const DESIGN_MARKER_VERSION = 1

/** The `design` item's data (D-A2), mirroring `BrowserItemData`. */
export interface DesignItemData {
  url: string
  title?: string
  /** Paintable favicon (`data:image/…` or `blob:`). */
  icon?: string
  /** This window's own navigate requests (never serialized). */
  navSeq?: number
}

export interface DesignMarker {
  v: number
}

/** What a Design item saves as: a Browser item plus the marker. */
export interface SerializedDesignItem extends SerializedBrowserItem {
  design: DesignMarker
}

export function serializeDesignItem(id: string, data: DesignItemData): SerializedDesignItem {
  return {
    id,
    type: 'browser',
    url: data.url,
    ...(data.title !== undefined ? { title: data.title } : {}),
    ...(data.icon !== undefined ? { icon: data.icon } : {}),
    design: { v: DESIGN_MARKER_VERSION },
  }
}

/** True when a saved Browser item is really a Design item. */
export function isSerializedDesign(si: unknown): si is SerializedDesignItem {
  if (!si || typeof si !== 'object') return false
  const o = si as { type?: unknown; design?: unknown }
  if (o.type !== 'browser') return false
  const d = o.design
  return !!d && typeof d === 'object' && typeof (d as { v?: unknown }).v === 'number'
}

/**
 * The Design data for a saved Browser item carrying the marker, else null
 * (restore it as a plain Browser item). `liveNavSeq` keeps this window's
 * navigate counter across a layout re-apply, like the Browser branch does.
 */
export function designDataFromSerializedBrowser(si: unknown, liveNavSeq?: number): DesignItemData | null {
  if (!isSerializedDesign(si)) return null
  return {
    url: typeof si.url === 'string' ? si.url : '',
    ...(typeof si.title === 'string' ? { title: si.title } : {}),
    ...(typeof si.icon === 'string' ? { icon: si.icon } : {}),
    ...(liveNavSeq !== undefined ? { navSeq: liveNavSeq } : {}),
  }
}
