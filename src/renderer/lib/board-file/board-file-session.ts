// One open `.k5board`, outside its frame (prd-board-tab-v1 §5; board-tab-b3-interface.md).
//
// The file is the truth and the daemon owns it. This holds what a tab must not lose while the daemon is busy, away
// or says no: the queue of gestures, the edits held in a conflict, and a copy of the board as the person sees it,
// so a frame that's torn down comes back with their edits and Save a copy works when the file can't be reached.
//
// Rules this keeps:
//   - A client never sends a file: only ops that name objects it read, each with the rev it knew at the gesture.
//   - Opening writes nothing. Nothing here sends a save without a gesture from the frame or a person's click.
//   - A group is all or nothing, and a held group is never dropped or resolved by itself.
//   - No save is tried over a file K2 can't read, a file that's gone, or a file from a newer K2.
//
// A held edit with a `heldId` is the daemon's (BT-24): it survives this tab, a restart and a crash, and comes back on
// the next read. One without (a daemon that doesn't store them yet) lives only in this tab's memory: `board-file-tabs.ts`
// asks before a close, quit or update would drop it, and leaves a note that says so on reopen if K2 goes down without
// asking.

import { DaemonHttpError } from '@/lib/daemon-http-error'

export const BOARD_FILE_SHAPE = 1
const IDLE_MS = 400
const MAX_WAIT_MS = 5_000
export const BOARD_FILE_RETRY_MS = 5_000
const UNREADABLE_RETRY_FOR_MS = 60_000
export const BOARD_FILE_BLOB_CHUNK = 8 * 1024 * 1024
/** Images kept for Save a copy when the file is gone (a board's pictures load lazily, so only those seen). */
const BLOB_CACHE_BYTES = 256 * 1024 * 1024

type Json = Record<string, unknown>
export type BoardObject = Json & { id: string; type: string }

export interface BoardFileApi {
  get<T = unknown>(route: string, params: Record<string, string | number | undefined>): Promise<T>
  post<T = unknown>(route: string, body: unknown): Promise<T>
}

export interface BoardFileDeps {
  api: BoardFileApi
  path: string
  setTimer(fn: () => void, ms: number): unknown
  clearTimer(t: unknown): void
  now(): number
  /** Is the tab showing? Retries of a failed save run only then. */
  visible(): boolean
}

export type BoardOp = Json & { op: string; id?: string }
export interface BoardGroup {
  id: string
  gesture: string
  kind: 'edit' | 'undo' | 'redo' | 'resolve'
  byAll: boolean
  ops: BoardOp[]
  /** Use mine on an edit the daemon holds: the held row clears when this group passes. */
  heldId?: string
}

export interface HeldObject {
  id: string
  why: 'changed' | 'deleted' | 'exists' | 'settings'
  theirs: Json | null
  /** Mine and theirs, field by field (BT-25). */
  fields: { field: string; mine: string; theirs: string }[]
  /** What the person had, for Put it back. */
  mine: BoardObject | null
}
export interface HeldGroup {
  id: string
  /** The daemon keeps it (it survives a restart). Null: only this tab has it. */
  heldId: string | null
  group: BoardGroup
  objects: HeldObject[]
}

export type BoardFilePhase =
  | { kind: 'loading' }
  | { kind: 'ready' }
  | { kind: 'empty' }
  | { kind: 'large'; bytes: number }
  | { kind: 'newer'; needsReader: number | null; ours: number | null }
  | { kind: 'shape' }
  | { kind: 'corrupt'; code: string; hint: string; at: string | null; badIds: string[]; badIdsTotal: number | null; badIdsTruncated: boolean }
  | { kind: 'gone' }
  /** The server's `board_file_preview` is off (a tab restored before its feature list was known). */
  | { kind: 'off' }
  | { kind: 'denied'; code: string; hint: string }
  | { kind: 'unreachable'; hint: string }

export type BoardFileState = 'ok' | 'moved' | 'unreadable' | 'unreadable-long'

export interface BoardFileView {
  phase: BoardFilePhase
  file: BoardFileState
  /** Why edits can't be made here now, in words for the person, or null. */
  readOnlyWhy: string | null
  status: { state: 'saved' | 'pending' | 'saving' | 'error'; text: string; detail?: string }
  queued: number
  held: HeldGroup[]
  /** Pictures the file names and doesn't hold. */
  missing: string[]
  /** A save was refused until those pictures are cleared. */
  missingBlocksSave: boolean
  /** K2 went down with edits this tab couldn't save (a note from before). */
  lostNote: { edits: number } | null
  /** The edits are safe in a copy the person saved. */
  copiedTo: string | null
}

/** What goes to the frame. */
export type BoardFileEvent =
  | { type: 'remote'; by: string; ops: BoardOp[] }
  | { type: 'status'; state: string; text: string; detail?: string }
  | { type: 'readOnly'; on: boolean; why: string }
  | { type: 'people'; me: string; people: Json }
  | { type: 'refused'; gesture: string; text: string }
  | { type: 'historyRefused'; gesture: string }
  | { type: 'saved'; tokens: string[] }
  | { type: 'focus'; id: string }

export interface BoardFileOpen {
  name: string
  me: string
  people: Json
  meta: Json
  objects: BoardObject[]
  readOnly: string | null
  status: BoardFileView['status']
}

interface ReadAnswer {
  same?: boolean
  empty?: boolean
  large?: boolean
  bytes?: number
  shape?: number
  hash?: string
  canWrite?: boolean
  objects?: BoardObject[]
  revs?: Record<string, string>
  settingsRev?: string
  meta?: Json
  people?: Json
  me?: string
  missing?: string[]
  held?: StoredHeld[]
}

interface StoredHeld {
  heldId: string
  groupId: string
  byAll?: boolean
  ops: BoardOp[]
  objects: { id: string; why: HeldObject['why']; theirs?: Json | null }[]
  /** The whole object as the person had it, for Put it back on a deleted one. */
  mine?: Record<string, BoardObject>
}

