import { createContext, useContext, type ReactNode } from 'react'
import type { OverlayThreadItem, ThreadTurn } from './overlayThread'

/** What a Thread that isn't an agent's own chat changes in its session
 *  body (a ticket's discussion, TicketThreadPane). Absent: an agent chat,
 *  as always. */
export interface ThreadComposeExtras {
  /** Drawn over the message box, above its notes (a ticket's quick answers
   *  and its "Replying reopens it" line). */
  above?: ReactNode
  /** The box's hint instead of "Message <agent>". */
  placeholder?: string
  /** False: no "/" commands (the post goes to a conversation, not into a
   *  harness), and no `/model`. */
  slash?: boolean
  /** Redraw a row (a ticket's message moved in from its older store names
   *  its author as text). Must be stable and return the same object for an
   *  unchanged row. */
  mapItem?: (item: OverlayThreadItem) => OverlayThreadItem
  /** The working strip over the box, when the Thread's own activity isn't
   *  the agent's (a ticket's replies reach the agent's own session).
   *  Undefined: the Thread's own. */
  turn?: ThreadTurn | null
  /** Room between the newest message and the box (px), on top of the
   *  list's own: the newest message rests clear of a taller footer band
   *  (a ticket's), which Jump to bottom then sits in. */
  restGapPx?: number
  /** Told the server's whole answer to each post this box makes (a ticket's
   *  carries the ticket's status and whether its agent got the message). */
  onPosted?: (resp: Record<string, unknown>) => void
}

export const ThreadComposeExtrasContext = createContext<ThreadComposeExtras | null>(null)

export function useThreadComposeExtras(): ThreadComposeExtras | null {
  return useContext(ThreadComposeExtrasContext)
}
