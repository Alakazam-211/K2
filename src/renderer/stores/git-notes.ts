// Changes tab v1.1 review notes (C5, prd-changes-tab-v1 "C5").
//
// One live list per (server, workspace path), fed by `GET /cli/git/notes`
// (every status: the pane shows open notes and keeps resolved ones folded).
// It refetches on the daemon's `git_notes_changed` for that workspace, on
// `git_changed` (the daemon's `outdated` / moved lines follow the files), and
// on an app-socket reconnect. No polling.
//
// Room-aware: every call goes to the `scope` passed in — the ROOM's server.
// Writes go through `daemonCliPost` (view-only rooms refuse them; a usable
// remote room sends only the `git/notes/*` routes listed in room-writes.ts).

import { useEffect, useMemo } from 'react'
import { useStore } from 'zustand'
import { createStore, type StoreApi } from 'zustand/vanilla'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import type { ServerScope } from '@/kessel/server-scope'
import { onAppHello, onGitChanged, onGitNotesChanged, type UnsubscribeFn } from '@/stores/session-events'

export type NoteSide = 'old' | 'new'

/** A `GET /cli/git/notes` row (k2_core::git_notes::ReviewNote + place). */
export interface ReviewNote {
  id: string
  workspaceId: string
  workspacePath: string
  /** Repo-relative file path. */
  path: string
  side: NoteSide
  startLine: number
  endLine: number
  body: string
  author: string
  authorName: string
  createdAt: number
  updatedAt: number
  sentAt: number | null
  deliveryId: string | null
  resolvedAt: number | null
  resolvedBy: string | null
  rev: string | null
  blob: string | null
  anchorText: string | null
  /** The noted lines are no longer in the file (daemon's view). */
  outdated: boolean
  currentStartLine: number | null
  currentEndLine: number | null
}

export interface NoteCounts {
  open: number
  unsent: number
  sent: number
  resolved: number
}

export interface NotesList {
  workspacePath: string
  counts: NoteCounts
  notes: ReviewNote[]
}

export interface GitNotesState {
  list: NotesList | null
  loading: boolean
  error: string | null
}

interface Entry {
  key: string
  scope: ServerScope
  workspacePath: string
  store: StoreApi<GitNotesState>
  refs: number
  unsubs: UnsubscribeFn[]
  inflight: boolean
  again: boolean
}

const entries = new Map<string, Entry>()

function normPath(p: string): string {
  return p.length > 1 ? p.replace(/\/+$/, '') : p
}

function errText(e: unknown): string {
  return e instanceof Error ? e.message : String(e)
}

function getEntry(scope: ServerScope, workspacePath: string): Entry {
  const key = `${scope.id}\u0000${normPath(workspacePath)}`
  let entry = entries.get(key)
  if (!entry) {
    entry = {
      key,
      scope,
      workspacePath: normPath(workspacePath),
      store: createStore<GitNotesState>(() => ({ list: null, loading: false, error: null })),
      refs: 0,
      unsubs: [],
      inflight: false,
      again: false,
    }
    entries.set(key, entry)
  }
  return entry
}

async function fetchEntry(entry: Entry): Promise<void> {
  if (entry.inflight) {
    entry.again = true
    return
  }
  entry.inflight = true
  entry.store.setState({ loading: true })
  try {
    const list = await daemonCliGet<NotesList>(entry.scope, 'git/notes', { path: entry.workspacePath, status: 'all' })
    if (!list || typeof list !== 'object' || !Array.isArray(list.notes)) {
      throw new Error('git/notes: unexpected response')
    }
    entry.store.setState({ list, error: null })
  } catch (e) {
    entry.store.setState({ error: errText(e) })
  } finally {
    entry.inflight = false
    entry.store.setState({ loading: false })
  }
  if (entry.again) {
    entry.again = false
    if (entry.refs > 0) await fetchEntry(entry)
  }
}

function acquire(entry: Entry): () => void {
  entry.refs += 1
  if (entry.refs === 1) {
    const mine = (p: string): boolean => normPath(p) === entry.workspacePath
    entry.unsubs = [
      onGitNotesChanged(entry.scope, (e) => {
        if (mine(e.workspacePath)) void fetchEntry(entry)
      }),
      onGitChanged(entry.scope, (e) => {
        if (mine(e.workspacePath) && (entry.store.getState().list?.counts.open ?? 0) > 0) void fetchEntry(entry)
      }),
      onAppHello(entry.scope, () => void fetchEntry(entry)),
    ]
    void fetchEntry(entry)
  }
  let released = false
  return () => {
    if (released) return
    released = true
    entry.refs -= 1
    if (entry.refs > 0) return
    for (const off of entry.unsubs) off()
    entry.unsubs = []
  }
}

export interface UseGitNotesResult extends GitNotesState {
  refetch: () => Promise<void>
}