interface SaveAnswer {
  hash: string
  wrote?: boolean
  stale?: boolean
  revs?: Record<string, string>
  settingsRev?: string
  me?: string
  removed?: Record<string, BoardObject>
  held?: { groupId: string; heldId?: string; heldError?: string; objects: { id: string; why: HeldObject['why']; theirs?: Json | null }[] }[]
  refused?: { groupId: string; code: string; at?: string; hint?: string }[]
}

const clone = <T>(v: T): T => (v === undefined ? v : (JSON.parse(JSON.stringify(v)) as T))
const FORMAT_CODE = /^(zip_|manifest_|json_|board_|blob_|limit_|id_|min_reader)/
const SAVE_OUTAGES = new Set(['disk_full', 'read_only', 'io_error', 'busy'])

export function errorBody(err: unknown): Json {
  return err instanceof DaemonHttpError && err.body && typeof err.body === 'object' ? (err.body as Json) : {}
}
function codeOf(err: unknown): string {
  return err instanceof DaemonHttpError ? err.code ?? '' : ''
}
function statusOf(err: unknown): number | null {
  return err instanceof DaemonHttpError ? err.status : null
}
function hintOf(err: unknown): string {
  return err instanceof Error ? err.message : String(err)
}
const fieldText = (v: unknown): string => (v === undefined || v === null ? '(none)' : typeof v === 'string' ? v : JSON.stringify(v))
const nameOfPath = (p: string): string => (p.split(/[\\/]/).pop() ?? p).replace(/\.k5board$/i, '')
export const blobHashOfRef = (ref: string): string => ref.replace(/^k2blob:/, '')

export interface BoardFileSession {
  view(): BoardFileView
  subscribe(fn: () => void): () => void
  /** The frame's `boardFile.events`. */
  onEvent(fn: (e: BoardFileEvent) => void): () => void
  start(opts?: { allowLarge?: boolean }): Promise<void>
  dispose(): void
  // The frame's verbs.
  open(): BoardFileOpen
  submit(g: { gesture: string; kind: BoardGroup['kind']; byAll: boolean; ops: BoardOp[] }): void
  hold(on: boolean): void
  flush(): Promise<void>
  // What K2 hears.
  onBoardChanged(hash: string): void
  onOutsideChange(): void
  pollInfo(): Promise<void>
  // A person's clicks.
  startBoard(): Promise<void>
  openAnyway(): Promise<void>
  useMine(heldId: string): void
  useTheirs(heldId: string): void
  focusHeld(id: string): void
  clearMissing(): void
  saveCopy(to: string, opts?: { dropMissing?: boolean }): Promise<{ ok: true; path: string; name: string; missing: string[] } | { ok: false; missing: string[] }>
  // Blobs: every picture the frame draws, kept for a rescue.
  rememberBlob(hash: string, mime: string, bytes: Uint8Array): void
  cachedBlob(hash: string): { mime: string; bytes: Uint8Array } | null
  // The leave guard.
  pendingCount(): number
  heldCount(): number
  /** Drop queued and held edits at the person's say-so. */
  discard(): void
  setLostNote(n: { edits: number } | null): void
  setDirtyListener(fn: (() => void) | null): void
}

