// Home 0.43.2 (prd-home-seamless-0432 Z21, Z24, Z25, Z39) — one list of
// `/cli/sessions/events` kinds, and who consumes each one on this client.
//
// The daemon and this client used to keep four hand-written kind lists
// (the per-workspace socket's swallow switch, the app socket's `if` chain,
// `dispatchAppEvent`, and the room carrier's `CARRIED_KINDS`). They drifted:
// `agent_status_changed` shipped in 0.39.39 and the per-workspace socket
// warned "unknown event kind" on every hook for a year.
//
// Now there is one registry, `src/shared/session-event-kinds.json`
// (`kind` → `app` | `workspace` | `handshake`), checked on both sides:
//   - Rust: `session_events_ws::tests::session_event_kinds_match_shared_registry`
//     reads the `SessionEvent` enum's variants from source and fails when
//     one is missing from the JSON, or when a class disagrees with routing.
//   - TS: `SESSION_EVENT_ROUTES` below is typed against the
//     `SessionEventMessage` union (a missing or extra kind does not compile),
//     and `session-event-kinds.test.ts` checks its keys and classes equal the
//     JSON's. A kind the daemon adds therefore fails a test until this table
//     says who handles it, or says on purpose that nobody does.
// The four lists are derived from this table.
//
// At runtime a kind that is in the registry but not owned by a socket is
// dropped silently there. A kind that is NOT in the registry (a newer
// daemon) warns once per kind per page and is counted for tests
// (`unknownEventKindCounts`).

import registry from '@shared/session-event-kinds.json'
import type { SessionEventMessage } from '@/stores/session-events'

export type SessionEventClass = 'app' | 'workspace' | 'handshake'

/** Who consumes one kind on this client. */
export interface SessionEventRoute {
  /** The daemon's routing class; equals the registry's. */
  readonly class: SessionEventClass
  /** Handled by `subscribeToWorkspaceSessionEvents` (tabs store, pinned
   *  Chat, pinned rooms). */
  readonly workspace?: true
  /** Handled by `subscribeToWorkspaceTabEvents`. */
  readonly tabs?: true
  /** Dispatched on the server's app bus by its app socket
   *  (`subscribeToActiveState`). */
  readonly app?: true
  /** Also dispatched by a pinned room's carrier socket, for a server with
   *  no app socket of its own (Home M4, MS14/MS16). */
  readonly carried?: true
  /** No consumer, on purpose: why. */
  readonly ignored?: string
}

/** Every wire kind, and who handles it. Typed against the client union:
 *  a kind missing here, or one the union doesn't have, does not compile. */
export const SESSION_EVENT_ROUTES = {
  hello: { class: 'handshake', workspace: true, tabs: true, app: true },
  session_added: { class: 'workspace', workspace: true, app: true },
  session_removed: { class: 'workspace', workspace: true, app: true },
  session_renamed: { class: 'workspace', workspace: true },
  active_changed: { class: 'app', app: true, carried: true },
  llm_status_changed: { class: 'app', app: true },
  // Home 0.43.2 (Z22): a pinned room applies hook status through its own
  // server's carrier.
  agent_status_changed: { class: 'app', app: true, carried: true },
  session_activity_changed: { class: 'app', app: true, carried: true },
  review_queue_changed: {
    class: 'workspace',
    ignored: 'The Review Queue modal was deleted in 0.40.31; `k2 review` still emits it for the CLI.',
  },
  review_changed: {
    class: 'workspace',
    ignored: 'ReviewPanel was deleted in 0.40.31; `k2 review` still emits it for the CLI.',
  },
  tunnel_status_changed: { class: 'app', app: true },
  tunnel_subdomains_changed: { class: 'app', app: true },
  publish_services_changed: { class: 'app', app: true, carried: true },
  workspace_resources_changed: { class: 'app', app: true, carried: true },
  tab_title_changed: { class: 'workspace', workspace: true, tabs: true },
  tab_order_changed: { class: 'workspace', workspace: true, tabs: true },
  heartbeat_state_changed: { class: 'workspace', tabs: true },
  heartbeat_roster_changed: { class: 'workspace', tabs: true },
  projects_changed: { class: 'app', app: true, carried: true },
  chat_history_changed: { class: 'app', app: true, carried: true },
  token_usage_changed: { class: 'app', app: true },
  presence_changed: { class: 'app', app: true, carried: true },
  // Not carried: a room never opens tabs on another server's say-so.
  open_url: { class: 'app', app: true },
  project_groups_changed: { class: 'app', app: true },
  feedback_changed: { class: 'app', app: true },
  // Home 0.43.2 (Q7): Settings → Email refetches on it (`onMailChanged`).
  mail_changed: { class: 'app', app: true },
  remote_session_access_denied: {
    class: 'app',
    ignored: 'Owner audit of a refused remote drive; no client surface shows it yet.',
  },
  fs_changed: { class: 'app', app: true, carried: true },
} as const satisfies { readonly [K in SessionEventMessage['kind']]: SessionEventRoute }

