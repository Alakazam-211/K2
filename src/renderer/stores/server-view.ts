// Home M4 — another server's presence roster and Active set, per server
// (prd-home-multi-server-client MS14 "presence: per server", "active: per
// server"; R7 presence both ways).
//
// The window's own server keeps its singletons (`usePresenceStore`,
// `useActiveStore`), fed by the window's app socket. A pinned room on server
// B reads B's: this module keeps one small store pair per server, fed by B's
// app bus (`onPresenceChanged(scope)`, `onActiveChanged(scope)`), which the
// room's own workspace socket carries (`carryAppBus` in
// `stores/session-events.ts`) — no extra socket. On every carrier hello, and
// when the first room on B acquires it, it snapshots `presence/roster` and
// `projects/active` from B.
//
// Reference-counted by rooms: the last release drops every bus handler, so
// an unmounted room leaves 0 handlers on B's bus (MS52 j).

import { createStore, type StoreApi } from 'zustand/vanilla'
import { daemonCliGet } from '@/lib/daemon-cli'
import { onActiveChanged, onAppHello, onPresenceChanged, type PresenceRosterUser } from '@/stores/session-events'
import type { ServerScope } from '@/kessel/server-scope'

export interface PresenceView {
  /** That server's live roster (whole-set replace, daemon order). */
  roster: PresenceRosterUser[]
  /** False once that server proved it lacks the presence routes. */
  supported: boolean
}

export interface ActiveView {
  /** That server's canonical Active set. */
  activeProjectIds: Set<string>
}

export type PresenceViewStore = Pick<StoreApi<PresenceView>, 'getState' | 'getInitialState' | 'subscribe'>
export type ActiveViewStore = Pick<StoreApi<ActiveView>, 'getState' | 'getInitialState' | 'subscribe'>

export interface ServerView {
  readonly hostKey: string
  readonly presence: PresenceViewStore
  readonly active: ActiveViewStore
  /** Re-read both snapshots from that server. */
  refresh(): Promise<void>
}

interface Entry {
  view: ServerView
  refs: number
  unsubs: Array<() => void>
}

const entries = new Map<string, Entry>()

function makeView(scope: ServerScope): { view: ServerView; unsubs: Array<() => void> } {
  const presence = createStore<PresenceView>(() => ({ roster: [], supported: true }))
  const active = createStore<ActiveView>(() => ({ activeProjectIds: new Set<string>() }))

  const refreshRoster = async (): Promise<void> => {
    try {
      const snap = await daemonCliGet<{ roster: PresenceRosterUser[] }>(scope, 'presence/roster')
      presence.setState({ roster: Array.isArray(snap?.roster) ? snap.roster : [], supported: true })
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err)
      if (msg.includes('route not found')) presence.setState({ roster: [], supported: false })
      else console.debug(`[server-view] ${scope.hostKey} roster snapshot skipped:`, err)
    }
  }
  const refreshActive = async (): Promise<void> => {
    if (!scope.serverSupports('canonical-active')) return
    try {
      const snap = await daemonCliGet<{ projectIds: string[] }>(scope, 'projects/active')
      active.setState({ activeProjectIds: new Set(Array.isArray(snap?.projectIds) ? snap.projectIds : []) })
    } catch (err) {
      console.debug(`[server-view] ${scope.hostKey} Active snapshot skipped:`, err)
    }
  }
  const view: ServerView = {
    hostKey: scope.hostKey,
    presence,
    active,
    refresh: async () => {
      await Promise.all([refreshRoster(), refreshActive()])
    },
  }
  const unsubs = [
    onPresenceChanged(scope, (e) => {
      presence.setState({ roster: Array.isArray(e.roster) ? e.roster : [], supported: true })
    }),
    onActiveChanged(scope, (e) => {
      active.setState({ activeProjectIds: new Set(Array.isArray(e.activeProjectIds) ? e.activeProjectIds : []) })
    }),
    onAppHello(scope, () => {
      void view.refresh()
    }),
  ]
  return { view, unsubs }
}

/** Take a reference on `scope`'s server view. Release it exactly once. */
export function acquireServerView(scope: ServerScope): { view: ServerView; release: () => void } {
  if (scope.isPrimary) {
    throw new Error('acquireServerView: the window’s own server uses usePresenceStore / useActiveStore')
  }
  let entry = entries.get(scope.id)
  if (!entry) {
    const made = makeView(scope)
    entry = { view: made.view, refs: 0, unsubs: made.unsubs }
    entries.set(scope.id, entry)
    void made.view.refresh()
  }
  entry.refs += 1
  const held = entry
  let released = false
  return {
    view: held.view,
    release: () => {
      if (released) return
      released = true
      held.refs -= 1
      if (held.refs > 0) return
      for (const u of held.unsubs) u()
      if (entries.get(scope.id) === held) entries.delete(scope.id)
    },
  }
}

/** Test seam: live server views (by scope id). */
export function serverViewCountForTests(): number {
  return entries.size
}
