import { daemonCliGet } from '@/lib/daemon-cli'
import { threadItemsFromSnapshot } from '@/components/SessionView/overlayThread'
import type { ServerScope } from '@/kessel/server-scope'

/** Which Thread the main chat shows after another chat (a sidecar, a past
 *  chat) becomes the main chat. The daemon's `thread` on
 *  `workspace/set-chat-session` and `k2 sessions main set --thread`. */
export type MainThreadChoice = 'keep_main' | 'adopt_sidecar'

/** True when `addr` (an address or a conversation id) has a Thread with
 *  at least one message. A chat K2 has no Thread for answers 404: false. */
export async function hasThreadMessages(scope: ServerScope, addr: string): Promise<boolean> {
  const a = addr.trim()
  if (!a) return false
  try {
    const raw = await daemonCliGet<unknown>(scope, 'thread', { addr: a, limit: 1 })
    return threadItemsFromSnapshot(raw).items.length > 0
  } catch {
    return false
  }
}

/** A person picked `pickedId` as the main chat. Ask which Thread to keep
 *  only when both the main chat (`mainAddr`, the workspace's address) and
 *  the picked chat have one; otherwise there is nothing to choose and the
 *  main Thread follows the main chat (`keep_main`). */
export async function needsMainThreadChoice(
  scope: ServerScope,
  mainAddr: string,
  pickedId: string,
): Promise<boolean> {
  const [main, picked] = await Promise.all([
    hasThreadMessages(scope, mainAddr),
    hasThreadMessages(scope, pickedId),
  ])
  return main && picked
}