type Routes = typeof SESSION_EVENT_ROUTES
export type SessionEventKind = keyof Routes

type Owner = 'workspace' | 'tabs' | 'app' | 'carried'

/** Kinds whose route sets `owner`. */
export type KindsOwnedBy<O extends Owner> = {
  [K in SessionEventKind]: Routes[K] extends { readonly [P in O]: true } ? K : never
}[SessionEventKind]

/** The registry JSON, as read by both sides. */
export const SESSION_EVENT_REGISTRY: Readonly<Record<string, SessionEventClass>> = registry as Record<
  string,
  SessionEventClass
>

function hasOwn(obj: object, key: string): boolean {
  return Object.prototype.hasOwnProperty.call(obj, key)
}

/** True when `kind` is a registry kind (handled somewhere, or ignored on
 *  purpose). */
export function isKnownSessionEventKind(kind: string): kind is SessionEventKind {
  return hasOwn(SESSION_EVENT_ROUTES, kind)
}

/** A route lookup that never reads the prototype (`constructor` is not a
 *  kind). */
export function routeFor(kind: string): SessionEventRoute | null {
  return isKnownSessionEventKind(kind) ? SESSION_EVENT_ROUTES[kind] : null
}

function kindsOwnedBy(owner: Owner): ReadonlySet<string> {
  const out = new Set<string>()
  for (const [kind, route] of Object.entries(SESSION_EVENT_ROUTES) as Array<[string, SessionEventRoute]>) {
    if (route[owner] === true) out.add(kind)
  }
  return out
}

/** App-level frames a room carrier forwards (derived; was hand-kept). */
export const CARRIED_KINDS: ReadonlySet<string> = kindsOwnedBy('carried')

/** Frames the app socket dispatches on its bus (derived). */
export const APP_SOCKET_KINDS: ReadonlySet<string> = kindsOwnedBy('app')

/** A typed handler table keyed by exactly the kinds `owner` handles. A
 *  route that gains the owner without a handler does not compile. */
export type DispatchTable<O extends Owner, H> = {
  readonly [K in KindsOwnedBy<O>]: (handlers: H, msg: Extract<SessionEventMessage, { kind: K }>) => void
}

/** Run `table`'s handler for `msg`, if it has one. Unknown kinds are
 *  reported through `noteUnknownEventKind`; known kinds this socket doesn't
 *  own are dropped silently. Returns true when a handler ran. */
export function dispatchFrom<H>(
  table: Readonly<Record<string, (handlers: H, msg: never) => void>>,
  socket: string,
  handlers: H,
  msg: { kind?: unknown },
): boolean {
  const kind = typeof msg.kind === 'string' ? msg.kind : ''
  if (hasOwn(table, kind)) {
    table[kind](handlers, msg as never)
    return true
  }
  if (!isKnownSessionEventKind(kind)) noteUnknownEventKind(socket, kind)
  return false
}

// ── Unknown kinds: warn once per kind per page (Z25) ──────────────────────

const _unknownCounts = new Map<string, number>()

/** Count a kind the registry doesn't know; warn the first time only. */
export function noteUnknownEventKind(socket: string, kind: string): void {
  const n = (_unknownCounts.get(kind) ?? 0) + 1
  _unknownCounts.set(kind, n)
  if (n === 1) {
    console.warn(
      `[session-events] unknown event kind: ${kind || '(none)'} (first seen on the ${socket} socket; ` +
        'a newer server? add it to src/shared/session-event-kinds.json)',
    )
  }
}

/** Frames per unknown kind since page load (tests read it). */
export function unknownEventKindCounts(): Record<string, number> {
  return Object.fromEntries(_unknownCounts)
}

/** Test seam: forget the counts (and so warn again). */
export function __resetUnknownEventKindsForTests(): void {
  _unknownCounts.clear()
}
