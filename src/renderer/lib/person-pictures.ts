// Profile pictures for K2's own people (0.46.8, Rosson 2026-11-04): the
// owner and every Connect login. The daemon owns them
// (`/cli/people/picture`, k2.db `person_pictures`); this module is the one
// place the app reads them, so every person chip (`PresenceChip`: the top
// bar, Home, Thread, Admin Access, the profile modal) shows the same one.
//
// - **Per server** (Rosson 2026-11-04): each server keeps its own picture
//   for each person, and a person may have different ones on two servers.
//   So everything here is keyed by the server (`scope.hostKey`): a chip
//   reads the picture from the server that owns the room, Thread or page
//   it sits in, with that server's own login. Nothing is copied between
//   servers, and nothing goes to K2 Connect.
// - `GET people/picture` lists who has one (`{person, logins, imageHash}`;
//   an owner-linked login shows the owner's), then each picture's bytes are
//   read once per hash (`?person=<login>`, a JPEG data URL; the app CSP
//   blocks a local http image, so it's never a URL to the daemon).
// - Live: `person_picture_changed` on that server's app bus (carried for a
//   pinned room) re-reads the list; so does every app `hello` (reconnect).
//   Subscriptions are counted by the chips using a server and dropped when
//   the last one unmounts.
// - An older server (no route: `route not found`) shows initials, and is
//   asked again on its next hello.

import { useEffect } from 'react'
import { create } from 'zustand'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import type { ServerScope } from '@/kessel/server-scope'
import { onAppHello, onPersonPictureChanged } from '@/stores/session-events'

export const PICTURE_ROUTE = 'people/picture'
export const PICTURE_CLEAR_ROUTE = 'people/picture/clear'
/** The daemon's upload cap (agent_look::MAX_INPUT_IMAGE_BYTES). */
export const PICTURE_MAX_BYTES = 2 * 1024 * 1024
/** What the daemon accepts (it re-encodes every one to JPEG). */
export const PICTURE_ACCEPT = 'image/png,image/jpeg,image/webp'

export type PicturesStatus = 'idle' | 'loading' | 'ok' | 'unsupported' | 'error'

export interface HostPictures {
  status: PicturesStatus
  /** Lowercased login (`owner`, a Connect username, an owner-linked
   *  login) → the picture's hash. */
  byLogin: Record<string, string>
}

interface PicturesState {
  hosts: Record<string, HostPictures>
  /** `${hostKey}|${hash}` → JPEG data URL. */
  urls: Record<string, string>
}

export const usePersonPicturesStore = create<PicturesState>(() => ({ hosts: {}, urls: {} }))

const EMPTY: HostPictures = { status: 'idle', byLogin: {} }

function urlKey(hostKey: string, hash: string): string {
  return `${hostKey}|${hash}`
}

function setHost(hostKey: string, patch: Partial<HostPictures>): void {
  usePersonPicturesStore.setState((s) => ({
    hosts: { ...s.hosts, [hostKey]: { ...(s.hosts[hostKey] ?? EMPTY), ...patch } },
  }))
}

/** Parse `GET people/picture`'s `people` into login → hash. */
export function parsePictureList(raw: unknown): Record<string, string> {
  const people = (raw as { people?: unknown } | null)?.people
  const out: Record<string, string> = {}
  if (!Array.isArray(people)) return out
  for (const p of people) {
    if (!p || typeof p !== 'object') continue
    const r = p as Record<string, unknown>
    if (typeof r.imageHash !== 'string' || r.imageHash === '') continue
    const logins = Array.isArray(r.logins) ? r.logins : [r.person]
    for (const l of logins) {
      if (typeof l === 'string' && l !== '') out[l.trim().toLowerCase()] = r.imageHash
    }
  }
  return out
}

function message(e: unknown): string {
  return e instanceof Error ? e.message : String(e)
}

const listInFlight = new Map<string, Promise<void>>()

