// Settings → Logs → Access Audit: each record in plain words.
//
// The daemon writes one terse line per event (`auth-audit.jsonl`): who
// (`owner`, `login:<name>`, `connect:<name>`, `agent:<workspace>[/<session>]`,
// `k2`), what (`event`) and an `outcome` whose shape depends on the event:
//   login            ok | bad_creds | locked | rate_limited | blocked_ingress | bad_attest:<why>
//   vault-handoff    ok send <NAME> to <ws> <id> <status> · ok accept <NAME> from <ws> <id> ·
//                    deny send <NAME> to <ws>: <code> · ok ask <NAME> from <ws> <id> · …
//   vault-carry      ok export <n> [<names>] of workspaces [..] by .. to <fp> (vault <v>) ·
//                    ok import <n> from <vault> into <ws> · ok clone export <n> …
//   permission-*     <request id> <action> <state> via=<via> [note=… | twin=… undo=…]
//   permission-switch  server on|off · agent <handle> on|off
// The list shows a sentence and one plain line built from those fields; the
// raw line stays in the opened record.

import type { AuthAuditEvent } from './access-audit-api'
import { isFailedRecord } from './access-audit-api'

/** Workspace id, handle or name → the name people see. */
export type WorkspaceNamer = (key: string) => string

export const plainWorkspace: WorkspaceNamer = (key) => key

export type AuditWho = {
  /** "alex", "You", "K2", or the agent's workspace name. */
  name: string
  /** True for an agent: the row reads "<name> agent …". */
  agent: boolean
  /** The agent's session, shown only in the opened record. */
  session?: string
}

/** Who did it. The owner is "You" (only the owner can open this page). */
export function auditWho(user: string, ws: WorkspaceNamer = plainWorkspace): AuditWho {
  const u = user.trim()
  if (!u || u === '-') return { name: 'Someone', agent: false }
  if (u === 'owner') return { name: 'You', agent: false }
  if (u === 'k2' || u.startsWith('system:')) return { name: 'K2', agent: false }
  if (u.startsWith('agent:')) {
    const [w, session] = u.slice('agent:'.length).split('/')
    return { name: ws(w ?? ''), agent: true, ...(session ? { session } : {}) }
  }
  if (u.startsWith('shell:')) return { name: `A terminal in ${ws(u.slice('shell:'.length))}`, agent: false }
  for (const p of ['login:', 'connect:', 'handoff:']) if (u.startsWith(p)) return { name: u.slice(p.length), agent: false }
  if (u.startsWith('guest:')) return { name: `${u.slice('guest:'.length)} (an App guest)`, agent: false }
  return { name: u, agent: false }
}

/** The who as one string, for search and the opened record. */
export function auditWhoText(user: string, ws: WorkspaceNamer = plainWorkspace): string {
  const w = auditWho(user, ws)
  return w.agent ? `${w.name} agent` : w.name
}

const SIGN_IN_FAILURES: Record<string, string> = {
  bad_creds: 'couldn’t sign in: wrong username or password',
  locked: 'couldn’t sign in: locked out after too many tries',
  rate_limited: 'couldn’t sign in: too many tries, so K2 is waiting',
  blocked_ingress: 'was blocked: came in by a route this server doesn’t accept',
}

/** Sign-in results as a short value for the opened record. */
const SIGN_IN_RESULT: Record<string, string> = {
  ok: 'Signed in',
  bad_creds: 'Wrong username or password',
  locked: 'Account locked after too many tries',
  rate_limited: 'Too many tries; K2 made them wait',
  blocked_ingress: 'Blocked: this server doesn’t accept sign-ins that way',
}

/** True when the sentence already says everything the outcome does. */
export function isPlainSignIn(ev: AuthAuditEvent): boolean {
  return ev.event === 'login' && (ev.outcome === 'ok' || ev.outcome in SIGN_IN_FAILURES)
}

const PERMISSION_WHAT: Record<string, string> = {
  'permission-request': 'asked for a permission',
  'permission-answer': 'answered a permission request',
  'permission-note': 'added a note to a permission request',
  'permission-undo': 'turned a permission off again',
  'permission-switch': 'changed a permission switch',
}

