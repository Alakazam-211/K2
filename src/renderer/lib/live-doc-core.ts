// Live docs: whether a file opens in the Live editor, from the daemon.
//
// A file opens live when its multiplayer core is on (Settings → Cores →
// Multiplayer) and that core has a live editor for people (Markdown, Text
// and code). Any other file gets the any-file panel instead, beside its usual
// view (`AnyFileDocPanel`). The daemon decides both by the file's name:
// `GET /cli/docs/cores?path=P` answers `file: {core, on, liveView}`, so no
// file-name rule lives here. It follows `doc_cores_changed` on the same
// server, so a switch flipped anywhere (another window, the CLI, an Admin)
// takes effect in an open tab.

import { useCallback, useEffect, useState } from 'react'
import { daemonCliGet } from '@/lib/daemon-cli'
import type { ServerScope } from '@/kessel/server-scope'
import { onDocCoresChanged } from '@/stores/session-events'

/** The daemon's answer for one file. */
export interface LiveDocCore {
  /** `markdown`, `text` or `any-file`. */
  core: string
  /** The core is on for this server. */
  on: boolean
  /** The core has a live editor for people (Markdown, Text and code). */
  liveView: boolean
}

/** Parse `file` from `GET /cli/docs/cores?path=`; loud on a shape we don't know. */
export function parseLiveDocCore(body: unknown): LiveDocCore {
  const file = body && typeof body === 'object' ? (body as Record<string, unknown>).file : undefined
  if (!file || typeof file !== 'object') throw new Error('docs/cores: no `file` for the path')
  const f = file as Record<string, unknown>
  if (typeof f.core !== 'string' || typeof f.on !== 'boolean' || typeof f.liveView !== 'boolean') {
    throw new Error('docs/cores: `file` is not {core, on, liveView}')
  }
  return { core: f.core, on: f.on, liveView: f.liveView }
}

/** Whether `path` opens in the Live editor on `scope`'s server. */
export async function fetchLiveDocCore(scope: ServerScope, path: string): Promise<LiveDocCore> {
  return parseLiveDocCore(await daemonCliGet<unknown>(scope, 'docs/cores', { path }))
}

/** Whether a server has the multiplayer switches (and with them the Live
 *  socket, 0.46.7). An older daemon: never live. `docs-live-v1` isn't
 *  asked: it is listed only while a core is on, and the window's feature
 *  list can predate the switch; `cores?path=` says whether the core is on
 *  now, and turning one on starts the docs runtime. */
export function liveDocsAvailable(scope: ServerScope): boolean {
  return scope.serverSupports('docs-cores-v1')
}

export type LiveDocCoreState =
  | { status: 'off' } // today's view: the core is off, has no live editor, or the server can't
  | { status: 'checking' }
  | { status: 'live'; core: string }
  | { status: 'any-file' } // today's view, with the any-file panel beside it

function stateFor(c: LiveDocCore): LiveDocCoreState {
  if (!c.on) return { status: 'off' }
  if (c.liveView) return { status: 'live', core: c.core }
  // Any other file has no live editor but does have the panel. The Board
  // (also no live editor) opens as its own thing.
  return c.core === 'any-file' ? { status: 'any-file' } : { status: 'off' }
}

/**
 * The Live view's switch for one open file: `live` while its core is on,
 * `any-file` while the "Any other file" core is on for a file with no live
 * editor, `off` otherwise (and on any error: the file then opens as today). Re-asks
 * on `doc_cores_changed` and when `epoch` changes (the tab asks again after
 * the socket said the core went off).
 */
export function useLiveDocCore(scope: ServerScope, path: string, epoch = 0): LiveDocCoreState {
  const available = liveDocsAvailable(scope)
  const [state, setState] = useState<LiveDocCoreState>(available ? { status: 'checking' } : { status: 'off' })
  const check = useCallback(
    async (isCurrent: () => boolean) => {
      if (!available || !path) {
        setState({ status: 'off' })
        return
      }
      try {
        const c = await fetchLiveDocCore(scope, path)
        if (isCurrent()) setState(stateFor(c))
      } catch {
        if (isCurrent()) setState({ status: 'off' })
      }
    },
    [scope, path, available],
  )
  useEffect(() => {
    let current = true
    const isCurrent = () => current
    void check(isCurrent)
    const off = available ? onDocCoresChanged(scope, () => void check(isCurrent)) : () => {}
    return () => {
      current = false
      off()
    }
  }, [scope, check, available, epoch])
  return state
}
