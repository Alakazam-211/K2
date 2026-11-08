// Connect login roles for Settings → Server Access → Users.
//
// prd-remove-viewer-role-v1.md: the Viewer role is gone. A login is a
// trusted operator — Member, Admin or Owner. Limited access is what app
// passes are for. Accounts that were Viewers were disabled when the
// daemon updated and come back from GET /cli/users with
// `wasViewer: true`; the owner turns one back on with "Enable as Member"
// (POST /cli/users/set-disabled {disabled:false}), which also clears the
// flag. Nobody is upgraded without that click.

//
// prd-voice-chat-only-members-v1 (0.46.10): `voice_chat` ("Voice & Chat
// only") sits below Member: the login may join the server's voice rooms
// and, with the owner's shares, read and post one agent's Thread and see
// one Garden. Nothing else. Offered only by a server that reports
// `voice-chat-role-v1`.

/** The login roles a daemon accepts. There is no `viewer`. */
export type K2Role = 'owner' | 'admin' | 'member' | 'voice_chat'

/** The role's label in pickers and lists. */
export const VOICE_CHAT_LABEL = 'Voice & Chat only'

/** Re-review M1: shown while a Voice & Chat only login has an agent shared
 * (the daemon's `notice`). Each login reads only its own conversation, but
 * they all reach the agent's one session. */
export const SHARED_CONTEXT_NOTICE =
  "Share only a dedicated demo agent. Its memory is shared: every Voice & Chat only login and the agent's own Thread talk to the same session, so don't use its Thread for your own work, and give each reviewer their own login."

/** Role picker options — lowest first. No Viewer option. Voice & Chat only
 *  isn't here: {@link roleOptions} adds it for a server that knows it. */
export const ROLE_OPTIONS: ReadonlyArray<{ value: K2Role; label: string }> = [
  { value: 'member', label: 'Member' },
  { value: 'admin', label: 'Admin' },
  { value: 'owner', label: 'Owner' },
]

/** The picker's options for a server: Voice & Chat only first (lowest)
 *  only when the server reports `voice-chat-role-v1`. */
export function roleOptions(voiceChatRole: boolean): ReadonlyArray<{ value: K2Role; label: string }> {
  return voiceChatRole ? [{ value: 'voice_chat', label: VOICE_CHAT_LABEL }, ...ROLE_OPTIONS] : ROLE_OPTIONS
}

/** Parse a wire role. The retired `viewer` (an older server can still
 *  send it) and anything unknown are `null`, never coerced to Member. */
export function parseK2Role(raw: unknown): K2Role | null {
  return raw === 'owner' || raw === 'admin' || raw === 'member' || raw === 'voice_chat' ? raw : null
}

/** Whether `actor` may move `target` from `from` to `to` (§5.3, Q1): the
 *  Owner any change; an Admin only between Member and Voice & Chat only
 *  (both below Admin). The daemon checks the same rule. */
export function canSetRole(actor: K2Role | null, from: K2Role | null, to: K2Role): boolean {
  if (actor === 'owner') return true
  if (actor !== 'admin') return false
  const low = (r: K2Role | null): boolean => r === 'member' || r === 'voice_chat'
  return low(from) && low(to)
}

/** What a Voice & Chat only login is shown (`GET /cli/users` `shares`). */
export interface VoiceChatShares {
  agents: Array<{ id?: string; address: string; name?: string }>
  gardens: Array<{ id: string; name?: string }>
}

/** `shares` from a users row, keeping only well-formed entries. Absent
 *  (an older server, or not Voice & Chat only) → empty lists. */