/** The live review notes on `workspacePath` (room's server). */
export function useGitNotes(scope: ServerScope, workspacePath: string | null | undefined): UseGitNotesResult {
  const entry = useMemo(() => (workspacePath ? getEntry(scope, workspacePath) : null), [scope, workspacePath])
  useEffect(() => (entry ? acquire(entry) : undefined), [entry])
  const state = useStore(entry?.store ?? EMPTY_STORE)
  return { ...state, refetch: () => (entry ? fetchEntry(entry) : Promise.resolve()) }
}

const EMPTY_STORE = createStore<GitNotesState>(() => ({ list: null, loading: false, error: null }))

/** Refetch the mounted list for `workspacePath` after a write here. */
export async function refreshGitNotes(scope: ServerScope, workspacePath: string): Promise<void> {
  const entry = entries.get(`${scope.id}\u0000${normPath(workspacePath)}`)
  if (entry && entry.refs > 0) await fetchEntry(entry)
}

// ── Anchors (same rules as k2_core::git_notes::locate) ────────────────────

/** A text's lines as the editor numbers them (no trailing empty line for a
 *  final newline, `\r` dropped). */
export function splitLines(text: string): string[] {
  if (text === '') return []
  const body = text.endsWith('\n') ? text.slice(0, -1) : text
  return body.split('\n').map((l) => (l.endsWith('\r') ? l.slice(0, -1) : l))
}

/** Lines `start..end` (1-based) of `text`, joined; null when out of range. */
export function anchorFromText(text: string, start: number, end: number): string | null {
  const lines = splitLines(text)
  if (start < 1 || end < start || end > lines.length) return null
  return lines.slice(start - 1, end).join('\n')
}

/** Where a note's lines are in `text` now: its own range when they still
 *  match, else the nearest place they moved to, else null (outdated). */
export function locateNote(note: Pick<ReviewNote, 'anchorText' | 'startLine' | 'endLine'>, text: string): { start: number; end: number } | null {
  const lines = splitLines(text)
  if (note.anchorText == null) {
    return note.startLine >= 1 && note.endLine >= note.startLine && note.endLine <= lines.length
      ? { start: note.startLine, end: note.endLine }
      : null
  }
  const want = note.anchorText.split('\n')
  const span = want.length
  if (lines.length < span) return null
  const at = (i: number): boolean => {
    for (let k = 0; k < span; k++) if (lines[i + k] !== want[k]) return false
    return true
  }
  const orig = Math.max(note.startLine - 1, 0)
  if (orig + span <= lines.length && at(orig)) return { start: orig + 1, end: orig + span }
  let best = -1
  for (let i = 0; i + span <= lines.length; i++) {
    if (at(i) && (best < 0 || Math.abs(i - orig) < Math.abs(best - orig))) best = i
  }
  return best < 0 ? null : { start: best + 1, end: best + span }
}

// ── Writes (all POST, all refresh) ────────────────────────────────────────

export interface AddNoteInput {
  filePath: string
  side: NoteSide
  startLine: number
  endLine: number
  body: string
  /** The lines as the viewer showed them. */
  anchorText?: string | null
  rev?: string | null
}

export interface SendNotesResult {
  success: boolean
  sent: number
  ids: string[]
  remaining: number
  deliveryId: string | null
  target: string
}

async function write<T>(scope: ServerScope, workspacePath: string, route: string, body: Record<string, unknown>): Promise<T> {
  const path = normPath(workspacePath)
  try {
    return await daemonCliPost<T>(scope, route, body)
  } finally {
    await refreshGitNotes(scope, path)
  }
}

export function addNote(scope: ServerScope, workspacePath: string, input: AddNoteInput): Promise<{ note: ReviewNote }> {
  const body: Record<string, unknown> = {
    path: normPath(workspacePath),
    filePath: input.filePath,
    side: input.side,
    startLine: input.startLine,
    endLine: input.endLine,
    body: input.body,
  }
  if (input.anchorText != null) body.anchorText = input.anchorText
  if (input.rev) body.rev = input.rev
  return write(scope, workspacePath, 'git/notes/add', body)
}

export function updateNote(scope: ServerScope, workspacePath: string, id: string, text: string): Promise<{ note: ReviewNote }> {
  return write(scope, workspacePath, 'git/notes/update', { id, body: text })
}

export function resolveNotes(scope: ServerScope, workspacePath: string, ids: string[], resolved = true): Promise<unknown> {
  return write(scope, workspacePath, 'git/notes/resolve', { ids, resolved })
}

export function deleteNotes(scope: ServerScope, workspacePath: string, ids: string[]): Promise<unknown> {
  return write(scope, workspacePath, 'git/notes/delete', { ids })
}

/** Send every open unsent note (or `ids`) to the workspace's agent. */
export function sendNotes(scope: ServerScope, workspacePath: string, ids?: string[]): Promise<SendNotesResult> {
  const body: Record<string, unknown> = { path: normPath(workspacePath) }
  if (ids && ids.length > 0) body.ids = ids
  return write(scope, workspacePath, 'git/notes/send', body)
}

// ── Test seams ────────────────────────────────────────────────────────────

export function __resetGitNotesForTests(): void {
  for (const entry of entries.values()) for (const off of entry.unsubs) off()
  entries.clear()
}
