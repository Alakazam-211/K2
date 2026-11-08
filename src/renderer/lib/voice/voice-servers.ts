// Voice rooms §4.3 / §5 (lane D, S5): which servers the headset list shows.
//
// The list is per-person view state gathered by this client, like Home
// (`stores/homes.ts`): one section per server in THIS client's server list
// where the person is signed in with their own login and the server reports
// `voice-rooms-v1`. No daemon gathers other servers' rooms; federation plays
// no part. Pure: the store feeds it Home's pool entries.

import type { HostEntry } from '@/lib/host-pool'
import { roleAllowsRoom } from '@/lib/host-pool'
import { isVoiceChatOnly } from '@/lib/voice-chat-only'
import { canonicalHostKey } from '@/lib/host-key'
import { VOICE_ROOMS_FEATURE } from './voice-wire'

export type VoiceServerKind =
  /** Signed in, live, reports rooms: list and join. */
  | 'live'
  /** Reported rooms the last time this client was signed in, but the
   *  login is gone now (§4.3): "Sign in to X to see its rooms". */
  | 'signed-out'
  /** Reported rooms before, unreachable now (§4.6): greyed, last seen. */
  | 'offline'

export interface VoiceServer {
  hostKey: string
  label: string
  /** The window's own server: listed first as "This server". */
  isWindow: boolean
  kind: VoiceServerKind
  /** When this client last read the server's `/boot-status` (offline rows). */
  lastSeenAt: number | null
  role: string | null
  /** The pool's sign-in note (kicked, refused password…), for signed-out rows. */
  authNote: string | null
  /** This login is Voice & Chat only there (prd-voice-chat-only-members-v1
   *  §8.2): listed and joinable, with the chip; the server never opens. */
  voiceChatOnly: boolean
}

export interface VoiceServerInputs {
  /** Every server key this client knows: the window's, this computer's
   *  (desktop only), and each saved server. */
  keys: readonly string[]
  windowKey: string
  entries: Readonly<Record<string, HostEntry | undefined>>
  /** Display name for a key ("This computer", the saved label). */
  labelOf: (hostKey: string) => string
  /** Servers that listed rooms the last time this client was signed in. */
  reportedBefore: ReadonlySet<string>
}

export function reportsVoice(entry: HostEntry | undefined): boolean {
  return entry?.boot?.features.includes(VOICE_ROOMS_FEATURE) ?? false
}

/** Signed in with a usable role. The window's own server (and this
 *  computer's daemon) is signed in by the window itself: the pool only
 *  reads its boot status there (`bootOnly`). Voice & Chat only counts: the
 *  headset is the one place such a server shows (§8.2). */
function signedIn(entry: HostEntry | undefined, isWindow: boolean, isLocal: boolean): boolean {
  if (isWindow || isLocal) return true
  return entry?.auth === 'ok' && (roleAllowsRoom(entry.role) || isVoiceChatOnly(entry.role))
}

/** The sections, "This server" first, then the rest A to Z by name. */
export function voiceServers(input: VoiceServerInputs): VoiceServer[] {
  const out: VoiceServer[] = []
  const seen = new Set<string>()
  const windowKey = safeKey(input.windowKey)
  for (const raw of input.keys) {
    const key = safeKey(raw)
    if (key === null || seen.has(key)) continue
    seen.add(key)
    const entry = input.entries[raw] ?? input.entries[key]
    const isWindow = key === windowKey
    const isLocal = key === 'local'
    const base = {
      hostKey: raw,
      label: input.labelOf(raw),
      isWindow,
      lastSeenAt: entry?.boot?.at ?? null,
      role: entry?.role ?? null,
      authNote: entry?.authNote ?? null,
      voiceChatOnly: isVoiceChatOnly(entry?.role),
    }
    const before = input.reportedBefore.has(key)
    if (entry?.reach === 'offline') {
      if (reportsVoice(entry) || before) out.push({ ...base, kind: 'offline' })
      continue
    }
    if (!signedIn(entry, isWindow, isLocal)) {
      // Kicked, signed out, refused, or a role below Member: only a server
      // that had rooms before gets a row (§4.3).
      if (before && entry?.auth !== 'ok') out.push({ ...base, kind: 'signed-out' })
      continue
    }
    if (reportsVoice(entry)) out.push({ ...base, kind: 'live' })
  }
  return out.sort((a, b) => {
    if (a.isWindow !== b.isWindow) return a.isWindow ? -1 : 1
    return a.label.localeCompare(b.label, undefined, { sensitivity: 'base' }) || a.hostKey.localeCompare(b.hostKey)
  })
}

/** §4.1: the headset shows only when a signed-in server reports rooms. */
export function anyVoiceServer(servers: readonly VoiceServer[]): boolean {
  return servers.some((s) => s.kind === 'live')
}

function safeKey(k: string): string | null {
  try {
    return canonicalHostKey(k)
  } catch {
    return null
  }
}

/** "Offline · last seen 3 min ago" (§4.6). */
export function lastSeenText(at: number | null, now: number): string {
  if (at === null) return 'Offline'
  const min = Math.floor((now - at) / 60_000)
  if (min < 1) return 'Offline · last seen just now'
  if (min < 60) return `Offline · last seen ${min} min ago`
  const h = Math.floor(min / 60)
  return h < 24 ? `Offline · last seen ${h} h ago` : `Offline · last seen ${Math.floor(h / 24)} d ago`
}
