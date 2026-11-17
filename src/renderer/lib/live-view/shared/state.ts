// The header's one state word: a dot and a word, one treatment for every
// state (Live, Live · picture behind, Loading, Reconnecting, Can't
// reconnect, Offline, Edits paused, File missing, Read only). The bar under
// the header says the rest once; the chip only names the state.

import type { ConnectionState, OutsideSave } from './notices'

export type LiveTabState = 'loading' | 'live' | 'live-behind' | 'reconnecting' | 'cant' | 'offline' | 'paused' | 'missing' | 'read-only' | 'read-only-file'

export type StateTone = 'ok' | 'warn' | 'danger' | 'muted'

const CHIPS: Record<LiveTabState, { word: string; tone: StateTone }> = {
  loading: { word: 'Loading', tone: 'muted' },
  live: { word: 'Live', tone: 'ok' },
  'live-behind': { word: 'Live · picture behind', tone: 'warn' },
  reconnecting: { word: 'Reconnecting', tone: 'warn' },
  cant: { word: 'Can’t reconnect', tone: 'danger' },
  offline: { word: 'Offline', tone: 'muted' },
  paused: { word: 'Edits paused', tone: 'warn' },
  missing: { word: 'File missing', tone: 'warn' },
  'read-only': { word: 'Read only (saved version)', tone: 'muted' },
  // The editors: a live file you can read but not edit (no saved version involved).
  'read-only-file': { word: 'Read only', tone: 'muted' },
}

export function stateChip(s: LiveTabState): { word: string; tone: StateTone } {
  return CHIPS[s]
}

export type LiveTier = 'loading' | 'live' | 'saved-only'

export function liveTabState({
  tier,
  connection,
  outsideSave,
  behind = false,
  readOnly = false,
}: {
  tier: LiveTier
  connection: ConnectionState | null
  outsideSave: OutsideSave | null
  /** The picture (a scene still, a timeline frame) lags the live file. */
  behind?: boolean
  /** Live, but the file can't be changed here. */
  readOnly?: boolean
}): LiveTabState {
  if (tier === 'loading') return 'loading'
  if (tier === 'saved-only') return 'read-only'
  if (connection === 'reconnecting' || connection === 'trying') return 'reconnecting'
  if (connection === 'cant') return 'cant'
  if (connection === 'offline') return 'offline'
  if (outsideSave?.outcome === 'declined') return 'paused'
  if (outsideSave?.outcome === 'missing') return 'missing'
  if (readOnly) return 'read-only-file'
  return behind ? 'live-behind' : 'live'
}

/** Not connected: Watching reads "as you last saw", the say field is off. */
export const isDisconnected = (c: ConnectionState | null): boolean => c === 'reconnecting' || c === 'trying' || c === 'cant' || c === 'offline'
