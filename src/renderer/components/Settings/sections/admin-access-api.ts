// Settings → Admin Access: the daemon routes behind the people list.
// Moved out of K2ConnectSection (the Tunnel page keeps its own tunnel
// calls). Users, policy and whoami talk to the server the window is on
// (`getDaemonWs(primaryScope())`), with the same stale-token retry as
// before: a 403 re-reads the creds once, and a dead pooled socket on a
// remote server retries through withRemoteRetry.
//
//   GET  /cli/auth/whoami          who is looking (role, owner token, username)
//   GET  /cli/users                the people who can sign in
//   POST /cli/users/add | remove | set-role | set-disabled | set-password |
//        set-can-clone | set-shares
//   GET/POST /cli/users/policy     the password rules
//   GET  /cli/tunnel/config        (this computer only) the address to give a new person

import { getDaemonWs, getLocalDaemonWs, daemonHttpBase, invalidateDaemonWs } from '@/kessel/daemon-ws'
import { withRemoteRetry } from '@/lib/remote-retry'
import { primaryScope } from '@/kessel/server-scope'

async function send(path: string, init: RequestInit): Promise<Response> {
  return withRemoteRetry(async () => {
    const once = async (): Promise<Response> => {
      const creds = await getDaemonWs(primaryScope())
      const sep = path.includes('?') ? '&' : '?'
      return fetch(`${daemonHttpBase(creds)}${path}${sep}token=${creds.token}`, init)
    }
    const res = await once()
    if (res.status !== 403) return res
    invalidateDaemonWs()
    return once()
  }, { onRetry: invalidateDaemonWs })
}

/** GET /cli/users<suffix>. */
export function userGet(suffix: string): Promise<Response> {
  return send(`/cli/users${suffix}`, { method: 'GET' })
}

/** POST /cli/users<suffix> with a JSON body. */
export function userPost(suffix: string, body?: unknown): Promise<Response> {
  return send(`/cli/users${suffix}`, {
    method: 'POST',
    headers: body !== undefined ? { 'Content-Type': 'application/json' } : undefined,
    body: body !== undefined ? JSON.stringify(body) : undefined,
  })
}

/** GET /cli/auth/whoami on the window's server. */
export function whoamiGet(): Promise<Response> {
  return send('/cli/auth/whoami', { method: 'GET' })
}

/** 0.46.8: whether a users/add or users/set-password answer says the
 *  password is temporary. An older daemon may not answer the field; then
 *  `fallback` (what this client asked for) stands. */
export async function answeredTemporary(res: Response, fallback: boolean): Promise<boolean> {
  const text = await res.text()
  try {
    const parsed = JSON.parse(text) as { mustChangePassword?: unknown }
    if (typeof parsed.mustChangePassword === 'boolean') return parsed.mustChangePassword
  } catch {
    /* an empty or non-JSON 200 from an older daemon */
  }
  return fallback
}

/** A failed answer: the daemon's code when it sends one with a sentence
 *  (`{ error: "role_change_not_allowed", message: "…" }`), and the text to
 *  show as its reason: the `message`, else the `error`, else the raw body. */
export async function errReply(res: Response): Promise<{ code: string | null; text: string }> {
  const text = await res.text()
  try {
    const parsed = JSON.parse(text)
    const code = parsed && typeof parsed.error === 'string' && /^[a-z_]+$/.test(parsed.error) ? parsed.error : null
    if (parsed && typeof parsed.message === 'string' && parsed.message.trim()) return { code, text: parsed.message }
    if (parsed && typeof parsed.error === 'string') return { code, text: parsed.error }
  } catch {
    /* raw text */
  }
  return { code: null, text: text || `request failed (${res.status})` }
}

/** Why a write failed, for a person (see `errReply`). */
export async function errText(res: Response): Promise<string> {
  return (await errReply(res)).text
}

/** This computer's internet address from its tunnel settings, or null
 *  (no tunnel set up, or the read failed). Local only: it is what a new
 *  person types to reach the server this computer runs. */
export async function localServerAddress(): Promise<string | null> {
  try {
    const creds = await getLocalDaemonWs()
    const res = await fetch(`${daemonHttpBase(creds)}/cli/tunnel/config?token=${creds.token}`, { method: 'GET' })
    if (!res.ok) return null
    const cfg = (await res.json()) as { publicUrl?: unknown }
    return typeof cfg.publicUrl === 'string' && cfg.publicUrl.trim() ? cfg.publicUrl.trim() : null
  } catch {
    return null
  }
}

