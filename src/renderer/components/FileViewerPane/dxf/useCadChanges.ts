// The live change highlight's state. When the file gets a new version from
// any writer, ask what changed since the version before it; versions that
// follow each other within a few seconds join one run, asked for as one range
// (so "Atlas made 3 changes"). The outlines show for about 2 s and then fade
// (removed at once under reduced motion); the note stays until the next run
// or until dismissed. Undo reverts the note's versions, newest first, and
// stops at the first refusal, keeping its words.

import { useCallback, useEffect, useRef, useState } from 'react'
import { useReducedMotion } from '@/hooks/useReducedMotion'
import type { ServerScope } from '@/kessel/server-scope'
import type { PresenceSlot } from '@/lib/live-view/shared/presence-slots'
import { onDocVersionChanged } from '@/stores/session-events'
import { docEventFilePath } from '../useDocVersion'
import type { CadChange } from './cad-change'
import { changeAuthors, changeSlots } from './cad-change-marks'
import type { CadChangeSource } from './cad-change-source'

/** How long the outlines hold before they fade. */
export const HOLD_MS = 2000
/** Past the slow motion token (300 ms at most), so the fade has finished. */
export const FADE_MS = 400
/** A version this soon after the last joins its run. */
export const RUN_MS = 10_000

export type UndoState = { kind: 'idle' } | { kind: 'busy' } | { kind: 'done' } | { kind: 'refused'; words: string }

export type CadChanges = {
  change: CadChange | null
  /** The outlines: shown, fading, or gone. */
  phase: 'on' | 'fading' | null
  slots: ReadonlyMap<string, PresenceSlot>
  undoState: UndoState
  undo: (versions: readonly number[]) => void
  dismiss: () => void
}

export function useCadChanges(source: CadChangeSource | null, scope: ServerScope | null, path: string | null): CadChanges {
  const reduced = useReducedMotion()
  const [change, setChange] = useState<CadChange | null>(null)
  const [phase, setPhase] = useState<'on' | 'fading' | null>(null)
  const [lit, setLit] = useState(0)
  const [slots, setSlots] = useState<ReadonlyMap<string, PresenceSlot>>(new Map())
  const [undoState, setUndoState] = useState<UndoState>({ kind: 'idle' })
  const run = useRef<{ from: number; to: number; at: number } | null>(null)
  const seen = useRef<number | null>(null)
  const seq = useRef(0)
  const authors = useRef<string[]>([])

  useEffect(() => {
    run.current = null
    seen.current = null
    setChange(null)
    setPhase(null)
    if (!source || !scope || !path) return
    return onDocVersionChanged(scope, (e) => {
      if (docEventFilePath(e) !== path || e.version === seen.current) return
      seen.current = e.version
      const now = Date.now()
      const r = run.current
      const from = r && e.version === r.to + 1 && now - r.at < RUN_MS ? r.from : e.version - 1
      const mine = ++seq.current
      void source.change(path, from, e.version).then((c) => {
        if (mine !== seq.current || !c) return
        run.current = { from, to: e.version, at: Date.now() }
        // Everyone seen while the drawing is open keeps their colour.
        for (const a of changeAuthors(c)) if (!authors.current.includes(a)) authors.current.push(a)
        setSlots((prev) => changeSlots(prev, authors.current))
        setChange(c)
        setUndoState({ kind: 'idle' })
        setPhase('on')
        setLit((n) => n + 1)
      })
    })
  }, [source, scope, path])

  useEffect(() => {
    if (!lit) return
    let fade: ReturnType<typeof setTimeout> | undefined
    const hold = setTimeout(() => {
      if (reduced) return setPhase(null)
      setPhase('fading')
      fade = setTimeout(() => setPhase(null), FADE_MS)
    }, HOLD_MS)
    return () => {
      clearTimeout(hold)
      if (fade) clearTimeout(fade)
    }
    // The reduced-motion setting is read when the hold ends.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [lit])

  const undo = useCallback(
    (versions: readonly number[]) => {
      if (!source || !path || !versions.length) return
      setUndoState({ kind: 'busy' })
      void (async () => {
        for (const v of versions) {
          const r = await source.revert(path, v)
          if (!r.ok) return setUndoState({ kind: 'refused', words: r.words })
        }
        setUndoState({ kind: 'done' })
      })()
    },
    [source, path],
  )

  const dismiss = useCallback(() => {
    run.current = null
    setChange(null)
    setPhase(null)
  }, [])

  return { change, phase, slots, undoState, undo, dismiss }
}
