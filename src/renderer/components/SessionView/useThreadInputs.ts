import { useCallback, useEffect, useRef, useState } from 'react'
import { daemonCliGet } from '@/lib/daemon-cli'
import { asArray } from '@/lib/as-array'
import type { ServerScope } from '@/kessel/server-scope'
import {
  HEARTBEAT_FIRES_LIMIT,
  HEARTBEAT_FIRES_REFRESH_MS,
  heartbeatInputRows,
  newestFirst,
  type HeartbeatFireRow,
  type ThreadInputRow,
} from './threadPanels'

export interface ThreadInputs {
  rows: ThreadInputRow[]
  /** The first read hasn't answered yet. */
  loading: boolean
  error: string | null
  refresh: () => void
}

/** The workspace's recent outside input, newest first. Today that's its
 *  heartbeat fires (`/cli/heartbeat/fires-list`, the route Settings →
 *  Heartbeats reads). There's no stream for fires, so it reads on open and
 *  again every 15 s while open. */
export function useThreadInputs(opts: {
  scope: ServerScope
  /** The workspace's path. Empty: nothing to read. */
  projectPath: string
  enabled: boolean
}): ThreadInputs {
  const { scope, projectPath, enabled } = opts
  const [rows, setRows] = useState<ThreadInputRow[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const generation = useRef(0)

  const read = useCallback(async (): Promise<void> => {
    const mine = ++generation.current
    try {
      const fires = asArray<HeartbeatFireRow>(
        await daemonCliGet<unknown>(scope, 'heartbeat/fires-list', {
          project: projectPath,
          limit: HEARTBEAT_FIRES_LIMIT,
        }),
      )
      if (mine !== generation.current) return
      setRows(newestFirst(heartbeatInputRows(fires)))
      setError(null)
    } catch (e) {
      if (mine !== generation.current) return
      setError(e instanceof Error && e.message ? e.message : String(e))
    } finally {
      if (mine === generation.current) setLoading(false)
    }
  }, [scope, projectPath])

  useEffect(() => {
    if (!enabled || !projectPath.trim()) {
      generation.current++
      setRows([])
      setError(null)
      setLoading(!!projectPath.trim())
      return
    }
    setLoading(true)
    void read()
    const timer = setInterval(() => void read(), HEARTBEAT_FIRES_REFRESH_MS)
    return () => {
      clearInterval(timer)
      generation.current++
    }
  }, [enabled, projectPath, read])

  const refresh = useCallback(() => void read(), [read])
  return { rows, loading: loading && !!projectPath.trim(), error, refresh }
}