/** The password rules (GET/POST /cli/users/policy). */
export interface PasswordPolicy {
  minLength: number
  requireSpecial: boolean
  requireNumber: boolean
  requireUppercase: boolean
}

export const DEFAULT_POLICY: PasswordPolicy = { minLength: 8, requireSpecial: false, requireNumber: false, requireUppercase: false }

/** A policy from the daemon's answer, with the defaults for missing fields. */
export function parsePolicy(raw: unknown): PasswordPolicy {
  const p = raw && typeof raw === 'object' ? (raw as Record<string, unknown>) : {}
  return {
    minLength: typeof p.minLength === 'number' && Number.isFinite(p.minLength) ? p.minLength : DEFAULT_POLICY.minLength,
    requireSpecial: p.requireSpecial === true,
    requireNumber: p.requireNumber === true,
    requireUppercase: p.requireUppercase === true,
  }
}

const LOWER = 'abcdefghijkmnpqrstuvwxyz'
const UPPER = 'ABCDEFGHJKLMNPQRSTUVWXYZ'
const DIGIT = '23456789'
const SPECIAL = '!#%+=?@'

function pick(set: string, rand: (n: number) => number): string {
  return set[rand(set.length)]
}

/** A password K2 makes for someone: at least 16 characters (or the rule's
 *  minimum), with a lowercase and uppercase letter, a number and a special
 *  character, so it passes every rule a server can set. No look-alike
 *  characters (l, 1, O, 0), since a person may read it out. `rand(n)` is
 *  a uniform integer below n (tests pass their own). */
export function makePassword(policy: Pick<PasswordPolicy, 'minLength'>, rand: (n: number) => number = cryptoRand): string {
  const length = Math.max(16, Math.min(128, policy.minLength))
  const all = LOWER + UPPER + DIGIT + SPECIAL
  const chars = [pick(LOWER, rand), pick(UPPER, rand), pick(DIGIT, rand), pick(SPECIAL, rand)]
  while (chars.length < length) chars.push(pick(all, rand))
  for (let i = chars.length - 1; i > 0; i--) {
    const j = rand(i + 1)
    ;[chars[i], chars[j]] = [chars[j], chars[i]]
  }
  return chars.join('')
}

function cryptoRand(n: number): number {
  // Rejection sampling, so every character is equally likely.
  const limit = Math.floor(0x100000000 / n) * n
  const buf = new Uint32Array(1)
  for (;;) {
    crypto.getRandomValues(buf)
    if (buf[0] < limit) return buf[0] % n
  }
}

/** Why a typed password breaks the rules, in plain words, or null. */
export function passwordProblem(pw: string, policy: PasswordPolicy): string | null {
  if (pw.length < policy.minLength) return `Use at least ${policy.minLength} characters.`
  if (policy.requireNumber && !/\d/.test(pw)) return 'Add a number.'
  if (policy.requireUppercase && !/[A-Z]/.test(pw)) return 'Add an uppercase letter.'
  if (policy.requireSpecial && !/[^A-Za-z0-9]/.test(pw)) return 'Add a special character, such as ! or #.'
  return null
}

/** Why a username can't be used, in plain words, or null. Mirrors the
 *  daemon's rule (connect_users::normalize_username + reserved names); the
 *  daemon still decides. */
export function usernameProblem(raw: string): string | null {
  const name = raw.trim().toLowerCase()
  if (!name) return null
  if (!/^[a-z0-9_-]+$/.test(name)) return 'Use only letters, numbers, - and _.'
  if (name.length < 2) return 'Use at least 2 characters.'
  if (name === 'owner' || name === 'agent') return `“${name}” is kept for K2 itself. Pick another.`
  return null
}

/** The password rules in a sentence, for the Type one choice. */
export function policyWords(policy: PasswordPolicy): string {
  const extra = [
    policy.requireNumber ? 'a number' : null,
    policy.requireUppercase ? 'an uppercase letter' : null,
    policy.requireSpecial ? 'a special character' : null,
  ].filter(Boolean)
  return `At least ${policy.minLength} characters${extra.length ? `, with ${extra.join(', ')}` : ''}.`
}