/** The outcome without a leading `ok`, split into words. */
function words(outcome: string): { ok: boolean; deny: boolean; rest: string; parts: string[] } {
  const o = outcome.trim()
  const ok = o === 'ok' || o.startsWith('ok ')
  const deny = /^(deny|denied|refused)\b/.test(o)
  const rest = ok ? o.slice(2).trim() : deny ? o.replace(/^\S+\s*/, '') : o
  return { ok, deny, rest, parts: rest.split(/\s+/).filter(Boolean) }
}

/** What happened, as the rest of a sentence after the person's name. */
export function auditWhat(ev: AuthAuditEvent): string {
  const failed = isFailedRecord(ev)
  const verb = words(ev.outcome).parts[0] ?? ''
  switch (ev.event) {
    case 'login':
      if (!failed) return 'signed in'
      if (ev.outcome.startsWith('bad_attest')) return 'was blocked: the sign-in couldn’t be verified'
      return SIGN_IN_FAILURES[ev.outcome] ?? 'couldn’t sign in'
    case 'logout':
      return 'signed out'
    case 'change-password':
      return failed ? 'couldn’t change their password' : 'changed their password'
    case 'set-password':
      return failed ? 'couldn’t set a password' : 'set a password'
    case 'add-user':
      return 'added a person'
    case 'vault-handoff':
      if (failed) return 'was refused a secret handoff'
      if (verb === 'send') return 'sent a secret'
      if (verb === 'receive') return 'received a secret'
      if (verb === 'accept') return 'accepted a secret'
      if (verb === 'decline') return 'declined a secret'
      if (verb === 'expire') return 'let a secret handoff expire'
      if (verb === 'ask') return 'asked for a secret'
      if (verb === 'unsend') return 'took back a secret'
      if (verb === 'accept-from') return 'changed who it takes secrets from'
      if (verb === 'send-allow') return 'changed which secrets it may send'
      return 'handed off a secret'
    case 'vault-carry':
      if (failed) return 'was refused a vault copy'
      if (verb === 'export') return 'exported secrets from this server’s vault'
      if (verb === 'import') return 'imported secrets into this server’s vault'
      if (verb === 'clone') return 'copied the vault for a new server'
      return 'moved secrets in the vault'
    default:
      return PERMISSION_WHAT[ev.event] ?? 'did something'
  }
}

// ── Permission cards ─────────────────────────────────────────────────

/** Catalog action → what it lets an agent do (the daemon's own words,
 *  shortened; `crates/k2-daemon/src/permission/actions`). */
const ACTION_WORDS: Record<string, string> = {
  'compute.use': 'run jobs on compute nodes',
  'compute.operate': 'operate compute nodes',
  'compute.manage': 'add and retire compute machines',
  'compute.ci': 'run CI on compute nodes',
  'compute.gpu': 'use GPUs on compute nodes',
  'compute.allow-nodes': 'use more compute machines',
  'compute.packages': 'install packages on compute nodes',
  'compute.packages-policy': 'change the compute package rules',
  'compute.git-access': 'reach git from compute jobs',
  'compute.git-access-setup': 'set up GitHub access for compute jobs',
  'compute.admin-approve': 'run an owner-only step on a compute node',
  'agents.manage': 'hire and manage agents',
  'projects.manage': 'manage K2 projects',
  'apps.build': 'build and run Apps',
  'apps.manage-full': 'manage its Apps end to end',
  'apps.manage-users': 'manage its Apps’ users',
  'apps.core': 'let its Apps use a core',
  'apps.approve': 'get an App approved',
  'chat.manage': 'run Chat / Voice Hosting',
  'vault.manage': 'share server-wide secrets',
  'vault.move-approve': 'move secrets to another server',
  'gardens.manage': 'manage Gardens and data sources',
  'garden.proposal.approve': 'change a Garden',
  'garden.keep-mine': 'keep a Garden as its own',
  'sources.approve': 'add a data source',
  'dns.manage': 'manage DNS records',
  'mail.manage': 'manage hosted mail',
  'mail.send-mode': 'send email',
  'mail.approve': 'send an email that needs approval',
  'db.agent-access': 'create databases',
  'sim.use': 'use mobile devices',
  'organization.manage': 'manage the org chart',
  'connections.create': 'add a connection',
  'connections.add': 'add a connection',
  'docs.share': 'share a document',
  'sew.owner-handoff': 'open Sew as the owner',
  'sew.rollback-data-loss': 'roll Sew back',
}

