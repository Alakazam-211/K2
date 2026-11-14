// "Most recent at the top" for people and channels: the person's OWN sends
// in Home chat stamp the row's `lastMessagedAt` (synced), at most once per
// row per 30 s with one trailing stamp (`home-row-stamp.ts`, the agent rows'
// rule). A message someone else sent never stamps a row.

import { createSendStamper } from '@/lib/home-row-stamp'
import { useHomesStore } from '@/stores/homes'
import type { HomeChatTarget } from './address'

const stamper = createSendStamper({
  stamp: (address, at) => void useHomesStore.getState().stampRowsMessaged(address, at),
  now: () => Date.now(),
  schedule: (fn, ms) => setTimeout(fn, ms),
})

/** The person sent in `target`'s chat: stamp its rows. */
export function noteHomeChatSend(target: HomeChatTarget): void {
  stamper(target.hostKey, target.kind === 'person' ? target.ext : target.conv)
}