/** Re-read who has a picture on `scope`'s server. */
export function refreshPersonPictures(scope: ServerScope): Promise<void> {
  const hostKey = scope.hostKey
  const running = listInFlight.get(hostKey)
  if (running) return running
  const cur = usePersonPicturesStore.getState().hosts[hostKey]
  if (!cur || cur.status === 'idle') setHost(hostKey, { status: 'loading' })
  const p = daemonCliGet<unknown>(scope, PICTURE_ROUTE)
    .then(
      (raw) => setHost(hostKey, { status: 'ok', byLogin: parsePictureList(raw) }),
      (e: unknown) => {
        if (message(e).includes('route not found')) setHost(hostKey, { status: 'unsupported', byLogin: {} })
        else {
          // Keep what was shown; the next hello or change event retries.
          console.debug(`[person-pictures] ${hostKey} list skipped:`, e)
          setHost(hostKey, { status: 'error' })
        }
      },
    )
    .finally(() => listInFlight.delete(hostKey))
  listInFlight.set(hostKey, p)
  return p
}

const urlInFlight = new Set<string>()

/** Read one picture's bytes (once per server and hash). */
function loadPicture(scope: ServerScope, login: string, hash: string): void {
  const hostKey = scope.hostKey
  const key = urlKey(hostKey, hash)
  if (usePersonPicturesStore.getState().urls[key] || urlInFlight.has(key)) return
  urlInFlight.add(key)
  daemonCliGet<{ imageHash?: unknown; imageUrl?: unknown }>(scope, PICTURE_ROUTE, { person: login })
    .then(
      (v) => {
        const got = typeof v?.imageHash === 'string' ? v.imageHash : null
        const url = typeof v?.imageUrl === 'string' && v.imageUrl.startsWith('data:image/jpeg;base64,') ? v.imageUrl : null
        if (got && url) {
          usePersonPicturesStore.setState((s) => ({ urls: { ...s.urls, [urlKey(hostKey, got)]: url } }))
        }
        // The picture changed between the list and this read: re-list.
        if (got !== hash) void refreshPersonPictures(scope)
      },
      (e: unknown) => console.debug(`[person-pictures] ${hostKey} picture for ${login} skipped:`, e),
    )
    .finally(() => urlInFlight.delete(key))
}

interface Retained {
  refs: number
  unsubs: Array<() => void>
}

const retained = new Map<string, Retained>()

/** Count one user of `scope`'s pictures: the first one subscribes to that
 *  server's change events and hellos and reads the list; the release of
 *  the last one drops the subscriptions. */
export function retainPersonPictures(scope: ServerScope): () => void {
  const hostKey = scope.hostKey
  let entry = retained.get(hostKey)
  if (!entry) {
    entry = {
      refs: 0,
      unsubs: [
        onPersonPictureChanged(scope, () => void refreshPersonPictures(scope)),
        onAppHello(scope, () => void refreshPersonPictures(scope)),
      ],
    }
    retained.set(hostKey, entry)
    const status = usePersonPicturesStore.getState().hosts[hostKey]?.status ?? 'idle'
    if (status === 'idle' || status === 'error') void refreshPersonPictures(scope)
  }
  entry.refs += 1
  let released = false
  return () => {
    if (released) return
    released = true
    const e = retained.get(hostKey)
    if (!e) return
    e.refs -= 1
    if (e.refs <= 0) {
      for (const u of e.unsubs) u()
      retained.delete(hostKey)
    }
  }
}

/** The picture `login` has on `scope`'s server, as a JPEG data URL, or
 *  null (none, not loaded yet, or an older server). Pass a null scope to
 *  read nothing. */
