// The text from before the last outside save, for Compare and Undo their save
// (wire-shapes: every outside_save carries `before_version`). The text this
// connection saw before the save's external change comes first; a tab that
// only got the save replayed after its welcome reads the stored version
// instead (`GET /cli/docs/show?version=<before_version>`, `{text}`). One
// fetch per save; a failed one can be tried again. Missing and declined saves
// have no version and nothing to fetch.

import { useCallback, useEffect, useRef, useState } from 'react'
import type { ServerScope } from '@/kessel/server-scope'
import { daemonCliGet } from '@/lib/daemon-cli'
import type { LiveDocSnapshot } from '@/lib/editor/live/live-doc-source'

export type BeforeStatus = 'idle' | 'loading' | 'ready' | 'failed'

export interface BeforeText {
  /** The earlier text, when it's here (seen on this connection, or fetched). */
  text: string | null
  status: BeforeStatus
  /** There is a way to get it: seen here, or a stored version to read. */
  available: boolean
  /** The text, fetching it if needed; null when it can't be had. */
  get: () => Promise<string | null>
}

export function useBeforeText(scope: ServerScope, filePath: string, snap: LiveDocSnapshot): BeforeText {
  const seen = snap.beforeExternal
  const version = snap.outsideSave?.outcome === 'ingested' ? snap.beforeVersion : null
  const key = `${snap.outsideSave?.at_ms ?? ''}|${version ?? ''}`
  const [state, setState] = useState<{ key: string; status: BeforeStatus; text: string | null }>({ key, status: 'idle', text: null })
  const cur = state.key === key ? state : { key, status: 'idle' as const, text: null }
  useEffect(() => setState((s) => (s.key === key ? s : { key, status: 'idle', text: null })), [key])
  const inflight = useRef<{ key: string; p: Promise<string | null> } | null>(null)

  const get = useCallback(async (): Promise<string | null> => {
    if (seen !== null) return seen
    if (version === null) return null
    if (cur.status === 'ready') return cur.text
    if (inflight.current?.key === key) return inflight.current.p
    setState({ key, status: 'loading', text: null })
    const p = daemonCliGet<{ text?: unknown }>(scope, 'docs/show', { path: filePath, version }).then(
      (r) => {
        const text = typeof r?.text === 'string' ? r.text : null
        setState({ key, status: text === null ? 'failed' : 'ready', text })
        return text
      },
      () => {
        setState({ key, status: 'failed', text: null })
        return null
      },
    )
    inflight.current = { key, p }
    void p.finally(() => {
      if (inflight.current?.p === p) inflight.current = null
    })
    return p
  }, [seen, version, cur.status, cur.text, key, scope, filePath])

  return { text: seen ?? (cur.status === 'ready' ? cur.text : null), status: seen !== null ? 'ready' : cur.status, available: seen !== null || version !== null, get }
}
