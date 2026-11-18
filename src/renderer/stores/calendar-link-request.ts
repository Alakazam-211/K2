// A one-shot "link this calendar, read-only" request from outside Settings:
// the Thread card an agent posts with `k2 cal link --request` (calendar S1,
// prd-google-calendar-link-v1 §3.1, flow 3). The card's button opens
// Settings on Email Link with the Connect form filled in (calendar,
// read-only); the person still signs in. The agent never holds the link.

import { create } from 'zustand'
import { useSettingsStore } from '@/stores/settings'

export interface CalendarLinkRequest {
  address: string
  provider: 'gmail' | 'microsoft'
  /** The workspace (path or id) whose agent asked. */
  workspace: string
}

interface CalendarLinkRequestState {
  pending: CalendarLinkRequest | null
  request: (req: CalendarLinkRequest) => void
  /** Take the request (the Connect form calls it once it fills in). */
  take: () => CalendarLinkRequest | null
}

export const useCalendarLinkRequestStore = create<CalendarLinkRequestState>((set, get) => ({
  pending: null,
  request: (req) => set({ pending: req }),
  take: () => {
    const p = get().pending
    if (p) set({ pending: null })
    return p
  },
}))

/** The card body's words → a request; anything else is null (no button). */
export function parseCalendarLinkRequest(body: unknown): CalendarLinkRequest | null {
  if (!body || typeof body !== 'object' || Array.isArray(body)) return null
  const b = body as Record<string, unknown>
  const address = typeof b.address === 'string' ? b.address.trim() : ''
  const provider = b.provider === 'gmail' || b.provider === 'microsoft' ? b.provider : null
  const workspace = typeof b.workspace === 'string' ? b.workspace.trim() : ''
  if (!address || !provider || !workspace) return null
  return { address, provider, workspace }
}

/** Open Settings → Email Link with the calendar Connect form filled in. */
export function requestCalendarLink(req: CalendarLinkRequest): void {
  useCalendarLinkRequestStore.getState().request(req)
  useSettingsStore.getState().openSettings('email-link')
}
