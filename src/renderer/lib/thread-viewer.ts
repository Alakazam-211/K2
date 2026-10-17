// Who is looking at a shared Thread, and which posts are theirs.
//
// A Thread is shared: the owner on this computer, Connect logins on other
// clients and the web, the Companion phone app, and app guests all see the
// same posts. The daemon stamps a person's post with `author: {kind, login}`
// from the request's credential (overlay_routes.rs `handle_post`; never the
// body): `owner` (the owner token), `user` (a Connect login, its username),
// `guest` (an app pass, its username). Agent posts carry none.
//
// The viewer comes from the server, not a new store: `GET /cli/notify/prefs`
// (`principal` + the owner's linked logins, on a server with
// `thread-push-v1`), else `GET /cli/auth/whoami`. Until that answers, the
// saved login on the scope's server entry stands in (this computer's daemon
// is always the owner token). Answers are memoized per connection; a
// `notify_prefs_changed` event drops the memo (owner logins can be linked).

import { useEffect, useState } from 'react'
import { daemonCliGet } from '@/lib/daemon-cli'
import type { ServerScope } from '@/kessel/server-scope'
import { fetchNotifyPrefs } from '@/components/Settings/sections/notify-api'
import { onNotifyPrefsChanged } from '@/stores/session-events'

/** `doc.author` on a Thread post. */
export interface ThreadAuthor {
  /** `owner` | `user` | `guest` | `imported` (a person's post that came
   *  with a clone: a historical name, never the viewer) — a later kind
   *  still parses. */
  kind: string
  /** `owner`, a Connect username, or an app guest's pass username. */
  login: string
  /** On a card the owner answered: the daemon's sender stamp (0.46.0,
   *  `person_name::sender_identity`: the owner's linked username, else
   *  `owner`; older cards carry the old display name). The renderer shows
   *  it as stamped and never re-derives it. Absent on other kinds and on
   *  cards from before the field. */
  name?: string
}

/** The person looking at a Thread on one server. */
export interface ThreadViewer {
  /** `owner` (the owner, or a login linked to the owner) or a Connect username. */
  principal: string
  /** When `principal` is `owner`: the Connect logins that are the owner too. */
  aliases: readonly string[]
}

export const OWNER_VIEWER: ThreadViewer = Object.freeze({ principal: 'owner', aliases: Object.freeze([]) as readonly string[] })

/** `doc.author`, or null when absent or malformed. */
export function parseThreadAuthor(raw: unknown): ThreadAuthor | null {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return null
  const r = raw as Record<string, unknown>
  const kind = typeof r.kind === 'string' ? r.kind.trim() : ''
  const login = typeof r.login === 'string' ? r.login.trim() : ''
  if (!kind || !login) return null
  const name = typeof r.name === 'string' ? r.name.trim() : ''
  return name ? { kind, login, name } : { kind, login }
}

const fold = (s: string): string => s.trim().toLowerCase()

/** Is `author` the person `viewer`? An unknown viewer owns nothing; a guest
 *  (or an unknown kind) is never a desktop/web viewer. */
export function isViewerAuthor(author: ThreadAuthor, viewer: ThreadViewer | null): boolean {
  if (!viewer) return false
  const me = fold(viewer.principal)
  if (!me) return false
  if (author.kind === 'owner') return me === 'owner'
  if (author.kind !== 'user') return false
  const login = fold(author.login)
  if (!login) return false
  if (me !== 'owner') return login === me
  return viewer.aliases.some((a) => fold(a) === login)
}

/** Is this Thread post the viewer's own? A post with `author` is theirs
 *  only when that person is the viewer. A post without one (a doc from
 *  before the field, or an older server) keeps the old rule: every
 *  `via=compose` post reads as yours, since nothing on it says who sent it. */
export function isOwnThreadPost(
  doc: { via?: string | null; author?: unknown },
  viewer: ThreadViewer | null,
): boolean {
  const author = parseThreadAuthor(doc.author)
  if (author) return isViewerAuthor(author, viewer)
  return doc.via === 'compose'
}

/** Who answered (or dismissed) a card, from the card's `answeredBy` (the
 *  daemon stamps it from the answerer's credential, like a post's
 *  `author`): "You" for the viewer; for the owner token, the daemon's
 *  stamp (the linked username, as the owner's posts show), or "the owner"
 *  when none is set; else the login. Null when the card names nobody
 *  (still pending, an agent's own answer, or a card from before the
 *  field), so it renders as before. */
