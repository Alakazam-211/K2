// The viewer's edit state: what is selected, whether the drawing can be
// changed here (and why not), agents' holds, and the two edits (a move and
// an attribute's value). Every edit runs against the version on screen; one
// that lands shows at once in the change highlight as yours, and the drawing
// is read again; further edits wait until that version is on screen, so
// your own last change never reads as someone else's. Before an edit that
// touches an agent's hold the viewer asks once; a refusal comes back in
// plain words and, when the drawing changed under you, the drawing is read
// again.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { CadChange } from '../cad-change'
import type { ModelHeader, ModelRow } from '../model/types'
import { attribArgv, moveArgv, runEdit } from './edit-flow'
import type { EditSource, Hold } from './edit-source'
import { refusalWords, viewOnlyWhy } from './edit-words'
import { holdPrompt, holdsOn } from './holds'
import { ownChange } from './own-change'
import { editReason, pickable } from './selection'

export type EditDrawing = { header: ModelHeader; rows: ModelRow[]; version: number }

export type EditPrompt = { text: string; verb: 'move' | 'relabel'; yes: () => void; no: () => void }

/** How often holds are read again while the drawing is open. */
export const HOLDS_EVERY_MS = 15_000
/** The longest edits wait for your own version to come on screen. */
export const PENDING_MS = 5000

export type CadEdit = {
  rows: ModelRow[]
  byHandle: ReadonlyMap<string, ModelRow>
  selection: string[]
  select: (handles: readonly string[]) => void
  /** The drawing can be changed here (edit controls show). */
  canEdit: boolean
  /** ...and now: not while your last edit's version is still on its way. */
  editable: boolean
  /** Why the drawing can't be changed here at all, or null. */
  viewOnly: string | null
  /** The drawing's unit word for the verbs (`drawing` when it has none). */
  unit: string
  holds: Hold[]
  /** Whether these handles can all be moved (none has a reason not to). */
  canMove: (handles: readonly string[]) => boolean
  busy: boolean
  message: string | null
  prompt: EditPrompt | null
  move: (dx: number, dy: number) => Promise<boolean>
  relabel: (insert: string, tag: string, value: string, nth: number | null) => Promise<boolean>
}

export function useCadEdit(p: {
  drawing: EditDrawing | null
  filePath: string | undefined
  source: EditSource | null
  showOwn: (c: CadChange) => void
  reload: () => void
  meet: (pids: readonly string[]) => void
}): CadEdit {
  const { drawing, filePath, source } = p
  const [selection, setSelection] = useState<string[]>([])
  const [holds, setHolds] = useState<Hold[]>([])
  const [busy, setBusy] = useState(false)
  const [message, setMessage] = useState<string | null>(null)
  const [prompt, setPrompt] = useState<EditPrompt | null>(null)
  const [off, setOff] = useState<string | null>(null)
  const [pending, setPending] = useState<number | null>(null)
  const cb = useRef(p)
  cb.current = p

  const rows = useMemo(() => (drawing ? pickable(drawing.rows) : []), [drawing])
  const byHandle = useMemo(() => new Map((drawing?.rows ?? []).map((r) => [r.h, r])), [drawing])
  const viewOnly = useMemo(() => (drawing ? (viewOnlyWhy(drawing.header, drawing.rows) ?? off) : null), [drawing, off])
  const waiting = pending !== null && (drawing?.version ?? 0) < pending
  const canEdit = !!drawing && !!source?.canWrite && !!filePath && !viewOnly
  const editable = canEdit && !waiting
  const unit = drawing?.header.units?.word || 'drawing'

  // A new file starts afresh.
  useEffect(() => {
    setSelection([])
    setMessage(null)
    setOff(null)
    setPending(null)
  }, [filePath])

  // What's still in the drawing stays selected when it's read again.
  useEffect(() => {
    setSelection((sel) => {
      const keep = sel.filter((h) => byHandle.get(h)?.visible === true)
      return keep.length === sel.length ? sel : keep
    })
  }, [byHandle])

  useEffect(() => {
    if (pending === null) return
    if (!waiting) return setPending(null)
    const t = setTimeout(() => setPending(null), PENDING_MS)
    return () => clearTimeout(t)
  }, [pending, waiting])

  const refreshHolds = useCallback(async (): Promise<Hold[]> => {
    if (!source || !filePath) return []
    const h = await source.holds(filePath)
    setHolds(h)
    cb.current.meet([...new Set(h.map((x) => x.pid))])
    return h
  }, [source, filePath])

  const version = drawing?.version
  useEffect(() => {
    if (!drawing) return
    void refreshHolds()
    const t = setInterval(() => void refreshHolds(), HOLDS_EVERY_MS)
    return () => clearInterval(t)
    // Read again on each version, not on each read of the same one.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [refreshHolds, version, !!drawing])

  const select = useCallback((handles: readonly string[]) => {
    setSelection([...handles])
    setMessage(null)
  }, [])

  const canMove = useCallback(
    (handles: readonly string[]) => !!drawing && handles.length > 0 && handles.every((h) => byHandle.has(h) && editReason(byHandle.get(h)!, drawing.header) === null),
    [drawing, byHandle],
  )

  /** Ask once when an edit touches agents' holds; true to go ahead. */
  const overHolds = useCallback(
    async (handles: readonly string[], verb: 'move' | 'relabel'): Promise<boolean> => {
      if (!drawing) return false
      const on = holdsOn(handles, await refreshHolds(), drawing.rows, drawing.header)
      if (!on.length) return true
      return new Promise<boolean>((resolve) => {
        const done = (v: boolean) => () => {
          setPrompt(null)
          resolve(v)
        }
        setPrompt({ text: holdPrompt(on, handles.length, verb), verb, yes: done(true), no: done(false) })
      })
    },
    [drawing, refreshHolds],
  )

  const run = useCallback(
    async (argv: string[], handles: readonly string[], verb: 'move' | 'relabel'): Promise<boolean> => {
      if (!editable || !source || !filePath || !drawing || busy) return false
      setMessage(null)
      if (!(await overHolds(handles, verb))) return false
      setBusy(true)
      try {
        const out = await runEdit(source, filePath, argv, drawing.version)
        if (out.kind === 'unchanged') return true
        if (out.kind === 'done') {
          const c = ownChange(out.result)
          if (c) cb.current.showOwn(c)
          setPending(out.result.after)
          cb.current.reload()
          return true
        }
        const w = refusalWords(out.result, { verb, layerOf: (a) => byHandle.get(a.toUpperCase().replace(/^@H:/, '@h:'))?.layer ?? null })
        setMessage(w.words)
        if (w.off) setOff(w.off)
        if (w.refresh) cb.current.reload()
        return false
      } finally {
        setBusy(false)
      }
    },
    [editable, source, filePath, drawing, busy, overHolds, byHandle],
  )

  const move = useCallback(
    (dx: number, dy: number) => (dx || dy ? run(moveArgv(selection, dx, dy, unit), selection, 'move') : Promise.resolve(false)),
    [run, selection, unit],
  )
  const relabel = useCallback(
    (insert: string, tag: string, value: string, nth: number | null) => run(attribArgv(insert, tag, value, nth), [insert], 'relabel'),
    [run],
  )

  return { rows, byHandle, selection, select, canEdit, editable, viewOnly, unit, holds, canMove, busy, message, prompt, move, relabel }
}
