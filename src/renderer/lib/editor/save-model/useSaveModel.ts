// Saving a plain file in the editor frame (build-editors.md E1c): the
// reducer's phases driven by real writes.
//   - Autosave 800 ms after typing stops, on blur, and (FileViewerPane's
//     leave flush, `leaveWrite`) when the tab closes. ⌘S saves now.
//   - Every write sends `expect`. A 409 with the file's text merges three
//     ways (merge3) and says so after the fact; one without it (an App
//     guest) or a missing file stops autosave and asks.
//   - The buffer is kept on this computer as a draft until a save lands, and
//     comes back after a restart with the restore bar.
// Read-only and locked files never write.

import { useCallback, useEffect, useReducer, useRef, useState } from 'react'
import { daemonCliGet, isHostSwitchedError } from '@/lib/daemon-cli'
import type { ServerScope } from '@/kessel/server-scope'
import { clearDraft, getDraft, putDraft, type Draft } from './drafts'
import { merge3, type KeptPair } from './merge3'
import { copyPath, saveNoticeView, type SaveActionId, type SaveNotice, type SaveNoticeView } from './notices'
import { canAutosave, INITIAL_SAVE, saveReducer, saveStatus, type SaveState, type SaveStatus } from './reducer'
import { failureReason, writeChecked } from './write'

/** Typing stops this long before autosave writes. */
export const AUTOSAVE_IDLE_MS = 800
/** The draft is written this long after the last change. */
export const DRAFT_IDLE_MS = 250

export interface SaveModelOptions {
  scope: ServerScope
  filePath: string
  /** The file's text as K2 last read it. */
  base: string
  /** K2 saw no file there (a new file): writes expect "absent". */
  baseAbsent: boolean
  /** The buffer. */
  text: string
  /** Read only, or not text: never write. */
  locked: boolean
  /** An Untitled document's first save picks a free name (FileViewerPane's). */
  saveNew?: (text: string) => Promise<void>
  /** Set the buffer; null = the file's text. */
  setText: (text: string | null) => void
  /** The file's text changed (a save landed, or a merge read it). */
  setBase: (text: string) => void
}

export interface MergeRecord {
  /** Your buffer before the merge ("Before external edit"). */
  before: string
  /** The file as saved outside K2. */
  theirs: string
  /** The merged text. */
  merged: string
  kept: KeptPair[]
}

/** What a restart brings back: the draft as is, or merged with a file that
 *  changed while K2 was closed. Null when there's nothing to restore. */
export function restoreDraft(draft: Draft, disk: string, diskAbsent: boolean): { text: string; kept: KeptPair[] } | null {
  if (draft.text === disk && !diskAbsent) return null
  const sameBase = diskAbsent ? draft.base === null : draft.base === disk
  if (sameBase) return { text: draft.text, kept: [] }
  const m = merge3(draft.base ?? '', disk, draft.text)
  return m.text === disk ? null : m
}

export interface SaveModel {
  state: SaveState
  status: SaveStatus | null
  notice: SaveNoticeView | null
  /** The raw notice (tests, shots). */
  noticeKind: SaveNotice['kind'] | null
  onAction: (id: SaveActionId) => void
  /** ⌘S. */
  saveNow: () => void
  /** The editor lost focus. */
  onBlur: () => void
  /** The last merge, while its notice is up (Compare, the kept marks). */
  merge: MergeRecord | null
  comparing: boolean
}