export function actionWords(action: string): string {
  const base = action.endsWith('-off') ? action.slice(0, -'-off'.length) : action
  const w = ACTION_WORDS[base]
  if (!w) return 'change a setting'
  return base === action ? w : `stop being able to ${w}`
}

const STATE_WORDS: Record<string, string> = {
  pending: 'waiting for an answer',
  applying: 'being applied',
  approved: 'allowed',
  denied: 'denied',
  expired: 'expired before anyone answered',
  stale: 'no longer needed',
  failed: 'allowed, but the change didn’t apply',
  cancelled: 'withdrawn',
  closed: 'closed',
  already_done: 'already on',
}

const VIA_WORDS: Record<string, string> = {
  'owner-token': 'the owner’s own key',
  login: 'their sign-in',
  passport: 'the agent’s own pass',
  daemon: 'K2 itself',
}

export type PermissionLine = { id: string; action: string; state: string; via: string; rest: string }

/** `<request id> <action> <state> via=<via> [more]`; also tolerates a leading `ok`. */
export function parsePermissionLine(outcome: string): PermissionLine | null {
  const m = /^(?:ok\s+)?(\S+)\s+([a-z0-9.-]+)\s+([a-z_]+)\s+via=(\S+)\s*(.*)$/.exec(outcome.trim())
  if (!m) return null
  return { id: m[1]!, action: m[2]!, state: m[3]!, via: m[4]!, rest: m[5] ?? '' }
}

const cap = (s: string): string => (s ? s[0]!.toUpperCase() + s.slice(1) : s)

/** Plain words for a refusal code (`never_sendable` → "this secret can never be sent"). */
function codeWords(code: string): string {
  const c = code.trim()
  const known: Record<string, string> = {
    never_sendable: 'this secret can never be sent',
    not_pending: 'it was already answered',
    forbidden: 'not allowed',
    not_found: 'it no longer exists',
    expired: 'it had expired',
  }
  return known[c] ?? c.replace(/_/g, ' ')
}

/** "server:GITHUB_TOKEN" → "GITHUB_TOKEN"; a list reads "A, B and 3 more". */
function secretList(names: string[]): string {
  const clean = names.map((n) => n.trim().replace(/^[a-z]+:/, '')).filter(Boolean)
  if (clean.length === 0) return ''
  if (clean.length <= 2) return clean.join(' and ')
  return `${clean.slice(0, 2).join(', ')} and ${clean.length - 2} more`
}

const secrets = (n: number): string => (n === 1 ? '1 secret' : `${n} secrets`)

export type DetailContext = {
  ws: WorkspaceNamer
  /** Permission request id → the request line, so an answer can say whose. */
  requests: Map<string, AuthAuditEvent>
}

/** Where it came from, in plain words. */
export function routeWords(ingress: string): string {
  const ing = ingress.trim()
  if (ing === 'loopback') return 'this computer'
  if (ing === 'lan') return 'the local network'
  if (ing === 'tunnel') return 'the tunnel'
  if (ing.startsWith('edge:')) return 'the web address'
  return ''
}

/** The address and route for a sign-in: "203.0.113.9 · from the web address". */
export function auditWhere(ev: AuthAuditEvent): string {
  const route = routeWords(ev.ingress)
  const ip = ev.ip && ev.ip !== '-' ? ev.ip : ''
  return [ip, route ? `through ${route}` : ''].filter(Boolean).join(' · ')
}

