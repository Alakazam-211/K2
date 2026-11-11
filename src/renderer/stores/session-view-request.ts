// A one-shot "open this workspace's Chat tab in this view" ask, for an
// opener that knows which view a person should land in (Kessel at first run
// opens in the Thread, whatever this client's "New agents open in" says).
//
// The view memory is keyed by conversation, and a new agent's conversation
// id isn't known until its chat is ready, so the ask is keyed by the
// workspace (`projects.id`) on the window's server. The pinned Chat pane
// takes it once it knows its conversation (`useSessionViewTab`), shows the
// view and remembers it like a pick the person made. An ask nobody takes
// goes stale after `SESSION_VIEW_REQUEST_TTL_MS`.

import { create } from 'zustand'
import type { SessionViewTab } from '@/components/SessionView/sessionViewTab'

/** Long enough for a brand-new agent's first chat to start. */
export const SESSION_VIEW_REQUEST_TTL_MS = 120_000

export interface SessionViewRequest {
  projectId: string
  view: SessionViewTab
  at: number
}

interface SessionViewRequestState {
  pending: SessionViewRequest | null
  request: (projectId: string, view: SessionViewTab) => void
  /** Drop the ask for `projectId` (the pane took it, or it went stale). */
  clear: (projectId: string) => void
}

export const useSessionViewRequestStore = create<SessionViewRequestState>((set, get) => ({
  pending: null,
  request: (projectId, view) => set({ pending: { projectId, view, at: Date.now() } }),
  clear: (projectId) => {
    if (get().pending?.projectId === projectId) set({ pending: null })
  },
}))

/** Open `projectId`'s pinned Chat in `view` the next time it shows. */
export function requestSessionView(projectId: string, view: SessionViewTab): void {
  useSessionViewRequestStore.getState().request(projectId, view)
}

/** The live ask for `projectId`, or null (none, another workspace's, stale). */
export function sessionViewRequestFor(
  pending: SessionViewRequest | null,
  projectId: string | null | undefined,
  now: number = Date.now(),
): SessionViewRequest | null {
  if (!pending || !projectId || pending.projectId !== projectId) return null
  if (now - pending.at > SESSION_VIEW_REQUEST_TTL_MS) return null
  return pending
}
