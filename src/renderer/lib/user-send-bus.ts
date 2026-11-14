// "The person just sent something to an agent": a Thread post (the Thread
// composer, the Agents compose bar and Zen's compose all post through
// `thread/post`) or a card answer (`thread/answer`). The `/cli` chokepoint
// (`daemonCliPost`) notes each one that the server accepted; Home groups
// stamp the agent's rows with it ("Most recent at the top",
// `lib/home-row-stamp.ts`).
//
// Only this window's own POSTs land here. An agent's message to the person
// arrives over the Thread socket or a GET and never passes through, so it
// never counts as a send.

type Listener = (hostKey: string, addr: string) => void

/** The routes that are the person sending to an agent. */
export const USER_SEND_ROUTES: ReadonlySet<string> = new Set(['thread/post', 'thread/answer'])

const listeners = new Set<Listener>()

export function onUserSend(listener: Listener): () => void {
  listeners.add(listener)
  return () => listeners.delete(listener)
}

/** Called by `daemonCliPost` after a POST the server accepted. A listener
 *  that throws never breaks the send. */
export function noteUserPost(route: string, hostKey: string, body: unknown): void {
  if (!USER_SEND_ROUTES.has(route)) return
  const addr = body && typeof body === 'object' ? (body as { addr?: unknown }).addr : undefined
  if (typeof addr !== 'string' || !addr.trim()) return
  for (const l of [...listeners]) {
    try {
      l(hostKey, addr)
    } catch (err) {
      console.error('[user-send] listener failed:', err)
    }
  }
}
