// Changes tab live data (prd-changes-tab-v1, C2).
//
// One store per (server, workspace path, compare base), shared by every
// mounted consumer and fed by `GET /cli/git/snapshot`. It refreshes on the
// daemon's `git_changed` event for that workspace — no polling. `rev` is
// opaque: a frame whose rev equals the snapshot we hold (or the one we are
// already fetching for) is dropped, so a burst never stacks refetches.
//
// Fallbacks:
//   - `fs_changed` for the workspace refetches after a short wait, but only
//     until the server has sent any `git_changed` (an older daemon without
//     C1). After that, git_changed alone drives it (C1 emits it for every
//     non-ignored working-tree edit too).
//   - the app socket's `hello` (a WS reconnect; events are not replayed)
//     refetches.
//
// Room-aware: every request and subscription goes to the `scope` the caller
// passes — the ROOM's scope, never the window's. Writes go through
// `daemonCliPost`, so a view-only room refuses them and a usable remote room
// sends only the room allowlist (`kessel/room-writes.ts`).

import { useEffect, useMemo } from 'react'
import { useStore } from 'zustand'
import { createStore, type StoreApi } from 'zustand/vanilla'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import type { ServerScope } from '@/kessel/server-scope'
import {
  onAppHello,
  onFsChanged,
  onGitChanged,
  type FsChangedEvent,
  type GitChangedEvent,
  type UnsubscribeFn,
} from '@/stores/session-events'
import type { ChangesGroup } from '@/stores/tabs'

// ── Wire types (contract: prd-changes-tab-v1 §1–§3) ──────────────────────

export type ChangesFileStatus = 'M' | 'A' | 'D' | 'R' | 'T' | 'U' | 'C'

export interface ChangesFile {
  path: string
  oldPath: string | null
  group: ChangesGroup
  status: ChangesFileStatus
  additions: number | null
  deletions: number | null
  binary: boolean
  dir: boolean
}

export interface ChangesCounts {
  staged: number
  unstaged: number
  untracked: number
  conflicted: number
  base: number
  total: number
}

export interface ChangesBaseInfo {
  ref: string
  oid: string
  mergeBase: string | null
  ahead: number
  behind: number
}

export interface GitSnapshotRepo {
  isRepo: true
  workspacePath: string
  repoRoot: string
  rev: string
  branch: string | null
  headOid: string | null
  detached: boolean
  unborn: boolean
  upstream: string | null
  ahead: number
  behind: number
  state: string
  base: ChangesBaseInfo | null
  counts: ChangesCounts
  truncated: boolean
  limit: number
  files: ChangesFile[]
}

export interface GitSnapshotNotRepo {
  isRepo: false
  workspacePath: string
  rev: string
}

export type GitSnapshot = GitSnapshotRepo | GitSnapshotNotRepo

export interface DiffSide {
  label: string
  exists: boolean
  text: string | null
  size: number
  binary: boolean
  tooLarge: boolean
}

/** `GET /cli/git/diff-file?sides=1` (contract §2). */
export interface DiffSides {
  path: string
  oldPath: string | null
  mode: 'worktree' | 'unstaged' | 'staged' | 'base'
  baseRef: string | null
  rev: string
  old: DiffSide
  new: DiffSide
  binary: boolean
  tooLarge: boolean
  additions: number | null
  deletions: number | null
}

export interface DiscardResult {
  success: boolean
  restored: string[]
  trashed: string[]
  unstaged: string[]
  skipped: Array<{ path: string; reason: string }>
  failed: Array<{ path: string; error: string }>
  rev: string
}

// ── Store registry ───────────────────────────────────────────────────────

export interface GitChangesState {
  snapshot: GitSnapshot | null
  /** A fetch is running (first load or refresh). */
  loading: boolean
  /** Last fetch error; the previous snapshot stays shown. */
  error: string | null
}

/** fs_changed fallback wait: long enough for C1's git_changed (300 ms
 *  debounce, 1 s max) to arrive first and cancel it. */