function permissionDetail(ev: AuthAuditEvent, ctx: DetailContext): string {
  if (ev.event === 'permission-switch') {
    const p = ev.outcome.trim().split(/\s+/)
    if (p[0] === 'server' && (p[1] === 'on' || p[1] === 'off')) return `Permission cards turned ${p[1]} for this server`
    if (p[0] === 'agent' && p[2] && (p[2] === 'on' || p[2] === 'off'))
      return `Permission cards turned ${p[2]} for the ${ctx.ws(p[1] ?? '')} agent`
    return ''
  }
  const line = parsePermissionLine(ev.outcome)
  if (!line) return ''
  const what = actionWords(line.action)
  const asked = ctx.requests.get(line.id)
  const asker = asked ? auditWhoText(asked.user, ctx.ws) : ''
  const theirs = asker ? `${asker}’s request to ${what}` : `a request to ${what}`
  switch (ev.event) {
    case 'permission-request':
      return `${cap(what)} · ${STATE_WORDS[line.state] ?? line.state.replace(/_/g, ' ')}`
    case 'permission-answer':
      if (line.state === 'approved') return asker ? `Allowed ${asker} to ${what}` : `Allowed a request to ${what}`
      if (line.state === 'denied') return `Denied ${theirs}`
      return `${cap(theirs)} · ${STATE_WORDS[line.state] ?? line.state.replace(/_/g, ' ')}`
    case 'permission-note': {
      const change = /note=(\w+)/.exec(line.rest)?.[1]
      const verb = change === 'deleted' ? 'Removed the note on' : change === 'replaced' ? 'Changed the note on' : 'Wrote a note on'
      return `${verb} ${theirs}`
    }
    case 'permission-undo': {
      const undo = /undo=(\S+)/.exec(line.rest)?.[1]
      const failed = undo && undo !== 'applied' && undo !== 'ok'
      return `Turned off again: ${theirs}${failed ? ' · it couldn’t be turned off' : ''}`
    }
  }
  return ''
}

function vaultDetail(ev: AuthAuditEvent, ctx: DetailContext): string {
  const { deny, rest, parts } = words(ev.outcome)
  const verb = parts[0] ?? ''
  if (deny) {
    const [head, code = ''] = rest.split(/:\s*/, 2)
    const send = /^send (\S+) to (\S+)/.exec(head ?? '')
    const lead = send ? `${send[1]} to ${ctx.ws(send[2]!)} · ` : ''
    return code ? `${lead}Refused: ${codeWords(code)}` : lead.replace(/ · $/, '')
  }
  if (ev.event === 'vault-handoff') {
    let m: RegExpExecArray | null
    if ((m = /^send (\S+) to (\S+)(?:\s+\S+)?(?:\s+(\S+))?/.exec(rest))) {
      const status = m[3] === 'pending' ? 'waiting for them to accept' : m[3] === 'landed' ? 'delivered' : m[3] ?? ''
      return [`${m[1]} to ${ctx.ws(m[2]!)}`, status].filter(Boolean).join(' · ')
    }
    if ((m = /^receive (\S+) from (\S+) into (\S+)/.exec(rest))) return `${m[1]} from ${ctx.ws(m[2]!)}, into ${ctx.ws(m[3]!)}`
    if ((m = /^ask (\S+) from (\S+)/.exec(rest))) return `${m[1]}, asked of ${ctx.ws(m[2]!)}`
    if ((m = /^unsend (\S+)/.exec(rest))) return m[1]!
    if ((m = /^accept-from (add|remove) (\S+) for (\S+)/.exec(rest)))
      return m[1] === 'add' ? `${ctx.ws(m[3]!)} now takes secrets from ${m[2]} without asking` : `${ctx.ws(m[3]!)} asks again before taking secrets from ${m[2]}`
    if ((m = /^send-allow (add|remove) (\S+) for (\S+) to (\S+)/.exec(rest)))
      return m[1] === 'add' ? `${ctx.ws(m[3]!)} may send ${m[2]} to ${ctx.ws(m[4]!)}` : `${ctx.ws(m[3]!)} may no longer send ${m[2]} to ${ctx.ws(m[4]!)}`
    if ((m = /^\S+ (\S+) from (\S+)(?:\s+\S+)?(?:\s+as\s+(\S+))?/.exec(rest)))
      return `${m[1]} from ${ctx.ws(m[2]!)}${m[3] ? `, kept as ${m[3]}` : ''}`
    return ''
  }
  // vault-carry
  let m: RegExpExecArray | null
  if (verb === 'export' && (m = /^export (\d+) \[([^\]]*)\]/.exec(rest))) {
    const list = secretList(m[2]!.split(','))
    return `${list || secrets(Number(m[1]))}, copied out to another K2 server`
  }
  if (verb === 'import' && (m = /^import (\d+) from \S+ into (\S+)/.exec(rest))) {
    const into = m[2] === '-' ? 'this server' : m[2]!.split(',').map(ctx.ws).join(', ')
    return `${secrets(Number(m[1]))}, into ${into}`
  }
  if (verb === 'clone' && (m = /^clone (export|import) (\d+)/.exec(rest)))
    return m[1] === 'export' ? `${secrets(Number(m[2]))}, for a copy of this server` : `${secrets(Number(m[2]))}, from a copy of a server`
  return ''
}

