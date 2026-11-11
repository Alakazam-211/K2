// Settings → User Access: how the page reads its data, part by part.
//
// The page needs four reads: the people (`skin/users`), what each can use
// (`skin/grants`), the roles (`skin/roles`) and the App names (one
// `publish/list` per workspace). They used to be one all-or-nothing read:
// any one failing (a busy database, one workspace that didn't answer, a
// moment of reconnecting) blanked the whole list. Now each part loads on its
// own, shows its own loading and failure, and retries alone:
//
// - parts run side by side; the people show as soon as they arrive;
// - a part that fails keeps what it last had and says why;
// - only the newest answer for a part is used (an older read that lands
//   late, after a save's re-read, is dropped);
// - while the window's server is reconnecting, a read waits and retries by
//   itself once it's back, instead of failing with "request skipped";
// - the people list's own details (locks, app access, …) load part by part
//   on the daemon; the ones that didn't are in `userDetails`, and their
//   fields stay unknown on the rows (never read as "no");
// - App names: a workspace that fails is counted, the rest still list.

import { useCallback, useEffect, useRef, useState } from 'react'
import { daemonCliGet } from '@/lib/daemon-cli'
import { daemonErrorText } from '@/lib/daemon-error'
import { primaryScope } from '@/kessel/server-scope'
import { useConnectHostStore } from '@/stores/connect-host'
import {
  HOST_APP,
  fetchAppsSettled,
  parseGrants,
  parsePeople,
  parseRoles,
  parseUserDetailErrors,
  type AppOption,
  type GrantRow,
  type PersonRow,
  type RoleRow,
} from './user-access-api'

export type PartName = 'users' | 'grants' | 'roles' | 'apps'

/** One part of the page's data. `data` is null until the first answer; a
 *  later failure keeps it. `waiting`: the server is reconnecting. */
export type Part<T> = {
  status: 'loading' | 'ready' | 'error'
  data: T | null
  error: string | null
  waiting: boolean
}

export type AppsData = { apps: AppOption[]; missed: number; reason: string | null }

/** The users list's parts that didn't load on the daemon (part → reason),
 *  from the newest people read; empty when everything loaded. */
export type UserDetailErrors = Record<string, string>

export type AccessData = {
  users: Part<PersonRow[]>
  userDetails: UserDetailErrors
  grants: Part<GrantRow[]>
  roles: Part<RoleRow[]>
  apps: Part<AppsData>
}

const idle = <T,>(): Part<T> => ({ status: 'loading', data: null, error: null, waiting: false })

/** A read skipped because the window's server is reconnecting (daemon-cli's
 *  RecoveringError), or one whose answer was dropped because the window
 *  switched servers (HostSwitchedError). Matched by name, not class. */
function errorName(e: unknown): string | null {
  return e instanceof Error ? e.name : null
}

type PeopleRead = { people: PersonRow[]; details: UserDetailErrors }
type ReadValue = {
  users: PeopleRead
  grants: GrantRow[]
  roles: RoleRow[]
  apps: AppsData
}

const READERS: { [K in PartName]: () => Promise<ReadValue[K]> } = {
  users: async () => {
    const raw = await daemonCliGet<unknown>(primaryScope(), 'skin/users')
    return { people: parsePeople(raw), details: parseUserDetailErrors(raw) }
  },
  grants: async () => parseGrants(await daemonCliGet<unknown>(primaryScope(), 'skin/grants')),
  roles: async () => parseRoles(await daemonCliGet<unknown>(primaryScope(), 'skin/roles')),
  apps: () => fetchAppsSettled(daemonErrorText),
}

/**
 * The page's data, each part on its own. `reload(part)` re-reads one part
 * (Try again on that part); `refresh()` re-reads the people, grants and
 * roles after a save, keeping what's shown while it runs. App names load
 * when `wantApps` turns true (someone has an App, or a form needs the list).
 */
export function useAccessData(wantApps: boolean): AccessData & {
  reload: (part: PartName) => Promise<void>
  refresh: () => Promise<void>
} {
  const [data, setData] = useState<AccessData>(() => ({ users: idle(), userDetails: {}, grants: idle(), roles: idle(), apps: idle() }))
  const seq = useRef<Record<PartName, number>>({ users: 0, grants: 0, roles: 0, apps: 0 })
  const live = useRef(true)
  const [waiting, setWaiting] = useState<Set<PartName>>(() => new Set())

  const reload = useCallback(async (part: PartName): Promise<void> => {
    const mine = ++seq.current[part]
    setData((d) => ({ ...d, [part]: { ...d[part], status: d[part].data == null || d[part].status === 'error' ? 'loading' : d[part].status, waiting: false } }))
    const current = (): boolean => live.current && seq.current[part] === mine
    for (let attempt = 0; ; attempt++) {
      try {
        const value = await READERS[part]()
        if (!current()) return
        if (part === 'users') {
          const { people, details } = value as PeopleRead
          setData((d) => ({ ...d, users: { status: 'ready', data: people, error: null, waiting: false }, userDetails: details }))
        } else {
          setData((d) => ({ ...d, [part]: { status: 'ready', data: value, error: null, waiting: false } }))
        }
        return
      } catch (e) {
        if (!current()) return
        const name = errorName(e)
        // The window switched servers mid-read: read again from the new one.
        if (name === 'HostSwitchedError' && attempt === 0) continue
        if (name === 'RecoveringError') {
          setData((d) => ({ ...d, [part]: { ...d[part], status: 'loading', waiting: true } }))
          setWaiting((w) => new Set(w).add(part))
          return
        }
        setData((d) => ({ ...d, [part]: { ...d[part], status: 'error', error: daemonErrorText(e), waiting: false } }))
        return
      }
    }
  }, [])

  const refresh = useCallback(async (): Promise<void> => {
    await Promise.all([reload('users'), reload('grants'), reload('roles')])
  }, [reload])

  useEffect(() => {
    live.current = true
    void refresh()
    return () => {
      live.current = false
    }
  }, [refresh])

  // App names: once someone has an App, or the page asks (a form, By App).
  const hasApps = (data.grants.data ?? []).some((g) => g.kind === 'app' && g.targetId !== HOST_APP)
  const appsAsked = data.apps.status !== 'loading' || data.apps.data != null || seq.current.apps > 0
  useEffect(() => {
    if ((wantApps || hasApps) && !appsAsked) void reload('apps')
  }, [wantApps, hasApps, appsAsked, reload])

  // Reads that waited for a reconnect run again once the server is back.
  useEffect(() => {
    if (waiting.size === 0) return
    const retry = (): void => {
      const parts = [...waiting]
      setWaiting(new Set())
      for (const p of parts) void reload(p)
    }
    if (useConnectHostStore.getState().recovery.kind === 'connected') {
      const t = setTimeout(retry, 1500)
      return () => clearTimeout(t)
    }
    return useConnectHostStore.subscribe((s) => {
      if (s.recovery.kind === 'connected') retry()
    })
  }, [waiting, reload])

  return { ...data, reload, refresh }
}
