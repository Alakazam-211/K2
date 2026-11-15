// DM calls on Home (prd-home-people-channels-v1 §11a.3, §11a.4): what each
// server's daemon said about this person's calls, from its Home chat events
// socket (`stores/home-chat.ts` feeds the frames in).
//
// - `rings`: calls ringing for this person now, per server. A ring stops on
//   ANY of Sew's reasons (accepted, declined, timeout, call_ended, cancelled):
//   the daemon sends `call_ring {ringing: false}` to every window's socket at
//   once, so answering in one window stops the card in all of them.
// - `calls`: live DM calls (`call_updated`), per server and conversation: the
//   caller's "Calling…" reads the other person's ring state here.
// - `ended`: the last way a call ended per conversation, for a moment, so the
//   caller sees "Declined" or "No answer" and can call again.
//
// The daemon owns the truth; this is one window's copy. `hello` on each
// (re)connect replaces a server's rings, so a window opened mid-ring shows it.

import { create } from 'zustand'
import type { HomeChatCall, HomeChatRing } from '@/lib/home-chat/calls'
import type { HomeChatFrame } from '@/lib/home-chat/events-socket'

/** How long "Declined" / "No answer" stays on the caller's bar. */
export const CALL_ENDED_SHOW_MS = 8_000

export interface CallEnded {
  callId: string
  reason: string | null
  at: number
}

interface HomeChatCallsStore {
  /** `<host>|<callId>` → a ring for this person. */
  rings: Record<string, HomeChatRing & { hostKey: string }>
  /** `<host>|<conv>` → the DM's live call. */
  calls: Record<string, HomeChatCall>
  /** `<host>|<conv>` → how its last call ended (for a moment). */
  ended: Record<string, CallEnded>
}

export const useHomeChatCallsStore = create<HomeChatCallsStore>(() => ({ rings: {}, calls: {}, ended: {} }))

const key = (hostKey: string, id: string): string => `${hostKey}|${id}`

/** What one frame does to the calls (exported for tests). */
export function applyCallFrame(hostKey: string, f: HomeChatFrame, now: number): void {
  const s = useHomeChatCallsStore.getState()
  switch (f.type) {
    case 'hello': {
      const rings = Object.fromEntries(Object.entries(s.rings).filter(([, r]) => r.hostKey !== hostKey))
      for (const r of f.rings ?? []) rings[key(hostKey, r.callId)] = { ...r, hostKey }
      useHomeChatCallsStore.setState({ rings })
      break
    }
    case 'call_ring': {
      const k = key(hostKey, f.ring.callId)
      if (f.ring.ringing) {
        useHomeChatCallsStore.setState({ rings: { ...s.rings, [k]: { ...f.ring, hostKey } } })
      } else if (s.rings[k]) {
        const rings = { ...s.rings }
        delete rings[k]
        useHomeChatCallsStore.setState({ rings })
      }
      break
    }
    case 'call_updated': {
      const k = key(hostKey, f.conv)
      const calls = { ...s.calls }
      const ended = { ...s.ended }
      const rings = { ...s.rings }
      if (f.call.ended) {
        if (calls[k]?.callId === f.call.callId || !calls[k]) delete calls[k]
        delete rings[key(hostKey, f.call.callId)]
        ended[k] = { callId: f.call.callId, reason: f.call.endReason, at: now }
      } else {
        calls[k] = f.call
        delete ended[k]
      }
      useHomeChatCallsStore.setState({ calls, ended, rings })
      break
    }
    default:
      break
  }
}

/** The rings to show now, oldest first. */
export function ringsNow(rings: HomeChatCallsStore['rings']): Array<HomeChatRing & { hostKey: string }> {
  return Object.values(rings).sort((a, b) => (a.expiresAt ?? 0) - (b.expiresAt ?? 0))
}

/** Tests: forget everything. */
export function resetHomeChatCallsForTests(): void {
  useHomeChatCallsStore.setState({ rings: {}, calls: {}, ended: {} })
}
