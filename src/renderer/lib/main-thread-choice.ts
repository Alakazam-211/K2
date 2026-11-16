import { daemonCliGet } from '@/lib/daemon-cli'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
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

/** Longest name the title shows whole; a longer one is cut with "…" and
 *  the whole title shows on hover. The answers always say the whole name. */
const TITLE_NAME_MAX = 32

/** Ask, on the shared choose dialog, which Thread the main chat keeps when
 *  `name` (the picked chat's display name) becomes the main chat. Resolves
 *  the choice, or null for Cancel (Esc, the ×, a press outside): nothing
 *  changes. Keeping the main chat's Thread has the focus, so Enter picks it.
 *  Neither answer deletes anything: the Thread not picked is kept under a
 *  new address of its own (`<agent>/<n>`). */
export async function askMainThreadChoice(name: string): Promise<MainThreadChoice | null> {
  const full = name.trim() || 'this chat'
  const short = full.length > TITLE_NAME_MAX ? `${full.slice(0, TITLE_NAME_MAX - 1).trimEnd()}…` : full
  const id = await useConfirmDialogStore.getState().choose({
    title: `Make ${short} the main chat?`,
    titleTooltip: short === full ? undefined : `Make ${full} the main chat?`,
    message: 'Choose which Thread the main chat keeps.',
    choices: [
      {
        id: 'keep_main',
        label: "Keep the main chat's Thread",
        hint: `${full}'s Thread is kept on its own, under a new address.`,
        safeDefault: true,
      },
      {
        id: 'adopt_sidecar',
        label: `Use ${full}'s Thread`,
        hint: "The main chat's Thread is kept on its own, under a new address.",
      },
    ],
  })
  return id === 'keep_main' || id === 'adopt_sidecar' ? id : null
}
