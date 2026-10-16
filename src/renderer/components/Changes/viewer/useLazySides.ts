// The view-all data hook: one row's sides through C2's `loadSides`, only
// once the row's section is near the viewport (`enabled`), reloaded when
// the snapshot `rev` moves. The old diff stays painted while the reload
// runs (no flash), and an answer that arrives after the row, base or rev
// moved on is dropped.

import { useCallback, useEffect, useRef, useState } from 'react'
import { classifyDiff, type DiffView } from './diffQuery'
import { diffRowKey, type ChangesDiffFile, type DiffSidesResponse } from './types'

interface Loaded {
  key: string
  view: DiffView
}

export function useLazySides(opts: {
  row: ChangesDiffFile
  base: string | null
  rev: string | null
  enabled: boolean
  loadSides: (row: ChangesDiffFile) => Promise<DiffSidesResponse>
}): DiffView & { reload: () => void } {
  const { row, base, rev, enabled, loadSides } = opts
  // Everything that changes which diff this is (rev only decides when).
  const key = `${diffRowKey(row)}\u0000${row.oldPath ?? ''}\u0000${base ?? ''}`
  const [loaded, setLoaded] = useState<Loaded | null>(null)
  const [pending, setPending] = useState(false)
  const [nonce, setNonce] = useState(0)
  const seq = useRef(0)
  const rowRef = useRef(row)
  rowRef.current = row
  const loadRef = useRef(loadSides)
  loadRef.current = loadSides

  useEffect(() => {
    if (!enabled) return
    if (rowRef.current.dir) {
      setLoaded({ key, view: { loading: false, refreshing: false, content: { kind: 'dir' }, error: null } })
      return
    }
    const my = ++seq.current
    setPending(true)
    loadRef.current(rowRef.current).then(
      (res) => {
        if (my !== seq.current) return
        let view: DiffView
        try {
          view = { loading: false, refreshing: false, content: classifyDiff(rowRef.current, res), error: null }
        } catch (e) {
          view = { loading: false, refreshing: false, content: null, error: e instanceof Error ? e.message : String(e) }
        }
        setLoaded({ key, view })
        setPending(false)
      },
      (e: unknown) => {
        if (my !== seq.current) return
        setLoaded({ key, view: { loading: false, refreshing: false, content: null, error: e instanceof Error ? e.message : String(e) } })
        setPending(false)
      },
    )
  }, [key, rev, enabled, nonce])

  // Unmount: drop any answer still in flight.
  useEffect(() => () => void ++seq.current, [])

  const reload = useCallback(() => setNonce((n) => n + 1), [])
  if (!loaded || loaded.key !== key) {
    return { loading: true, refreshing: false, content: null, error: null, reload }
  }
  return { ...loaded.view, refreshing: pending, reload }
}