export function parseShares(raw: unknown): VoiceChatShares {
  const out: VoiceChatShares = { agents: [], gardens: [] }
  if (!raw || typeof raw !== 'object') return out
  const r = raw as { agents?: unknown; gardens?: unknown }
  if (Array.isArray(r.agents)) {
    for (const a of r.agents) {
      if (!a || typeof a !== 'object') continue
      const { id, address, name } = a as Record<string, unknown>
      if (typeof address !== 'string' || address.length === 0) continue
      out.agents.push({
        address,
        ...(typeof id === 'string' ? { id } : {}),
        ...(typeof name === 'string' ? { name } : {}),
      })
    }
  }
  if (Array.isArray(r.gardens)) {
    for (const g of r.gardens) {
      if (!g || typeof g !== 'object') continue
      const { id, name } = g as Record<string, unknown>
      if (typeof id !== 'string' || id.length === 0) continue
      out.gardens.push({ id, ...(typeof name === 'string' ? { name } : {}) })
    }
  }
  return out
}

/** A comma/space separated list typed in the shares editor → trimmed,
 *  de-duplicated entries. */
export function splitShareList(text: string): string[] {
  const seen = new Set<string>()
  const out: string[] = []
  for (const part of text.split(/[\s,]+/)) {
    const v = part.trim()
    if (v.length === 0 || seen.has(v)) continue
    seen.add(v)
    out.push(v)
  }
  return out
}

/** Re-review L4: the "Temporary password" box starts off for a Voice &
 *  Chat only login (the daemon makes its password fixed and kept unless
 *  told otherwise) and on for everyone else (0.46.8). */
export function temporaryDefaultFor(role: K2Role): boolean {
  return role !== 'voice_chat'
}

/** The `users/add` body (§5.3, V5): the role rides the add itself, so a
 *  Voice & Chat only login is never a Member, not even for one write. */
export function addUserBody(input: {
  username: string
  password: string
  mustChangePassword: boolean
  role: K2Role
  firstName?: string
  lastName?: string
  shareAgents?: string[]
  shareGardens?: string[]
}): Record<string, unknown> {
  const body: Record<string, unknown> = {
    username: input.username,
    password: input.password,
    mustChangePassword: input.mustChangePassword,
  }
  if (input.firstName) body.firstName = input.firstName
  if (input.lastName) body.lastName = input.lastName
  if (input.role === 'voice_chat') {
    body.role = 'voice_chat'
    if (input.shareAgents && input.shareAgents.length > 0) body.shareAgents = input.shareAgents
    if (input.shareGardens && input.shareGardens.length > 0) body.shareGardens = input.shareGardens
  }
  return body
}

/** One row of GET /cli/users. `role` stays a string: an older server may
 *  still report `viewer`, which the UI shows as-is rather than guessing. */
export interface K2User {
  /** The login's stable, never-reused id (Sew `k2:connect:<id>`; a chat
   *  grant's subjectId). Absent from a daemon that doesn't send it yet. */
  id?: string
  username: string
  createdAt?: string | null
  disabled: boolean
  role?: string
  /** True for an account disabled by the Viewer removal. */
  wasViewer?: boolean
  /** Settings → Admin Access → "Can clone" (0.46.0). Owner rows are always
   *  true; absent from an older daemon (= allowed, today's behaviour). */
  canClone?: boolean
  /** 0.46.0: optional real name (display only; null/absent when unset or
   *  from an older daemon). */
  firstName?: string | null
  lastName?: string | null
  displayName?: string
  /** 0.46.0: the colour this login chose (`#rrggbb`), absent for
   *  Automatic or from an older daemon. */
  color?: string | null
  /** 0.46.8: the password is temporary (someone else set it; K2 asks the
   *  person to choose their own at the next sign-in). Absent from an
   *  older daemon. */
  mustChangePassword?: boolean
  /** 0.46.10: what a Voice & Chat only login is shown (empty lists for
   *  everyone else; absent from an older daemon). Read with parseShares. */
  shares?: unknown
}

/** Whether the row should offer "Enable as Member": a former Viewer that
 *  is still disabled. Once enabled the daemon clears `wasViewer`. */
export function needsEnableAsMember(u: Pick<K2User, 'wasViewer' | 'disabled'>): boolean {
  return u.wasViewer === true && u.disabled
}

/** Text under a former Viewer's name in the users list. */
export const WAS_VIEWER_NOTE =
  'Was a Viewer. The Viewer role was removed, so this login was turned off. Enable it as a Member to let them sign in again.'
