// The old background Claude token refresher: a scheduled job on the server
// (a launchd agent on macOS, a crontab line on Linux) that refreshed the
// server default Claude token every 20 minutes. It can no longer be turned
// on, since Claude refreshes its own token. Anyone who turned it on keeps a
// way off: `/cli/claude-auth/status` says whether it is still installed
// (`schedulerInstalled`) and `/cli/claude-auth/uninstall-scheduler` removes
// it. The server is the truth; this only reads it and sends the gesture.

import { useCallback, useEffect, useRef, useState } from 'react'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import type { ServerScope } from '@/kessel/server-scope'

interface ClaudeAuthStatus {
  schedulerInstalled?: boolean
}

export interface ClaudeRefresher {
  /** The server reports the refresher installed. False until it says so,
   *  and when the status can't be read. */
  installed: boolean
  /** A Turn off is on its way. */
  busy: boolean
  /** Remove it on the server, then read the status again. Throws when the
   *  server refuses, or still reports it installed. */
  turnOff: () => Promise<void>
}

async function readInstalled(scope: ServerScope): Promise<boolean> {
  const status = await daemonCliGet<ClaudeAuthStatus>(scope, 'claude-auth/status')
  return status?.schedulerInstalled === true
}

/** Read on open and again on a server switch (the window's scope keeps its
 *  identity across a switch; its connection key changes). */
export function useClaudeRefresher(scope: ServerScope): ClaudeRefresher {
  const conn = scope.connectionKey
  const [installed, setInstalled] = useState(false)
  const [busy, setBusy] = useState(false)
  // The connection the answers belong to, so a switch drops a late one.
  const current = useRef(conn)
  current.current = conn

  useEffect(() => {
    setInstalled(false)
    setBusy(false)
    let live = true
    readInstalled(scope).then(
      (v) => {
        if (live) setInstalled(v)
      },
      (e: unknown) => console.debug('[claude-refresher] status:', e),
    )
    return () => {
      live = false
    }
  }, [scope, conn])

  const turnOff = useCallback(async () => {
    setBusy(true)
    try {
      await daemonCliPost(scope, 'claude-auth/uninstall-scheduler', {})
      const still = await readInstalled(scope)
      if (current.current === conn) setInstalled(still)
      if (still) throw new Error('it is still installed')
    } finally {
      if (current.current === conn) setBusy(false)
    }
  }, [scope, conn])

  return { installed, busy, turnOff }
}