export function usePersonPicture(scope: ServerScope | null, login: string | null | undefined): string | null {
  const hostKey = scope?.hostKey ?? null
  const name = login?.trim().toLowerCase() ?? ''
  const hash = usePersonPicturesStore((s) => (hostKey && name ? s.hosts[hostKey]?.byLogin[name] ?? null : null))
  const url = usePersonPicturesStore((s) => (hostKey && hash ? s.urls[urlKey(hostKey, hash)] ?? null : null))
  useEffect(() => {
    if (!scope) return
    return retainPersonPictures(scope)
    // The scope object is stable per server; its hostKey is the identity.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [hostKey])
  useEffect(() => {
    if (scope && hash && !url && name) loadPicture(scope, name, hash)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [hostKey, name, hash, url])
  return url
}

/** One write's answer. */
export interface PictureWrite {
  person: string
  imageHash: string | null
  imageUrl: string | null
}

function applyWrite(scope: ServerScope, v: PictureWrite): void {
  const hostKey = scope.hostKey
  if (v.imageHash && v.imageUrl) {
    usePersonPicturesStore.setState((s) => ({ urls: { ...s.urls, [urlKey(hostKey, v.imageHash as string)]: v.imageUrl as string } }))
  }
  // The person's own login shows it at once; the re-read fills the rest
  // (owner-linked logins).
  const byLogin = { ...(usePersonPicturesStore.getState().hosts[hostKey]?.byLogin ?? {}) }
  const before = byLogin[v.person]
  if (v.imageHash) byLogin[v.person] = v.imageHash
  else {
    delete byLogin[v.person]
    if (before) for (const [login, h] of Object.entries(byLogin)) if (h === before) delete byLogin[login]
  }
  setHost(hostKey, { status: 'ok', byLogin })
  void refreshPersonPictures(scope)
}

function parseWrite(raw: unknown): PictureWrite {
  const v = (raw ?? {}) as Record<string, unknown>
  if (typeof v.person !== 'string') throw new Error('the server answered without a person')
  return {
    person: v.person.toLowerCase(),
    imageHash: typeof v.imageHash === 'string' ? v.imageHash : null,
    imageUrl: typeof v.imageUrl === 'string' ? v.imageUrl : null,
  }
}

/** Set (or replace) a picture: yours, or `person`'s (the daemon decides
 *  who may: Admin Access's rule). `dataUrl` is the file as read. */
export async function setPersonPicture(scope: ServerScope, dataUrl: string, person?: string): Promise<PictureWrite> {
  const v = parseWrite(await daemonCliPost<unknown>(scope, PICTURE_ROUTE, person ? { image: dataUrl, person } : { image: dataUrl }))
  applyWrite(scope, v)
  return v
}

/** Remove a picture: yours, or `person`'s. */
export async function clearPersonPicture(scope: ServerScope, person?: string): Promise<PictureWrite> {
  const v = parseWrite(await daemonCliPost<unknown>(scope, PICTURE_CLEAR_ROUTE, person ? { person } : {}))
  applyWrite(scope, v)
  return v
}

/** A refusal before anything is sent, or null. */
export function pictureFileProblem(file: Pick<File, 'type' | 'size'>): string | null {
  if (!PICTURE_ACCEPT.split(',').includes(file.type)) return 'Pick a PNG, JPEG or WebP picture.'
  if (file.size > PICTURE_MAX_BYTES) return 'That picture is over 2 MB. Pick a smaller one.'
  return null
}

/** Read a picked file as a data URL. */
export function readPictureFile(file: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const r = new FileReader()
    r.onload = () => (typeof r.result === 'string' ? resolve(r.result) : reject(new Error('could not read the picture')))
    r.onerror = () => reject(r.error ?? new Error('could not read the picture'))
    r.readAsDataURL(file)
  })
}

/** Tests: forget everything. */
export function resetPersonPicturesForTests(): void {
  for (const e of retained.values()) for (const u of e.unsubs) u()
  retained.clear()
  listInFlight.clear()
  urlInFlight.clear()
  usePersonPicturesStore.setState({ hosts: {}, urls: {} })
}