export const FS_FALLBACK_MS = 1500

interface Entry {
  key: string
  scope: ServerScope
  workspacePath: string
  base: string | null
  store: StoreApi<GitChangesState>
  refs: number
  unsubs: UnsubscribeFn[]
  inflight: boolean
  /** A refetch was asked while one was running. */
  again: boolean
  /** The newest event rev we fetched (or are fetching) for. */
  wantedRev: string | null
  fsTimer: ReturnType<typeof setTimeout> | null
}

const entries = new Map<string, Entry>()
/** Scopes whose server has sent at least one `git_changed` (C1+ daemon). */
const gitLiveScopes = new Set<string>()

function normPath(p: string): string {
  return p.length > 1 ? p.replace(/\/+$/, '') : p
}

function entryKey(scope: ServerScope, workspacePath: string, base: string | null): string {
  return `${scope.id}\u0000${normPath(workspacePath)}\u0000${base ?? ''}`
}

function errText(e: unknown): string {
  return e instanceof Error ? e.message : String(e)
}

function getEntry(scope: ServerScope, workspacePath: string, base: string | null): Entry {
  const key = entryKey(scope, workspacePath, base)
  let entry = entries.get(key)
  if (!entry) {
    entry = {
      key,
      scope,
      workspacePath: normPath(workspacePath),
      base,
      store: createStore<GitChangesState>(() => ({ snapshot: null, loading: false, error: null })),
      refs: 0,
      unsubs: [],
      inflight: false,
      again: false,
      wantedRev: null,
      fsTimer: null,
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
    const params: Record<string, string> = { path: entry.workspacePath }
    if (entry.base) params.base = entry.base
    const snap = await daemonCliGet<GitSnapshot>(entry.scope, 'git/snapshot', params)
    if (!snap || typeof snap !== 'object' || typeof snap.isRepo !== 'boolean') {
      throw new Error('git/snapshot: unexpected response')
    }
    entry.store.setState({ snapshot: snap, error: null })
  } catch (e) {
    entry.store.setState({ error: errText(e) })
  } finally {
    entry.inflight = false
    entry.store.setState({ loading: false })
  }
  if (entry.again && entry.refs > 0) {
    entry.again = false
    const have = entry.store.getState().snapshot?.rev ?? null
    // Asked again for a rev we now hold: nothing new to read.
    if (entry.wantedRev === null || entry.wantedRev !== have) await fetchEntry(entry)
  } else {
    entry.again = false
  }
}

function onGitEvent(entry: Entry, e: GitChangedEvent): void {
  if (normPath(e.workspacePath) !== entry.workspacePath) return
  if (entry.fsTimer) {
    clearTimeout(entry.fsTimer)
    entry.fsTimer = null
  }
  const have = entry.store.getState().snapshot?.rev ?? null
  if (e.rev === have) return
  if (e.rev === entry.wantedRev && entry.inflight) return
  entry.wantedRev = e.rev
  void fetchEntry(entry)
}

function fsTouches(entry: Entry, e: FsChangedEvent): boolean {
  if (normPath(e.workspacePath) === entry.workspacePath) return true
  const prefix = `${entry.workspacePath}/`
  return (e.paths ?? []).some((p) => p === entry.workspacePath || p.startsWith(prefix))
}

function acquire(entry: Entry): () => void {
  entry.refs += 1
  if (entry.refs === 1) {
    const scopeId = entry.scope.id
    entry.unsubs = [
      onGitChanged(entry.scope, (e) => {
        gitLiveScopes.add(scopeId)
        onGitEvent(entry, e)
      }),
      onFsChanged(entry.scope, (e) => {
        if (gitLiveScopes.has(scopeId)) return
        if (!fsTouches(entry, e)) return
        if (entry.fsTimer) clearTimeout(entry.fsTimer)
        entry.fsTimer = setTimeout(() => {
          entry.fsTimer = null
          if (entry.refs > 0 && !gitLiveScopes.has(scopeId)) {
            entry.wantedRev = null
            void fetchEntry(entry)
          }
        }, FS_FALLBACK_MS)
      }),
      // A reconnect: frames sent while the socket was down are gone.
      onAppHello(entry.scope, () => {
        entry.wantedRev = null
        void fetchEntry(entry)
      }),
    ]
    entry.wantedRev = null
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
    if (entry.fsTimer) {
      clearTimeout(entry.fsTimer)
      entry.fsTimer = null
    }
    // The entry (and its last snapshot) stays registered: a reopened tab
    // paints the cached list at once and refetches. Only its subscriptions
    // end here.
  }
}

export interface UseGitChangesResult extends GitChangesState {
  refetch: () => Promise<void>
}

/** The live Changes snapshot for `workspacePath` on `scope`'s server,
 *  compared against `base` (null = HEAD). One shared store per key. */
export function useGitChanges(
  scope: ServerScope,
  workspacePath: string | null | undefined,
  base: string | null = null,
): UseGitChangesResult {
  const entry = useMemo(
    () => (workspacePath ? getEntry(scope, workspacePath, base) : null),
    [scope, workspacePath, base],
  )
  useEffect(() => (entry ? acquire(entry) : undefined), [entry])
  const state = useStore(entry?.store ?? EMPTY_STORE)
  return {
    ...state,
    refetch: () => (entry ? refetchEntry(entry) : Promise.resolve()),
  }
}

const EMPTY_STORE = createStore<GitChangesState>(() => ({ snapshot: null, loading: false, error: null }))

function refetchEntry(entry: Entry): Promise<void> {
  entry.wantedRev = null
  return fetchEntry(entry)
}

/** Refetch every mounted snapshot of `workspacePath` on `scope` (all
 *  bases) — after a write from this window, so it does not wait for the
 *  event round trip. */
export async function refreshGitChanges(scope: ServerScope, workspacePath: string): Promise<void> {
  const want = normPath(workspacePath)
  const jobs: Promise<void>[] = []
  for (const entry of entries.values()) {
    if (entry.scope.id !== scope.id || entry.workspacePath !== want || entry.refs === 0) continue
    jobs.push(refetchEntry(entry))
  }
  await Promise.all(jobs)
}

// ── Reads ────────────────────────────────────────────────────────────────

/** The `diff-file` mode for a snapshot row (contract §2). */
export function diffParamsFor(file: Pick<ChangesFile, 'group' | 'path' | 'oldPath'>, base: string | null): Record<string, string> {
  const params: Record<string, string> = { file_path: file.path, sides: '1' }
  if (file.oldPath) params.old_path = file.oldPath
  if (file.group === 'base' && base) {
    params.base = base
  } else if (file.group === 'staged') {
    params.mode = 'staged'
  } else if (file.group === 'unstaged') {
    params.mode = 'unstaged'
  } else {
    params.mode = 'worktree'
  }
  return params
}

/** Both sides of one changed file, for the diff viewer. */
export function fetchDiffSides(
  scope: ServerScope,
  workspacePath: string,
  file: Pick<ChangesFile, 'group' | 'path' | 'oldPath'>,
  base: string | null,
): Promise<DiffSides> {
  return daemonCliGet<DiffSides>(scope, 'git/diff-file', { path: normPath(workspacePath), ...diffParamsFor(file, base) })
}

/** Is `path` a git repo on `scope`'s server? Cached per server+path after
 *  the first answer (the + menu reads the cache, then refreshes it). */
const repoCache = new Map<string, boolean>()

export function cachedIsGitRepo(scope: ServerScope, path: string): boolean | undefined {
  return repoCache.get(`${scope.id}\u0000${normPath(path)}`)
}

export async function probeIsGitRepo(scope: ServerScope, path: string): Promise<boolean> {
  const key = `${scope.id}\u0000${normPath(path)}`
  const info = await daemonCliGet<{ isRepo?: boolean }>(scope, 'git/info', { path: normPath(path) })
  const isRepo = info?.isRepo === true
  repoCache.set(key, isRepo)
  return isRepo
}

/** Does the + menu offer Changes for `path` on `scope`'s server? Yes for
 *  every workspace while the server can do git (`git/info.gitAvailable`,
 *  C5); an older daemon without the field: only inside a repo (C2). The
 *  not-a-repo state in the tab offers Initialize repository. */
const changesCache = new Map<string, boolean>()

export function cachedChangesAvailable(scope: ServerScope, path: string): boolean | undefined {
  return changesCache.get(`${scope.id}\u0000${normPath(path)}`)
}

export async function probeChangesAvailable(scope: ServerScope, path: string): Promise<boolean> {
  const key = `${scope.id}\u0000${normPath(path)}`
  const info = await daemonCliGet<{ isRepo?: boolean; gitAvailable?: boolean }>(scope, 'git/info', { path: normPath(path) })
  repoCache.set(key, info?.isRepo === true)
  const yes = typeof info?.gitAvailable === 'boolean' ? info.gitAvailable : info?.isRepo === true
  changesCache.set(key, yes)
  return yes
}

// ── Writes (all POST, all refresh) ───────────────────────────────────────

/** C5 scope add: `git init` at the workspace root (the tab's empty state). */
export async function initRepo(scope: ServerScope, workspacePath: string): Promise<{ success: boolean; repoRoot: string; rev: string | null }> {
  const path = normPath(workspacePath)
  try {
    return await daemonCliPost<{ success: boolean; repoRoot: string; rev: string | null }>(scope, 'git/init', { path })
  } finally {
    await refreshGitChanges(scope, path)
  }
}

export async function stageFiles(scope: ServerScope, workspacePath: string, files: string[]): Promise<void> {
  const path = normPath(workspacePath)
  try {
    for (const filePath of files) {
      await daemonCliPost(scope, 'git/stage', { path, filePath })
    }
  } finally {
    await refreshGitChanges(scope, path)
  }
}

export async function unstageFiles(scope: ServerScope, workspacePath: string, files: string[]): Promise<void> {
  const path = normPath(workspacePath)
  try {
    for (const filePath of files) {
      await daemonCliPost(scope, 'git/unstage', { path, filePath })
    }
  } finally {
    await refreshGitChanges(scope, path)
  }
}

export async function stageAllChanges(scope: ServerScope, workspacePath: string): Promise<void> {
  const path = normPath(workspacePath)
  try {
    await daemonCliPost(scope, 'git/stage-all', { path })
  } finally {
    await refreshGitChanges(scope, path)
  }
}

export async function discardFiles(
  scope: ServerScope,
  workspacePath: string,
  files: string[],
  includeStaged = false,
): Promise<DiscardResult> {
  const path = normPath(workspacePath)
  try {
    return await daemonCliPost<DiscardResult>(scope, 'git/discard', { path, files, includeStaged })
  } finally {
    await refreshGitChanges(scope, path)
  }
}

export async function commitStaged(
  scope: ServerScope,
  workspacePath: string,
  message: string,
): Promise<{ oid: string; message: string }> {
  const path = normPath(workspacePath)
  try {
    return await daemonCliPost<{ oid: string; message: string }>(scope, 'git/commit', { path, message })
  } finally {
    await refreshGitChanges(scope, path)
  }
}

// ── Test seams ───────────────────────────────────────────────────────────

export function __resetGitChangesForTests(): void {
  for (const entry of entries.values()) {
    for (const off of entry.unsubs) off()
    if (entry.fsTimer) clearTimeout(entry.fsTimer)
  }
  entries.clear()
  gitLiveScopes.clear()
  repoCache.clear()
  changesCache.clear()
}

/** Entries with a mounted consumer (live subscriptions). */
export function __gitChangesLiveCountForTests(): number {
  let n = 0
  for (const entry of entries.values()) if (entry.refs > 0) n += 1
  return n
}
