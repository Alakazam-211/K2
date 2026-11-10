// The Thread's message box shows before its session is ready (the pinned
// Chat's ensure, a side chat's spawn). When the session comes up, the box
// can be drawn by a different tree (the pinned Chat's starting body gives
// way to its terminal pane), so the old box unmounts and a new one mounts
// in the same React commit. A box that had the caret leaves it here (in its
// layout cleanup); the new box with the same draft takes it in its layout
// effect, later in that same commit, before paint: its text, caret and "/"
// command, so typing goes on without a pause.
//
// A handoff lives for that one commit only: it is dropped on the next
// microtask, so a box unmounted with the caret (a closed tab, a test's
// cleanup) never hands its text or caret to a box mounted later.

import type { ComposeSlashCommand } from './terminalCompose'

export interface ComposeHandoff {
  text: string
  start: number
  end: number
  command: ComposeSlashCommand | null
}

const handoffs = new Map<string, ComposeHandoff>()

export function leaveComposeHandoff(key: string, h: ComposeHandoff): void {
  if (!key) return
  handoffs.set(key, h)
  queueMicrotask(() => {
    if (handoffs.get(key) === h) handoffs.delete(key)
  })
}

/** The handoff left for this draft in this commit, if any (taken once). */
export function takeComposeHandoff(key: string): ComposeHandoff | null {
  if (!key) return null
  const h = handoffs.get(key) ?? null
  if (h) handoffs.delete(key)
  return h
}