export function useSaveModel(o: SaveModelOptions): SaveModel {
  const [state, dispatch] = useReducer(saveReducer, INITIAL_SAVE)
  const [notice, setNotice] = useState<SaveNotice | null>(null)
  const [merge, setMerge] = useState<MergeRecord | null>(null)
  const [comparing, setComparing] = useState(false)
  const live = useRef(o)
  live.current = o
  const stateRef = useRef(state)
  stateRef.current = state
  const absentRef = useRef(o.baseAbsent)
  const inFlight = useRef(false)
  const mounted = useRef(true)
  /** The disk's text when a draft was restored (Undo goes back to it). */
  const restoredFrom = useRef<string | null>(null)
  /** Use their version's Undo: the buffer it replaced. */
  const replaced = useRef<string | null>(null)
  const host = o.scope.hostKey

  useEffect(() => {
    absentRef.current = o.baseAbsent
  }, [o.baseAbsent])

  const save = useCallback(async (): Promise<void> => {
    const { scope, filePath, locked, saveNew, setBase, setText } = live.current
    if (locked || inFlight.current) return
    const text = live.current.text
    const base = live.current.base
    if (!stateRef.current.dirty && text === base && !absentRef.current) return
    inFlight.current = true
    dispatch({ type: 'save' })
    try {
      if (saveNew) {
        await saveNew(text)
        if (!mounted.current) return
        dispatch({ type: 'saved', dirty: live.current.text !== text })
        return
      }
      const r = await writeChecked(scope, filePath, text, absentRef.current ? null : base)
      if (!mounted.current) {
        if (r.ok) void clearDraft(host, filePath)
        return
      }
      if (r.ok) {
        absentRef.current = false
        setBase(text)
        const still = live.current.text !== text
        if (!still) {
          setText(null)
          void clearDraft(host, filePath)
        }
        dispatch({ type: 'saved', dirty: still })
        setNotice((n) => (n && (n.kind === 'failed' || n.kind === 'missing' || n.kind === 'copy-failed') ? null : n))
        return
      }
      if (r.code === 'file_missing') {
        dispatch({ type: 'conflict', code: 'file_missing' })
        setNotice({ kind: 'missing' })
        return
      }
      const theirs = r.current?.content
      if (theirs === undefined) {
        dispatch({ type: 'conflict', code: r.code })
        setNotice({ kind: 'unmergeable' })
        return
      }
      // Merge: base = what K2 read, theirs = the file now, yours = the buffer
      // (typing during the write included).
      const yours = live.current.text
      const m = merge3(absentRef.current ? '' : base, theirs, yours)
      absentRef.current = false
      setBase(theirs)
      setText(m.text === theirs ? null : m.text)
      setMerge({ before: yours, theirs, merged: m.text, kept: m.kept })
      setComparing(false)
      setNotice({ kind: 'merged', kept: m.kept.length, comparing: false })
      dispatch(m.text === theirs ? { type: 'reset' } : { type: 'merged' })
    } catch (err) {
      if (!mounted.current || isHostSwitchedError(err)) return
      const reason = failureReason(err)
      const second = stateRef.current.failures >= 1
      dispatch({ type: 'failed', reason })
      setNotice({ kind: 'failed', reason, second })
    } finally {
      inFlight.current = false
    }
  }, [host])

  // The buffer changed: the reducer hears it, the draft is kept, autosave waits.
  const dirtyNow = o.text !== o.base || (o.baseAbsent && o.text !== '')
  useEffect(() => {
    dispatch({ type: 'edit', dirty: dirtyNow })
  }, [o.text, o.base, dirtyNow])

  const isNew = o.saveNew !== undefined
  useEffect(() => {
    if (o.locked || isNew) return
    const t = window.setTimeout(() => {
      if (dirtyNow) void putDraft(host, o.filePath, o.text, o.baseAbsent ? null : o.base)
      else void clearDraft(host, o.filePath)
    }, DRAFT_IDLE_MS)
    return () => window.clearTimeout(t)
  }, [o.text, o.base, o.baseAbsent, o.filePath, o.locked, isNew, dirtyNow, host])

  useEffect(() => {
    if (!canAutosave(state, { locked: o.locked })) return
    const t = window.setTimeout(() => void save(), AUTOSAVE_IDLE_MS)
    return () => window.clearTimeout(t)
  }, [o.text, state, o.locked, save])

  // Open: unsaved words from before K2 quit come back.
  useEffect(() => {
    mounted.current = true
    let off = false
    const { filePath, locked, saveNew, base, baseAbsent } = live.current
    if (!locked && !saveNew) {
      void getDraft(host, filePath).then((d) => {
        if (off || !d || !mounted.current) return
        const cur = live.current
        if (cur.text !== cur.base) return // already typing: leave it
        const r = restoreDraft(d, base, baseAbsent)
        if (!r) {
          void clearDraft(host, filePath)
          return
        }
        restoredFrom.current = base
        cur.setText(r.text)
        dispatch({ type: 'restored' })
        setNotice({ kind: 'restored' })
        if (r.kept.length) setMerge({ before: d.text, theirs: base, merged: r.text, kept: r.kept })
      })
    }
    return () => {
      off = true
      mounted.current = false
    }
    // Once per file.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [o.filePath, host])

  const saveCopy = useCallback(async (): Promise<void> => {
    const { scope, filePath, text } = live.current
    try {
      for (let n = 1; n <= 20; n++) {
        const path = copyPath(filePath, n)
        const r = await writeChecked(scope, path, text, null)
        if (r.ok) {
          setNotice({ kind: 'copied', name: path.slice(path.lastIndexOf('/') + 1) })
          return
        }
        if (r.code !== 'file_exists') break
      }
      setNotice({ kind: 'copy-failed', reason: 'every copy name is taken' })
    } catch (err) {
      if (isHostSwitchedError(err)) return
      setNotice({ kind: 'copy-failed', reason: failureReason(err) })
    }
  }, [])

  const onAction = useCallback(
    (id: SaveActionId): void => {
      const cur = live.current
      switch (id) {
        case 'retry':
          void save()
          return
        case 'save-copy':
          void saveCopy()
          return
        case 'save-again':
          absentRef.current = true
          void save()
          return
        case 'undo-restore':
          cur.setText(restoredFrom.current === cur.base ? null : restoredFrom.current)
          setNotice(null)
          setMerge(null)
          return
        case 'compare':
        case 'close-compare':
          setComparing(id === 'compare')
          setNotice((n) => (n?.kind === 'merged' ? { ...n, comparing: id === 'compare' } : n))
          return
        case 'undo-theirs':
          if (merge) cur.setText(merge.before === cur.base ? null : merge.before)
          setComparing(false)
          setNotice({ kind: 'undone' })
          return
        case 'redo-merge':
          if (merge) cur.setText(merge.merged === cur.base ? null : merge.merged)
          setNotice({ kind: 'merged', kept: merge?.kept.length ?? 0, comparing: false })
          return
        case 'use-theirs':
          replaced.current = cur.text
          if (merge) cur.setText(merge.theirs === cur.base ? null : merge.theirs)
          setComparing(false)
          setNotice({ kind: 'used-theirs' })
          return
        case 'undo-use-theirs':
          if (replaced.current !== null) cur.setText(replaced.current === cur.base ? null : replaced.current)
          setNotice(merge ? { kind: 'merged', kept: merge.kept.length, comparing: false } : null)
          return
        case 'reload':
          void daemonCliGet<{ content: string }>(cur.scope, 'fs/read-file', { path: cur.filePath })
            .then((r) => {
              if (!mounted.current) return
              live.current.setBase(r.content)
              live.current.setText(null)
              absentRef.current = false
              dispatch({ type: 'reset' })
              setNotice(null)
            })
            .catch(() => {
              /* the notice stays: Save a copy still keeps the words */
            })
          return
        case 'dismiss':
          setNotice(null)
          setComparing(false)
          setMerge(null)
      }
    },
    [save, saveCopy, merge],
  )

  const saveNow = useCallback((): void => {
    const s = stateRef.current
    if (s.phase === 'conflict') {
      if (s.conflict === 'file_missing') onAction('save-again')
      return
    }
    void save()
  }, [save, onAction])

  const onBlur = useCallback((): void => {
    if (canAutosave(stateRef.current, { locked: live.current.locked })) void save()
  }, [save])

  const fileName = o.filePath.split('/').pop() || o.filePath
  return {
    state,
    status: saveStatus(state, { locked: o.locked, merged: notice?.kind === 'merged' }),
    notice: notice ? saveNoticeView(notice, fileName) : null,
    noticeKind: notice?.kind ?? null,
    onAction,
    saveNow,
    onBlur,
    merge: notice?.kind === 'merged' || notice?.kind === 'undone' || notice?.kind === 'restored' ? merge : null,
    comparing: comparing && merge !== null,
  }
}
