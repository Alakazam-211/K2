// Voice & Chat only logins (prd-voice-chat-only-members-v1 §8). A login
// with the `voice_chat` role on a server may join that server's voice rooms
// and nothing else: the daemon refuses every other route with 403
// `role_required`. The client never offers to open such a server: it is
// left out of the switcher, Cmd+L, Home, Add to Home and Clone to, and shows
// up only in the headset's voice room list with a "Voice & Chat only" chip.
//
// The server's whoami is the truth (D4). `ConnectHost.access` is a hint
// written from whoami so the switcher hides the server before the first
// check after a restart (vs-live V18); the gate screen is what holds.
//
// No runtime imports: the gate, the pool and the stores all read this.

/** The role's wire string. */
export const VOICE_CHAT_ROLE = 'voice_chat'

/** `/boot-status` `features`: the server knows the role (Settings offers it
 *  only then). */
export const VOICE_CHAT_ROLE_FEATURE = 'voice-chat-role-v1'

/** The chip's text. */
export const VOICE_CHAT_CHIP = 'Voice & Chat only'

/** The line under the server in the headset list, and the chip's tooltip. */
export const VOICE_CHAT_LINE =
  "Voice & Chat only. You can join this server's voice rooms. You can't open the server or talk to its agents."

/** The line under Settings → Users and Access's role picker. */
export const VOICE_CHAT_ROLE_HELP =
  "Voice & Chat only: joins voice rooms. Can't open this server, see workspaces or talk to agents."

/** Whether a whoami role is Voice & Chat only. */
export function isVoiceChatOnly(role: string | null | undefined): boolean {
  return role === VOICE_CHAT_ROLE
}

/** The non-secret hint a saved host carries. */
export type HostAccess = typeof VOICE_CHAT_ROLE

/** The hint for a whoami role: `voice_chat`, or none. */
export function accessForRole(role: string | null | undefined): HostAccess | undefined {
  return isVoiceChatOnly(role) ? VOICE_CHAT_ROLE : undefined
}

/** Whether a saved host is (by its hint) Voice & Chat only. */
export function hostIsVoiceChatOnly(host: { access?: string } | null | undefined): boolean {
  return host?.access === VOICE_CHAT_ROLE
}

/** The saved hosts this client may switch to: every host whose hint isn't
 *  Voice & Chat only. Order kept. */
export function openableHosts<T extends { access?: string }>(hosts: readonly T[]): T[] {
  return hosts.filter((h) => !hostIsVoiceChatOnly(h))
}

/** A daemon's refusal of a route for this login's role (any role, not only
 *  voice_chat): 403 `{"error":"role_required", …}`. Never a dead login. */
export function isRoleRequired(status: number, body: string): boolean {
  if (status !== 403) return false
  try {
    const v = JSON.parse(body) as { error?: unknown }
    return v?.error === 'role_required'
  } catch {
    return false
  }
}

/** The gate screen's copy (§8.3). The web client can't carry a call. */
export function voiceChatGateText(server: string, web: boolean): string {
  return web
    ? `${server} gives you Voice & Chat only. You can't open this server or talk to its agents. Use the K2 desktop app to join its voice rooms.`
    : `${server} gives you Voice & Chat only. Join its rooms from the headset. You can't open this server or talk to its agents.`
}

/** What the add-server flow says when the new server is Voice & Chat only (§8.4). */
export function voiceChatAddedText(server: string): string {
  return `Added ${server}. It gives you Voice & Chat only: its rooms are in the headset.`
}