export function createBoardFileSession(deps: BoardFileDeps): BoardFileSession {
  const { api, path } = deps
  const name = nameOfPath(path)
  let phase: BoardFilePhase = { kind: 'loading' }
  let file: BoardFileState = 'ok'
  let canWrite = true
  let hash = ''
  let me = ''
  let people: Json = {}
  let meta: Json = {}
  let settingsRev = ''
  const shadow = new Map<string, BoardObject>()
  const revs = new Map<string, string>()
  const removed = new Map<string, BoardObject>()
  let queue: BoardGroup[] = []
  let inflight: BoardGroup[] | null = null
  let held: HeldGroup[] = []
  /** Held ids the person just resolved, until the daemon's clear lands. */
  const resolving = new Set<string>()
  let missing: string[] = []
  let missingBlocksSave = false
  let holding = false
  let flushTimer: unknown = null
  let retryTimer: unknown = null
  let unreadableTimer: unknown = null
  let unreadableSince = 0
  let lastSaveMs = 0
  let saveErr: string | null = null
  let seq = 0
  let lostNote: BoardFileView['lostNote'] = null
  let copiedTo: string | null = null
  let disposed = false
  let reading = false
  let readAgain: 'if' | 'full' | null = null
  const listeners = new Set<() => void>()
  const frames = new Set<(e: BoardFileEvent) => void>()
  let dirtyListener: (() => void) | null = null
  let lastStatus = ''
  let lastRO: string | null = null
  const blobs = new Map<string, { mime: string; bytes: Uint8Array }>()
  let blobBytes = 0
  /** Objects whose copy here holds an edit K2 refused: the next read puts the file's truth back. */
  const redraw = new Set<string>()
  let running: Promise<void> | null = null

  const newGroupId = (): string => `${deps.now().toString(36)}-${(++seq).toString(36)}-${Math.random().toString(36).slice(2, 8)}`
  const pushFrame = (e: BoardFileEvent): void => frames.forEach((f) => f(e))

  const readOnlyWhy = (): string | null => {
    if (file === 'moved') return 'this file was moved or deleted'
    if (file !== 'ok') return 'K2 can’t read this file right now'
    if (!canWrite) return 'this folder is read only'
    if (saveErr === 'read_only') return 'this folder is read only'
    if (missingBlocksSave) return 'a picture is missing from this file'
    return null
  }
  const pendingCount = (): number => queue.length + (inflight?.length ?? 0)
  const status = (): BoardFileView['status'] => {
    const detail = 'Saved in this file'
    const n = held.length
    const here = held.some((h) => h.heldId === null)
    if (saveErr) {
      const why = saveErr === 'disk_full' ? 'the disk is full' : saveErr === 'read_only' ? 'this folder is read only' : saveErr === 'offline' ? 'K2 isn’t answering' : 'K2 can’t write the file'
      return { state: 'error', text: `Not saved: ${why}. Retrying`, detail }
    }
    if (file === 'moved') return pendingCount() + n ? { state: 'error', text: 'Not saved: this file was moved or deleted' } : { state: 'saved', text: 'Read only' }
    if (file !== 'ok') return { state: 'error', text: pendingCount() + n ? 'Not saved: K2 can’t read this file' : 'K2 can’t read this file' }
    if (inflight) return { state: 'saving', text: 'Saving…', detail }
    if (queue.length) return { state: 'pending', text: holding ? 'Editing…' : 'Saving…', detail }
    if (n) {
      const edits = n === 1 ? '1 edit isn’t saved' : `${n} edits aren’t saved`
      return { state: 'error', text: edits, detail: here ? 'Only this tab has it until you choose' : 'K2 keeps it until you choose, even after a restart' }
    }
    return { state: 'saved', text: 'Saved', detail }
  }
  const notify = (): void => {
    if (disposed) return
    const s = status()
    const key = JSON.stringify(s)
    if (key !== lastStatus) {
      lastStatus = key
      pushFrame({ type: 'status', ...s })
    }
    const ro = readOnlyWhy()
    if (ro !== lastRO) {
      lastRO = ro
      pushFrame({ type: 'readOnly', on: ro !== null, why: ro ?? '' })
    }
    listeners.forEach((f) => f())
    dirtyListener?.()
  }

  // ---------- reading ----------

  function takeRead(r: ReadAnswer, by: string): void {
    const objects = r.objects ?? []
    const nextRevs = r.revs ?? {}
    const ops: BoardOp[] = []
    const seen = new Set<string>()
    const queuedAdds = new Set<string>()
    for (const g of [...(inflight ?? []), ...queue]) for (const op of g.ops) if (op.op === 'add') queuedAdds.add(String((op.obj as BoardObject).id))
    const orderBefore = [...shadow.values()].sort((a, b) => Number(a.z ?? 0) - Number(b.z ?? 0)).map((o) => o.id)
    for (const o of objects) {
      seen.add(o.id)
      const before = shadow.get(o.id)
      if (!before) {
        shadow.set(o.id, clone(o))
        ops.push({ op: 'add', obj: clone(o) })
      } else if (revs.get(o.id) !== nextRevs[o.id] || redraw.has(o.id)) {
        const f: Json = {}
        for (const k of Object.keys(o)) {
          if (k === 'id' || k === 'type' || k === 'z' || k === 'order') continue
          if (JSON.stringify(o[k]) !== JSON.stringify(before[k])) f[k] = o[k]
        }
        for (const k of Object.keys(before)) {
          if (k === 'id' || k === 'type' || k === 'z' || k === 'order') continue
          if (!(k in o)) f[k] = null
        }
        shadow.set(o.id, { ...clone(o), z: before.z, ...(before.order !== undefined ? { order: before.order } : {}) })
        if (Object.keys(f).length) ops.push({ op: 'set', id: o.id, f: clone(f) })
      }
    }
    for (const id of [...shadow.keys()]) {
      if (seen.has(id) || queuedAdds.has(id)) continue
      shadow.delete(id)
      ops.push({ op: 'del', id })
    }
    // Order: numbers come per read, so only a real change of order moves anything.
    const orderAfter = objects.map((o) => o.id)
    const oldKept = orderBefore.filter((id) => seen.has(id))
    const added = orderAfter.filter((id) => !orderBefore.includes(id))
    if (JSON.stringify(orderAfter) !== JSON.stringify([...oldKept, ...added])) {
      for (const o of objects) {
        const cur = shadow.get(o.id)!
        cur.z = o.z
        if (o.order !== undefined) cur.order = o.order
        ops.push({ op: 'set', id: o.id, f: { z: o.z, ...(o.order !== undefined ? { order: o.order } : {}) } })
      }
    } else {
      // New objects keep the file's place above what's here.
      for (const o of objects) {
        const cur = shadow.get(o.id)
        if (cur && cur.z === undefined) cur.z = o.z
      }
    }
    redraw.clear()
    revs.clear()
    for (const [k, v] of Object.entries(nextRevs)) revs.set(k, v)
    if (r.settingsRev) settingsRev = r.settingsRev
    if (r.people) people = r.people
    if (r.me) me = r.me
    pushFrame({ type: 'people', me, people })
    if (r.meta && JSON.stringify(r.meta) !== JSON.stringify(meta)) {
      const f: Json = {}
      for (const k of Object.keys(r.meta)) if (JSON.stringify(r.meta[k]) !== JSON.stringify(meta[k])) f[k] = r.meta[k]
      meta = clone(r.meta)
      ops.push({ op: 'meta', f: clone(f) })
    }
    missing = r.missing ?? []
    if (r.hash) hash = r.hash
    if (r.canWrite !== undefined) canWrite = r.canWrite
    if (r.held) takeStoredHeld(r.held)
    if (ops.length) pushFrame({ type: 'remote', by, ops })
  }

  function firstRead(r: ReadAnswer): void {
    if (r.empty) {
      phase = { kind: 'empty' }
      return
    }
    if (r.large) {
      phase = { kind: 'large', bytes: r.bytes ?? 0 }
      return
    }
    if ((r.shape ?? 0) > BOARD_FILE_SHAPE) {
      phase = { kind: 'shape' }
      return
    }
    shadow.clear()
    for (const o of r.objects ?? []) shadow.set(o.id, clone(o))
    revs.clear()
    for (const [k, v] of Object.entries(r.revs ?? {})) revs.set(k, v)
    hash = r.hash ?? ''
    me = r.me ?? ''
    people = r.people ?? {}
    meta = clone(r.meta ?? {})
    settingsRev = r.settingsRev ?? ''
    canWrite = r.canWrite !== false
    missing = r.missing ?? []
    if (r.held) takeStoredHeld(r.held)
    phase = { kind: 'ready' }
  }

  function heldObject(ops: BoardOp[], o: { id: string; why: HeldObject['why']; theirs?: Json | null }, mine: BoardObject | null): HeldObject {
    const mineOp =
      o.why === 'settings'
        ? ops.find((op) => op.op === 'meta')
        : ops.find((op) => op.id === o.id || (op.op === 'add' && (op.obj as BoardObject).id === o.id))
    const f = (mineOp?.f ?? {}) as Json
    const theirs = o.theirs ?? null
    const fields = Object.keys(f).map((field) => ({ field, mine: fieldText(f[field]), theirs: fieldText(theirs ? theirs[field] : undefined) }))
    return { id: o.id, why: o.why, theirs, fields, mine }
  }

  /** What the daemon holds for this person on this file. Edits this tab holds in memory alone stay; one the person just
   *  resolved stays gone (its clear may not have landed when this read began). */
  function takeStoredHeld(stored: StoredHeld[]): void {
    const live = stored.filter((h) => !resolving.has(h.heldId))
    const known = new Map(held.filter((h) => h.heldId !== null).map((h) => [h.heldId, h]))
    const next: HeldGroup[] = live.map((h) => {
      const have = known.get(h.heldId)
      if (have) return { ...have, objects: h.objects.map((o, i) => ({ ...heldObject(h.ops, o, have.objects[i]?.mine ?? h.mine?.[o.id] ?? null) })) }
      return {
        id: h.groupId,
        heldId: h.heldId,
        group: { id: h.groupId, gesture: `held-${h.heldId}`, kind: 'edit', byAll: h.byAll === true, ops: h.ops },
        objects: h.objects.map((o) => heldObject(h.ops, o, h.mine?.[o.id] ?? null)),
      }
    })
    held = [...next, ...held.filter((h) => h.heldId === null)]
  }

  function failFirst(err: unknown): void {
    const code = codeOf(err)
    const st = statusOf(err)
    const body = errorBody(err)
    const details = (body.error ?? {}) as Json
    if (code === 'not_enabled') phase = { kind: 'off' }
    else if (code === 'not_found' || st === 404) phase = { kind: 'gone' }
    else if (code === 'min_reader_too_high') phase = { kind: 'newer', needsReader: (details.needsReader as number) ?? null, ours: (details.ours as number) ?? null }
    else if (st === 422 || FORMAT_CODE.test(code)) {
      phase = {
        kind: 'corrupt',
        code,
        hint: hintOf(err),
        at: (details.at as string) ?? null,
        badIds: (details.badIds as string[]) ?? [],
        badIdsTotal: (details.badIdsTotal as number) ?? null,
        badIdsTruncated: details.badIdsTruncated === true,
      }
    } else if (st === 403) phase = { kind: 'denied', code, hint: hintOf(err) }
    else phase = { kind: 'unreachable', hint: hintOf(err) }
  }

  const start = async (opts?: { allowLarge?: boolean }): Promise<void> => {
    phase = { kind: 'loading' }
    notify()
    try {
      firstRead(await api.get<ReadAnswer>('board/file/read', { path, ...(opts?.allowLarge ? { allowLarge: 1 } : {}) }))
    } catch (err) {
      failFirst(err)
    }
    notify()
  }

  /** Read again. Never while a save is in flight: its answer settles the revs first. */
  const read = async (how: 'if' | 'full', by = 'someone'): Promise<void> => {
    if (disposed || phase.kind !== 'ready') return
    if (reading || inflight) {
      readAgain = how === 'full' || readAgain === 'full' ? 'full' : 'if'
      return
    }
    reading = true
    try {
      const r = await api.get<ReadAnswer>('board/file/read', { path, allowLarge: 1, ...(how === 'if' && hash ? { ifHash: hash } : {}) })
      fileBack()
      if (!r.same && !r.empty && !r.large) {
        if ((r.shape ?? 0) > BOARD_FILE_SHAPE) phase = { kind: 'shape' }
        else takeRead(r, by)
      }
    } catch (err) {
      fileTrouble(err)
    } finally {
      reading = false
    }
    notify()
    if (readAgain) {
      const again = readAgain
      readAgain = null
      void read(again, by)
    }
  }

  function fileBack(): void {
    if (file === 'ok') return
    file = 'ok'
    unreadableSince = 0
    if (unreadableTimer !== null) deps.clearTimer(unreadableTimer)
    unreadableTimer = null
    scheduleFlush(0)
  }

  /** The open file can't be read or written as it was: keep the board and its edits, and say why. */
  function fileTrouble(err: unknown): void {
    const code = codeOf(err)
    const st = statusOf(err)
    if (st === null) {
      // No answer at all: the daemon's gone, not the file.
      saveErr = 'offline'
      scheduleRetry()
      return
    }
    if (code === 'not_found' || st === 404 || code === 'io_error') {
      file = 'moved'
      return
    }
    if (st === 422 || FORMAT_CODE.test(code)) {
      if (file !== 'unreadable' && file !== 'unreadable-long') unreadableSince = deps.now()
      file = deps.now() - unreadableSince >= UNREADABLE_RETRY_FOR_MS ? 'unreadable-long' : 'unreadable'
      if (unreadableTimer === null && file === 'unreadable') {
        unreadableTimer = deps.setTimer(() => {
          unreadableTimer = null
          void read('full')
        }, BOARD_FILE_RETRY_MS)
      }
    }
  }

  // ---------- the queue ----------

  const mayFlush = (): boolean => phase.kind === 'ready' && file === 'ok' && canWrite && !holding && !missingBlocksSave && !inflight

  function scheduleFlush(delay?: number): void {
    if (disposed) return
    if (flushTimer !== null) deps.clearTimer(flushTimer)
    const idle = delay ?? Math.min(MAX_WAIT_MS, Math.max(IDLE_MS, 2 * lastSaveMs))
    flushTimer = deps.setTimer(() => {
      flushTimer = null
      void flushOnce()
    }, idle)
  }

  function scheduleRetry(): void {
    if (retryTimer !== null) return
    retryTimer = deps.setTimer(() => {
      retryTimer = null
      if (deps.visible()) void flushOnce()
      else if (pendingCount()) scheduleRetry()
    }, BOARD_FILE_RETRY_MS)
  }

  /** The longest run of groups whose ops each know their rev (an object this tab just added has none until saved). */
  function sendable(): BoardGroup[] {
    const out: BoardGroup[] = []
    const touched = new Set<string>()
    const idOf = (op: BoardOp): string => (op.op === 'meta' ? '\0settings' : String(op.id ?? (op.obj as BoardObject | undefined)?.id ?? ''))
    for (const g of queue) {
      if (g.ops.some((op) => (op.op === 'set' || op.op === 'del' || op.op === 'meta') && op.rev === undefined)) break
      // An object one group in this save changes has a new rev for the next group: that one waits for the answer.
      const ids = g.ops.filter((op) => op.op === 'set' || op.op === 'del' || op.op === 'add' || op.op === 'meta').map(idOf)
      if (ids.some((id) => touched.has(id))) break
      for (const id of ids) touched.add(id)
      out.push(g)
    }
    return out
  }

  /** What a group's ops become on the wire: never a number for position, and a raw object for an undone delete. */
  function wireOf(g: BoardGroup): Json {
    const ops = g.ops.map((op) => {
      const { z: _z, order: _order, undoAdd: _u, ...rest } = op as Json & { z?: unknown; order?: unknown; undoAdd?: unknown }
      if (op.op === 'add' && g.kind !== 'edit' && g.kind !== 'resolve') {
        const raw = removed.get(String((op.obj as BoardObject).id))
        if (raw) return { op: 'putback', object: raw, at: op.at }
      }
      return rest
    })
    return { id: g.id, noHold: g.kind === 'undo' || g.kind === 'redo', byAll: g.byAll, ops, ...(g.heldId ? { heldId: g.heldId } : {}) }
  }

  const flushOnce = (): Promise<void> => {
    if (running) return running
    running = runFlush().finally(() => {
      running = null
    })
    return running
  }

  const runFlush = async (): Promise<void> => {
    if (disposed || !mayFlush() || queue.length === 0) return
    const sending = sendable()
    if (sending.length === 0) return
    queue = queue.slice(sending.length)
    inflight = sending
    notify()
    const startedAt = deps.now()
    let answer: SaveAnswer | null = null
    let stop = false
    try {
      answer = await api.post<SaveAnswer>('board/file/save', { path, baseHash: hash, groups: sending.map(wireOf) })
    } catch (err) {
      const code = codeOf(err)
      if (code === 'board_conflict') {
        answer = errorBody(err) as unknown as SaveAnswer
      } else {
        // Nothing is lost: the same group ids go back to the front, and a retried id gets its first answer.
        queue = [...sending, ...queue]
        inflight = null
        if (SAVE_OUTAGES.has(code) || (statusOf(err) === 503)) {
          saveErr = code === 'read_only' || code === 'disk_full' ? code : 'io'
          scheduleRetry()
        } else if (code === 'blob_missing') {
          missingBlocksSave = true
        } else if (statusOf(err) === null) {
          saveErr = 'offline'
          scheduleRetry()
        } else {
          fileTrouble(err)
          if (file === 'ok') {
            // A refusal for the whole request that names no group: said, and the edits stay queued.
            saveErr = 'io'
            scheduleRetry()
          }
        }
        stop = true
      }
    }
    if (stop || !answer) {
      notify()
      if (readAgain) {
        const again = readAgain
        readAgain = null
        void read(again)
      }
      return
    }
    lastSaveMs = deps.now() - startedAt
    saveErr = null
    inflight = null
    takeSave(sending, answer)
    notify()
    if (queue.length) scheduleFlush(0)
  }

  function takeSave(sent: BoardGroup[], a: SaveAnswer): void {
    hash = a.hash ?? hash
    if (a.settingsRev) settingsRev = a.settingsRev
    if (a.me && a.me !== me) {
      me = a.me
      pushFrame({ type: 'people', me, people })
    }
    for (const [id, raw] of Object.entries(a.removed ?? {})) removed.set(id, raw)
    // The daemon has answered for these: from here on its list says whether they're still held.
    for (const g of sent) if (g.heldId) resolving.delete(g.heldId)
    const heldIds = new Set((a.held ?? []).map((h) => h.groupId))
    const refused = new Map((a.refused ?? []).map((r) => [r.groupId, r]))
    const heldObjects = new Set<string>()
    const remoteOps: BoardOp[] = []
    for (const h of a.held ?? []) {
      const group = sent.find((g) => g.id === h.groupId)
      if (!group) continue
      const objects: HeldObject[] = h.objects.map((o) => {
        heldObjects.add(o.id)
        return heldObject(group.ops, o, clone(shadow.get(o.id) ?? null))
      })
      held.push({ id: h.groupId, heldId: h.heldId ?? null, group, objects })
      // Put it back has to survive a restart too, and only this tab still has the deleted object.
      if (h.heldId && objects.some((o) => o.why === 'deleted' && o.mine)) {
        const mine = Object.fromEntries(objects.filter((o) => o.why === 'deleted' && o.mine).map((o) => [o.id, o.mine]))
        api.post('board/file/held/mine', { path, heldId: h.heldId, mine }).catch((err: unknown) => console.warn('[board-file] the daemon didn’t keep the deleted object:', err))
      }
      // The other side's change applies at once (BT-25): this tab shows theirs while the edit waits.
      for (const o of h.objects) {
        if (o.why === 'deleted') {
          if (shadow.has(o.id)) {
            shadow.delete(o.id)
            remoteOps.push({ op: 'del', id: o.id })
          }
        } else if (o.theirs) {
          const cur = shadow.get(o.id)
          if (cur) {
            const f: Json = {}
            for (const k of Object.keys(o.theirs)) if (k !== 'id' && k !== 'type' && k !== 'z' && k !== 'order') f[k] = o.theirs[k]
            Object.assign(cur, clone(f))
            remoteOps.push({ op: 'set', id: o.id, f: clone(f) })
          }
        }
      }
    }
    if (remoteOps.length) pushFrame({ type: 'remote', by: 'someone', ops: remoteOps })
    const saved: string[] = []
    for (const g of sent) {
      if (heldIds.has(g.id)) continue
      const r = refused.get(g.id)
      if (!r) {
        saved.push(g.gesture)
        continue
      }
      if (r.code === 'board_conflict') {
        pushFrame({ type: 'historyRefused', gesture: g.gesture })
        rollbackShadow(g)
      } else if (r.code === 'blob_missing') {
        missingBlocksSave = true
        queue = [g, ...queue]
      } else {
        pushFrame({ type: 'refused', gesture: g.gesture, text: refusalText(r.code, r.hint) })
        rollbackShadow(g)
      }
    }
    if (saved.length) pushFrame({ type: 'saved', tokens: saved })
    // Own-save revs: queued ops naming an object this save changed take the new rev, unless its group was held.
    for (const [id, rev] of Object.entries(a.revs ?? {})) {
      if (!heldObjects.has(id)) revs.set(id, rev)
    }
    for (const g of queue) {
      for (const op of g.ops) {
        if (op.op === 'meta') {
          if (op.rev === undefined || a.settingsRev) op.rev = settingsRev
          continue
        }
        if (op.op !== 'set' && op.op !== 'del') continue
        const id = String(op.id)
        if (heldObjects.has(id)) continue
        if (a.revs?.[id]) op.rev = a.revs[id]
        else if (op.rev === undefined && revs.has(id)) op.rev = revs.get(id)
      }
    }
    if (a.stale || (a.held?.length ?? 0) > 0 || readAgain === 'full') {
      readAgain = null
      void read('full')
    } else if (readAgain) {
      readAgain = null
      void read('if')
    }
  }

  /** A refused group's edits leave the tab's copy too: the next read shows the file's truth. */
  function rollbackShadow(g: BoardGroup): void {
    readAgain = 'full'
    for (const op of g.ops) {
      if (op.op === 'add') shadow.delete(String((op.obj as BoardObject).id))
      else if (op.id !== undefined) redraw.add(String(op.id))
    }
  }

  function refusalText(code: string, hint?: string): string {
    if (code === 'object_locked') return 'That item is locked in this file, so the change was undone'
    return `K2 couldn’t save that change, so it was undone (${hint || code})`
  }

  const applyToShadow = (ops: BoardOp[]): void => {
    for (const op of ops) {
      if (op.op === 'add') {
        const obj = clone(op.obj) as BoardObject
        if (op.z !== undefined) obj.z = op.z
        if (op.order !== undefined) obj.order = op.order
        shadow.set(obj.id, obj)
      } else if (op.op === 'set') {
        const cur = shadow.get(String(op.id))
        if (!cur) continue
        for (const [k, v] of Object.entries((op.f ?? {}) as Json)) {
          if (v === null) delete cur[k]
          else cur[k] = clone(v)
        }
      } else if (op.op === 'del') {
        shadow.delete(String(op.id))
      } else if (op.op === 'z') {
        const cur = shadow.get(String(op.id))
        if (cur) cur.z = op.z
      } else if (op.op === 'frame') {
        const cur = shadow.get(String(op.id))
        if (cur) cur.order = op.order
      } else if (op.op === 'meta') {
        Object.assign(meta, clone(op.f))
      }
    }
  }

  /** Same-object `set`s merge only within one gesture: a group is all or nothing. */
  const mergeWithin = (ops: BoardOp[]): BoardOp[] => {
    const out: BoardOp[] = []
    for (const op of ops) {
      if (op.op === 'set') {
        const add = out.find((p) => p.op === 'add' && (p.obj as BoardObject).id === op.id)
        if (add) {
          Object.assign(add.obj as Json, clone(op.f))
          continue
        }
        const prev = out.find((p) => p.op === 'set' && p.id === op.id)
        if (prev) {
          Object.assign(prev.f as Json, clone(op.f))
          continue
        }
      }
      out.push(clone(op))
    }
    return out
  }

  // A queued delete undone before it left: both go, and the undo's gesture is answered as saved.
  function cancelQueuedDelete(g: BoardGroup): boolean {
    if (g.kind !== 'undo' || g.ops.length === 0 || !g.ops.every((op) => op.op === 'add')) return false
    const ids = new Set(g.ops.map((op) => String((op.obj as BoardObject).id)))
    const i = queue.findIndex((q) => q.ops.length === ids.size && q.ops.every((op) => op.op === 'del' && ids.has(String(op.id))))
    if (i < 0) return false
    const [gone] = queue.splice(i, 1)
    pushFrame({ type: 'saved', tokens: [gone.gesture, g.gesture] })
    return true
  }

  const submit: BoardFileSession['submit'] = ({ gesture, kind, byAll, ops }) => {
    copiedTo = null
    const stamped = mergeWithin(ops)
    for (const op of stamped) {
      if (op.op === 'set' || op.op === 'del') {
        const r = revs.get(String(op.id))
        if (r !== undefined) op.rev = r
      } else if (op.op === 'meta') {
        op.rev = settingsRev
      }
    }
    applyToShadow(stamped)
    const g: BoardGroup = { id: newGroupId(), gesture, kind, byAll, ops: stamped }
    if (!cancelQueuedDelete(g)) queue.push(g)
    scheduleFlush()
    notify()
  }

  const flush = async (): Promise<void> => {
    if (flushTimer !== null) deps.clearTimer(flushTimer)
    flushTimer = null
    const was = holding
    holding = false
    // A save in flight, then what's left after it (an object just added has its rev only after the first answer).
    for (let i = 0; i < 16; i++) {
      await flushOnce()
      // A failed save waits for its retry: a person's Cmd+S doesn't hammer a full disk.
      if (saveErr || !queue.length || !mayFlush() || sendable().length === 0) break
    }
    holding = was
  }

  // ---------- saving a copy ----------

  const putBlobForRescue = async (h: string, b: { mime: string; bytes: Uint8Array }): Promise<void> => {
    const total = b.bytes.length
    for (let offset = 0; offset < total || offset === 0; offset += BOARD_FILE_BLOB_CHUNK) {
      const end = Math.min(total, offset + BOARD_FILE_BLOB_CHUNK)
      let s = ''
      const chunk = b.bytes.subarray(offset, end)
      for (let i = 0; i < chunk.length; i += 0x8000) s += String.fromCharCode(...chunk.subarray(i, i + 0x8000))
      await api.post('board/file/blob/put', { hash: h, offset, total, mime: b.mime, data: btoa(s) })
      if (end >= total) break
    }
  }

  const copyGroups = (): Json[] => {
    const all = [...(inflight ?? []), ...queue, ...held.map((h) => h.group)]
    return all.map((g) => {
      const w = wireOf(g)
      for (const op of w.ops as BoardOp[]) {
        if (op.op === 'set' || op.op === 'del') op.rev = revs.get(String(op.id)) ?? op.rev
        if (op.op === 'meta') op.rev = settingsRev
      }
      return { ...w, noHold: false }
    })
  }

  const saveCopy: BoardFileSession['saveCopy'] = async (to, opts) => {
    const unreachable = async (): Promise<Awaited<ReturnType<BoardFileSession['saveCopy']>>> => {
      const objects = [...shadow.values()].sort((a, b) => Number(a.z ?? 0) - Number(b.z ?? 0))
      const refs = new Set<string>()
      for (const o of objects) if (typeof o.src === 'string' && o.src.startsWith('k2blob:')) refs.add(blobHashOfRef(o.src))
      for (const h of refs) {
        const b = blobs.get(h)
        if (b) await putBlobForRescue(h, b)
      }
      const frameOrder = objects.filter((o) => o.type === 'frame').sort((a, b) => Number(a.order ?? 0) - Number(b.order ?? 0)).map((o) => o.id)
      try {
        const r = await api.post<{ path: string; name: string; missing?: string[] }>('board/file/copy/rescue', {
          to,
          objects,
          meta,
          people,
          frames: frameOrder,
          blobs: [...refs],
          ...(opts?.dropMissing ? { dropMissing: true } : {}),
        })
        return { ok: true, path: r.path, name: r.name, missing: r.missing ?? [] }
      } catch (err) {
        // The contract has the list beside the error; an older shape had it inside: either is read.
        const body = errorBody(err)
        const list = Array.isArray(body.missing) ? body.missing : Array.isArray((body.error as Json | undefined)?.missing) ? (body.error as Json).missing : null
        if (statusOf(err) === 409 && list) return { ok: false, missing: list as string[] }
        throw err
      }
    }
    const done = (r: { ok: true; path: string; name: string; missing: string[] }): typeof r => {
      copiedTo = r.path
      notify()
      return r
    }
    if (file !== 'ok') {
      const r = await unreachable()
      return r.ok ? done(r) : r
    }
    try {
      const groups = copyGroups()
      const r = await api.post<{ path: string; name: string; missing?: string[] }>('board/file/copy', { path, to, ...(groups.length ? { groups } : {}) })
      return done({ ok: true, path: r.path, name: r.name, missing: r.missing ?? [] })
    } catch (err) {
      // The original turned out to be the problem: the tab's own copy of the board still goes out.
      const code = codeOf(err)
      if (code === 'not_found' || (statusOf(err) === 503 && code === 'io_error') || statusOf(err) === 422) {
        const r = await unreachable()
        return r.ok ? done(r) : r
      }
      throw err
    }
  }

  // ---------- the session ----------

  return {
    view: () => ({
      phase,
      file,
      readOnlyWhy: readOnlyWhy(),
      status: status(),
      queued: pendingCount(),
      held,
      missing,
      missingBlocksSave,
      lostNote,
      copiedTo,
    }),
    subscribe(fn) {
      listeners.add(fn)
      return () => listeners.delete(fn)
    },
    onEvent(fn) {
      frames.add(fn)
      return () => frames.delete(fn)
    },
    start,
    dispose() {
      disposed = true
      for (const t of [flushTimer, retryTimer, unreadableTimer]) if (t !== null) deps.clearTimer(t)
      listeners.clear()
      frames.clear()
    },
    open: () => ({
      name,
      me,
      people: clone(people),
      meta: clone(meta),
      objects: [...shadow.values()].sort((a, b) => Number(a.z ?? 0) - Number(b.z ?? 0)).map(clone),
      readOnly: readOnlyWhy(),
      status: status(),
    }),
    submit,
    hold(on) {
      holding = on
      if (!on && queue.length) scheduleFlush()
      notify()
    },
    flush,
    onBoardChanged(h) {
      if (h === hash) return
      void read('if', 'someone')
    },
    onOutsideChange() {
      void pollInfo()
    },
    pollInfo,
    async startBoard() {
      try {
        await api.post('board/file/start', { path })
        await start()
      } catch (err) {
        if (codeOf(err) === 'not_empty') {
          await start()
          return
        }
        throw err
      }
    },
    openAnyway: () => start({ allowLarge: true }),
    useMine(heldId) {
      const h = held.find((x) => x.id === heldId)
      if (!h) return
      // Put it back needs the whole object. If it wasn't kept (K2 restarted before the daemon had it), the field list is
      // all there is: the edit stays held until the person lets it go, never cleared by this click.
      if (h.objects.some((o) => o.why === 'deleted' && !o.mine)) return
      held = held.filter((x) => x !== h)
      const deleted = new Map(h.objects.filter((o) => o.why === 'deleted').map((o) => [o.id, o]))
      const ops: BoardOp[] = []
      const remote: BoardOp[] = []
      for (const op of h.group.ops) {
        const id = String(op.id ?? (op.obj as BoardObject | undefined)?.id)
        const d = deleted.get(id)
        if (d) {
          // Put it back: the object as the person had it, as a new add.
          if (op.op === 'del' || !d.mine) continue
          ops.push({ op: 'add', obj: d.mine, at: 'front', z: d.mine.z })
          remote.push({ op: 'add', obj: d.mine })
          shadow.set(id, clone(d.mine))
          continue
        }
        const next = clone(op)
        if (op.op === 'set' || op.op === 'del') next.rev = revs.get(id) ?? op.rev
        if (op.op === 'meta') next.rev = settingsRev
        ops.push(next)
        if (op.op === 'set') remote.push({ op: 'set', id, f: clone(op.f) as Json })
        if (op.op === 'del') remote.push({ op: 'del', id })
      }
      applyToShadow(ops)
      if (remote.length) pushFrame({ type: 'remote', by: me, ops: remote })
      if (ops.length) {
        if (h.heldId) resolving.add(h.heldId)
        queue.push({ id: newGroupId(), gesture: `resolve-${h.id}`, kind: 'resolve', byAll: h.group.byAll, ops, ...(h.heldId ? { heldId: h.heldId } : {}) })
        scheduleFlush(0)
      } else if (h.heldId) {
        clearStored(h.heldId)
      }
      notify()
    },
    useTheirs(heldId) {
      // The tab already shows theirs. The save route isn't called, and the edit goes at the person's say-so.
      const h = held.find((x) => x.id === heldId)
      held = held.filter((x) => x.id !== heldId)
      if (h?.heldId) clearStored(h.heldId)
      notify()
    },
    focusHeld(id) {
      pushFrame({ type: 'focus', id })
    },
    clearMissing() {
      const ops: BoardOp[] = missing.map((h) => ({ op: 'clearBlob', hash: h }))
      missingBlocksSave = false
      if (ops.length) queue.unshift({ id: newGroupId(), gesture: `clear-${seq}`, kind: 'resolve', byAll: false, ops })
      scheduleFlush(0)
      notify()
    },
    saveCopy,
    rememberBlob(h, mime, bytes) {
      if (blobs.has(h) || bytes.length > BLOB_CACHE_BYTES) return
      blobs.set(h, { mime, bytes })
      blobBytes += bytes.length
      for (const [k, v] of blobs) {
        if (blobBytes <= BLOB_CACHE_BYTES) break
        blobs.delete(k)
        blobBytes -= v.bytes.length
      }
    },
    cachedBlob: (h) => blobs.get(h) ?? null,
    pendingCount,
    // What's only here: the daemon keeps a held edit with an id, whatever happens to this tab.
    heldCount: () => held.filter((h) => h.heldId === null).length,
    discard() {
      queue = []
      held = held.filter((h) => h.heldId !== null)
      if (flushTimer !== null) deps.clearTimer(flushTimer)
      flushTimer = null
      notify()
    },
    setLostNote(n) {
      lostNote = n
      notify()
    },
    setDirtyListener(fn) {
      dirtyListener = fn
    },
  }

  /** Clear an edit the daemon holds. If this doesn't land, the next read brings it back: never lost, at worst asked twice. */
  function clearStored(heldId: string): void {
    resolving.add(heldId)
    api
      .post('board/file/held/resolve', { path, heldId })
      .then(() => resolving.delete(heldId))
      .catch((err: unknown) => {
        resolving.delete(heldId)
        console.warn('[board-file] the held edit couldn’t be cleared:', err)
        void read('full')
      })
  }

  async function pollInfo(): Promise<void> {
    if (disposed || phase.kind !== 'ready' || reading) return
    try {
      const info = await api.get<{ hash?: string; canWrite?: boolean; empty?: boolean }>('board/file/info', { path })
      const wasBad = file !== 'ok'
      fileBack()
      if (info.canWrite !== undefined && info.canWrite !== canWrite) {
        canWrite = info.canWrite
        if (canWrite && queue.length) scheduleFlush(0)
      }
      if (wasBad || (info.hash && info.hash !== hash)) await read(wasBad ? 'full' : 'if')
      notify()
    } catch (err) {
      fileTrouble(err)
      notify()
    }
  }
}

/** The frame's gesture parts, joined. A gesture can be larger than one 64 KB message. */
export function createBoardFilePartJoiner(limitChars = 64 * 1024 * 1024): (p: { gesture: string; part: number; last: boolean; text: string }) => string | null {
  const open = new Map<string, string[]>()
  return ({ gesture, part, last, text }) => {
    const got = open.get(gesture) ?? []
    if (part !== got.length) {
      open.delete(gesture)
      throw new Error('board file: a gesture’s parts came out of order')
    }
    got.push(text)
    if (got.reduce((n, s) => n + s.length, 0) > limitChars) {
      open.delete(gesture)
      throw new Error('board file: a gesture is too large')
    }
    if (!last) {
      open.set(gesture, got)
      return null
    }
    open.delete(gesture)
    return got.join('')
  }
}
