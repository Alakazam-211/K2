import { createContext, useContext, type ReactNode } from 'react'

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
  /** The workspace the Thread's printed paths and files resolve in, when
   *  the Thread's address isn't a workspace's (`ticket:<id>`). */
  fileWorkspace?: string | null
}

export const ThreadComposeExtrasContext = createContext<ThreadComposeExtras | null>(null)

export function useThreadComposeExtras(): ThreadComposeExtras | null {
  return useContext(ThreadComposeExtrasContext)
}