export function cardAnswererLabel(raw: unknown, viewer: ThreadViewer | null): string | null {
  const a = parseThreadAuthor(raw)
  if (!a) return null
  if (isViewerAuthor(a, viewer)) return 'You'
  if (a.kind === 'owner') return a.name && a.name.toLowerCase() !== 'owner' ? a.name : 'the owner'
  // A card that came with a clone: its historical name ("Rosson",
  // "alice (Connect, rosson.k2.dev)"), never "You" here.
  if (a.kind === 'imported') return a.name || a.login
  return a.login
}

/** The viewer before the server answers: the saved login on the scope's
 *  server entry, or the owner on this computer. A remote with no saved
 *  login (a hosted web page) is unknown until whoami answers. */
export function guessThreadViewer(scope: ServerScope): ThreadViewer | null {
  const username = scope.connectHost()?.username?.trim()
  if (username) return { principal: username, aliases: [] }
  return scope.isRemote ? null : OWNER_VIEWER
}

const RETRY_MS = 60_000
const cache = new Map<string, ThreadViewer>()
const inflight = new Map<string, Promise<ThreadViewer | null>>()
const failedAt = new Map<string, number>()
const listeners = new Set<() => void>()

function keyOf(scope: ServerScope): string {
  return scope.connectionKey
}

function changed(): void {
  for (const fn of [...listeners]) fn()
}

async function fetchViewer(scope: ServerScope): Promise<ThreadViewer> {
  if (scope.serverSupports('thread-push-v1')) {
    try {
      const prefs = await fetchNotifyPrefs(scope)
      if (prefs.principal) return { principal: prefs.principal, aliases: prefs.aliases }
    } catch (e) {
      console.warn('[thread] notify/prefs failed; asking whoami', e)
    }
  }
  const w = await daemonCliGet<{ owner?: unknown; username?: unknown }>(scope, 'auth/whoami')
  if (w?.owner === true) return OWNER_VIEWER
  const username = typeof w?.username === 'string' ? w.username.trim() : ''
  if (!username) throw new Error('auth/whoami named no login')
  return { principal: username, aliases: [] }
}

/** The server's answer for `scope`, if it already came. */
export function cachedThreadViewer(scope: ServerScope): ThreadViewer | null {
  return cache.get(keyOf(scope)) ?? null
}

/** The viewer now: the server's answer, else the guess. Pure (no fetch). */
export function threadViewerNow(scope: ServerScope): ThreadViewer | null {
  return cachedThreadViewer(scope) ?? guessThreadViewer(scope)
}

/** Ask the server who this is (one request per connection; a failure
 *  answers the guess and is retried after a minute). */
export function loadThreadViewer(scope: ServerScope): Promise<ThreadViewer | null> {
  const key = keyOf(scope)
  const hit = cache.get(key)
  if (hit) return Promise.resolve(hit)
  const running = inflight.get(key)
  if (running) return running
  const failed = failedAt.get(key)
  if (failed !== undefined && Date.now() - failed < RETRY_MS) return Promise.resolve(guessThreadViewer(scope))
  const p = fetchViewer(scope)
    .then(
      (v) => {
        cache.set(key, v)
        failedAt.delete(key)
        changed()
        return v as ThreadViewer | null
      },
      (e: unknown) => {
        console.warn('[thread] viewer lookup failed; using the saved login', e)
        failedAt.set(key, Date.now())
        return guessThreadViewer(scope)
      },
    )
    .finally(() => {
      inflight.delete(key)
    })
  inflight.set(key, p)
  return p
}

/** Drop the memo for `scope` (its owner logins changed). */
export function forgetThreadViewer(scope: ServerScope): void {
  cache.delete(keyOf(scope))
  failedAt.delete(keyOf(scope))
}

/** Called whenever a server's answer lands. */
export function onThreadViewerChange(fn: () => void): () => void {
  listeners.add(fn)
  return () => {
    listeners.delete(fn)
  }
}

/** Tests: forget every answer and listener. */
export function resetThreadViewersForTests(): void {
  cache.clear()
  inflight.clear()
  failedAt.clear()
  listeners.clear()
}

/** Tests: set the server's answer for `scope`. */
export function setThreadViewerForTests(scope: ServerScope, viewer: ThreadViewer): void {
  cache.set(keyOf(scope), viewer)
  changed()
}

/** The person looking at `scope`'s Threads, kept current. */
export function useThreadViewer(scope: ServerScope): ThreadViewer | null {
  const [, bump] = useState(0)
  useEffect(() => {
    const off = onThreadViewerChange(() => bump((n) => n + 1))
    void loadThreadViewer(scope)
    const offPrefs = onNotifyPrefsChanged(scope, () => {
      forgetThreadViewer(scope)
      void loadThreadViewer(scope)
    })
    return () => {
      off()
      offPrefs()
    }
  }, [scope])
  return threadViewerNow(scope)
}
