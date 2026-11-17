// The live change highlight's state. When the file gets a new version from
// any writer, ask what changed since the version before it; versions that
// follow each other within a few seconds join one run, asked for as one range
// (so "Atlas made 3 changes"). The outlines show for about 2 s and then fade
// (removed at once under reduced motion); the note stays until the next run
// or until dismissed. Undo reverts the note's versions, newest first, and
// stops at the first refusal, keeping its words. Your own edit is shown the
// moment it lands, from its result, and its version isn't asked about again.
// A server with no change feed answers no change: the drawing is read again
// and, when the version names who, the note says "Atlas changed the drawing".

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
  /** The newest version seen of the file (from any writer, you too), or null before any. */
  head: number | null
  /** Who changed the drawing, when the server can't say what changed: "Atlas changed the drawing". */
  quiet: string | null
  /** The drawing's digest after your own latest change (what Undo sends), or null. */
  digest: string | null
  /** The outlines: shown, fading, or gone. */
  phase: 'on' | 'fading' | null
  slots: ReadonlyMap<string, PresenceSlot>
  undoState: UndoState
  undo: (versions: readonly number[], expectDigest: string) => void
  dismiss: () => void
  /** Show your own landed edit at once, with the drawing's digest after it (its version's event is then not asked about again). */
  showOwn: (c: CadChange, digest?: string | null) => void
  /** Give these people presence colours now (an agent holding something before it changes anything). */
  meet: (pids: readonly string[]) => void
}

/** Who a pid is on this file: their name, and whether it's you. */
export type WhoIs = (pid: string) => Promise<{ name: string | null; self: boolean }>

/** The quiet line for someone else's version, when the server can't say what changed. */
export const SOMEONE_CHANGED = 'The drawing was changed'

export function useCadChanges(
  source: CadChangeSource | null,
  scope: ServerScope | null,
  path: string | null,
  hooks: {
    /** A new version of the file arrived (not your own), or an undo landed or found the drawing moved: read it again. */
    onVersion?: (version: number | null) => void
    /** Undo found the drawing can't be changed here: open it view only, with these words. */
    onOff?: (words: string) => void
    /** A version's author (its pid) as a name, from who's on the file. */
    whoIs?: WhoIs
  } = {},
): CadChanges {
  const reduced = useReducedMotion()
  const [change, setChange] = useState<CadChange | null>(null)
  const [phase, setPhase] = useState<'on' | 'fading' | null>(null)
  const [lit, setLit] = useState(0)
  const [slots, setSlots] = useState<ReadonlyMap<string, PresenceSlot>>(new Map())
  const [undoState, setUndoState] = useState<UndoState>({ kind: 'idle' })
  const [head, setHead] = useState<number | null>(null)
  const [quiet, setQuiet] = useState<string | null>(null)
  const [digest, setDigest] = useState<string | null>(null)
  const cbs = useRef(hooks)
  cbs.current = hooks
  const run = useRef<{ from: number; to: number; at: number } | null>(null)
  const seen = useRef<number | null>(null)
  const seq = useRef(0)
  const authors = useRef<string[]>([])
  /** Versions you made here, and your pid once a version's event names it. */
  const mine = useRef(new Set<number>())
  const selfPid = useRef<string | null>(null)

  useEffect(() => {
    run.current = null
    seen.current = null
    setChange(null)
    setPhase(null)
    setHead(null)
    setQuiet(null)
    if (!source || !scope || !path) return
    return onDocVersionChanged(scope, (e) => {
      if (docEventFilePath(e) !== path) return
      setHead((h) => (h === null || e.version > h ? e.version : h))
      const by = typeof e.by === 'string' && e.by ? e.by : null
      if (by && mine.current.has(e.version)) selfPid.current = by
      if (e.version === seen.current) return
      seen.current = e.version
      cbs.current.onVersion?.(e.version)
      const now = Date.now()
      const r = run.current
      const from = r && e.version === r.to + 1 && now - r.at < RUN_MS ? r.from : e.version - 1
      const ask = ++seq.current
      void source.change(path, from, e.version).then(async (c) => {
        if (ask !== seq.current) return
        // No change feed on this server: the drawing is read again and, when the version says who (not you), a quiet line names them.
        if (!c) {
          if (!by || by === selfPid.current) return
          const who = (await cbs.current.whoIs?.(by)) ?? { name: null, self: false }
          if (ask !== seq.current || who.self) return
          setChange(null)
          setPhase(null)
          setQuiet(who.name ? `${who.name} changed the drawing` : SOMEONE_CHANGED)
          return
        }
        setQuiet(null)
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

  const showOwnRef = useRef<(c: CadChange, digest?: string | null) => void>(() => {})
  const undo = useCallback(
    (versions: readonly number[], expectDigest: string) => {
      if (!source || !path || !versions.length) return
      setUndoState({ kind: 'busy' })
      void (async () => {
        // Undo-last: one version, the head.
        const r = await source.revert(path, versions[0], expectDigest)
        if (!r.ok) {
          setUndoState({ kind: 'refused', words: r.words })
          if (r.off) cbs.current.onOff?.(r.off)
          if (r.reload) cbs.current.onVersion?.(null)
          return
        }
        if (r.change) showOwnRef.current(r.change, null)
        else setUndoState({ kind: 'done' })
        cbs.current.onVersion?.(r.change?.to ?? null)
      })()
    },
    [source, path],
  )

  const showOwn = useCallback((c: CadChange, after: string | null = null) => {
    mine.current.add(c.to)
    setDigest(after)
    seq.current++
    seen.current = c.to
    setHead((h) => (h === null || c.to > h ? c.to : h))
    setQuiet(null)
    // Your own version starts no run: the next version by anyone is told on its own.
    run.current = null
    setChange(c)
    setUndoState({ kind: 'idle' })
    setPhase('on')
    setLit((n) => n + 1)
  }, [])

  showOwnRef.current = showOwn

  const meet = useCallback((pids: readonly string[]) => {
    const fresh = pids.filter((a) => !authors.current.includes(a))
    if (!fresh.length) return
    authors.current.push(...fresh)
    setSlots((prev) => changeSlots(prev, authors.current))
  }, [])

  const dismiss = useCallback(() => {
    run.current = null
    setChange(null)
    setPhase(null)
    setQuiet(null)
  }, [])

  return { change, head, quiet, digest, phase, slots, undoState, undo, dismiss, showOwn, meet }
}
