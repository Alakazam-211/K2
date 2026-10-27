// Mixed-version guard for Mobile tabs (prd-mobile-dev-v1 M-A7), a copy of
// the Design tab's (design-serialize.ts, D-A4).
//
// An unknown serialized item type restores as an empty file-viewer on an
// older client, and an older client re-saving the shared layout would turn
// a Mobile tab into a broken file tab. So a Mobile item is saved as a
// Browser item (`type: 'browser'`, `url: ''`) with an extra `mobile` field.
// A new client reads the field and restores a Mobile item; an older client
// ignores it and shows an empty Browser tab. If the older client re-saves,
// the item drops back to an empty Browser tab (harmless).
// `LAYOUT_SCHEMA_VERSION` does not change.
//
// The device itself is never here: the daemon owns the workspace's sim
// session (one per workspace in v1); every Mobile tab shows that device.

import type { SerializedBrowserItem } from '@/stores/tabs'

/** Bumped only if the `mobile` marker itself grows a breaking field. */
export const MOBILE_MARKER_VERSION = 1

/** The `mobile` item's data (M-A7). */
export interface MobileItemData {
  /** The device profile this tab last asked for ("Pixel 9"); the daemon's
   *  sim session is the truth for what is running. */
  deviceType?: string
}

export interface MobileMarker {
  v: number
  deviceType?: string
}

/** What a Mobile item saves as: an empty Browser item plus the marker. */
export interface SerializedMobileItem extends SerializedBrowserItem {
  mobile: MobileMarker
}

export function serializeMobileItem(id: string, data: MobileItemData): SerializedMobileItem {
  return {
    id,
    type: 'browser',
    url: '',
    title: 'Mobile',
    mobile: {
      v: MOBILE_MARKER_VERSION,
      ...(data.deviceType ? { deviceType: data.deviceType } : {}),
    },
  }
}

/** True when a saved Browser item is really a Mobile item. */
export function isSerializedMobile(si: unknown): si is SerializedMobileItem {
  if (!si || typeof si !== 'object') return false
  const o = si as { type?: unknown; mobile?: unknown }
  if (o.type !== 'browser') return false
  const m = o.mobile
  return !!m && typeof m === 'object' && typeof (m as { v?: unknown }).v === 'number'
}

/** The Mobile data for a saved Browser item carrying the marker, else null
 *  (restore it as a Browser or Design item). */
export function mobileDataFromSerializedBrowser(si: unknown): MobileItemData | null {
  if (!isSerializedMobile(si)) return null
  const deviceType = si.mobile.deviceType
  return typeof deviceType === 'string' && deviceType ? { deviceType } : {}
}
