// prd-zen-mode-v1 Z42 and prd-zen-gardens-v1 G28, G60 — the message-box
// drafts of Zen conversations (this window only), and the door the bridge
// verb `compose.draft(address, text)` uses to fill one.
//
// A draft set while the conversation is on screen shows at once (the box
// subscribes); one set before it mounts is read when it does. Setting a
// draft never sends anything.

const drafts = new Map<string, string>()
const listeners = new Map<string, Set<(text: string) => void>>()

/** The draft for `address` ('' when none). */
export function zenComposeDraft(address: string): string {
  return drafts.get(address) ?? ''
}

/** The box's own typing: remember, tell nobody. */
export function keepZenComposeDraft(address: string, text: string): void {
  if (text) drafts.set(address, text)
  else drafts.delete(address)
}

/** `compose.draft`: set the box's text from outside, and show it now. */
export function draftZenCompose(address: string, text: string): void {
  keepZenComposeDraft(address, text)
  for (const fn of [...(listeners.get(address) ?? [])]) fn(text)
}

/** The box listens for drafts set from outside. Returns the unsubscribe. */
export function onZenComposeDraft(address: string, fn: (text: string) => void): () => void {
  let set = listeners.get(address)
  if (!set) {
    set = new Set()
    listeners.set(address, set)
  }
  set.add(fn)
  return () => {
    const s = listeners.get(address)
    s?.delete(fn)
    if (s && s.size === 0) listeners.delete(address)
  }
}

/** Tests only. */
export function __resetZenComposeDraftsForTests(): void {
  drafts.clear()
  listeners.clear()
}