/** The record's second line: plain words built from its fields (never the
 *  raw line). Empty when there is nothing to add. */
export function auditDetail(ev: AuthAuditEvent, ctx: DetailContext): string {
  if (ev.event === 'login' || ev.event === 'logout' || ev.event === 'change-password' || ev.event === 'set-password' || ev.event === 'add-user')
    return auditWhere(ev)
  if (ev.event === 'vault-handoff' || ev.event === 'vault-carry') return vaultDetail(ev, ctx)
  if (ev.event.startsWith('permission-')) return permissionDetail(ev, ctx)
  return ''
}

// ── The opened record ────────────────────────────────────────────────

const EVENT_WORDS: Record<string, string> = {
  login: 'Sign-in',
  logout: 'Sign-out',
  'change-password': 'Password change',
  'set-password': 'Password set',
  'add-user': 'New person',
  'vault-handoff': 'Secret handoff',
  'vault-carry': 'Vault copy',
  'permission-request': 'Permission request',
  'permission-answer': 'Permission answer',
  'permission-note': 'Note on a permission request',
  'permission-undo': 'Permission turned off again',
  'permission-switch': 'Permission cards switch',
}

const CLIENT_WORDS: Record<string, string> = {
  web: 'A web browser',
  api: 'The K2 app or command line',
}

/** One row of the opened record: a plain value, and the raw value quietly. */
export type AuditField = { label: string; value: string; raw?: string }

export function auditFields(ev: AuthAuditEvent, ctx: DetailContext, when: { local: string; utc: string }): AuditField[] {
  const who = auditWho(ev.user, ctx.ws)
  const whoText = auditWhoText(ev.user, ctx.ws)
  const out: AuditField[] = [{ label: 'When', value: when.local, raw: `${when.utc} UTC` }]
  out.push({ label: 'Who', value: whoText, ...(ev.user && ev.user !== whoText ? { raw: ev.user } : {}) })
  if (who.session) out.push({ label: 'Session', value: who.session })
  out.push({ label: 'What', value: EVENT_WORDS[ev.event] ?? 'Other', ...(ev.event ? { raw: ev.event } : {}) })
  if (ev.event === 'login') {
    const result = SIGN_IN_RESULT[ev.outcome] ?? (ev.outcome.startsWith('bad_attest') ? 'Blocked: the sign-in couldn’t be verified' : 'Couldn’t sign in')
    out.push({ label: 'Result', value: result })
  } else if (ev.event.startsWith('permission-') && ev.event !== 'permission-switch') {
    const line = parsePermissionLine(ev.outcome)
    if (line) {
      out.push({ label: 'Asked to', value: cap(actionWords(line.action)), raw: line.action })
      out.push({ label: 'Result', value: cap(STATE_WORDS[line.state] ?? line.state.replace(/_/g, ' ')) })
      const how = ev.event === 'permission-request' ? 'Asked with' : ev.event === 'permission-answer' ? 'Answered with' : 'Done with'
      out.push({ label: how, value: VIA_WORDS[line.via] ?? line.via })
      out.push({ label: 'Request', value: line.id })
    }
  } else {
    const detail = auditDetail(ev, ctx)
    if (detail) out.push({ label: 'Details', value: detail })
  }
  if (ev.ip && ev.ip !== '-') out.push({ label: 'Address', value: ev.ip })
  const route = routeWords(ev.ingress)
  if (route) out.push({ label: 'Came in through', value: cap(route), raw: ev.ingress })
  if (ev.client && CLIENT_WORDS[ev.client]) out.push({ label: 'Signed in with', value: CLIENT_WORDS[ev.client]! })
  if (!isPlainSignIn(ev) && ev.outcome) out.push({ label: 'Log line', value: '', raw: ev.outcome })
  return out
}
