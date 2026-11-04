// The Boards Garden's file list (Rosson 2026-11-03): every `.k5board` the viewer may read, across the workspaces on
// this computer, beside the Garden's own boards. Names only for 0.46.8 (no titles, counts or thumbnails).
//
// Daemon-first: the rows are the daemon's `GET /cli/board/file/list` (it walks the workspaces and checks each file
// with the same rights as a read; an agent sees only its own). This only reads, reads again after a change, and opens
// a row's tab. The route has no watcher, so a change is K2's `board_file_changed` (a save through K2) or an
// `fs_changed` that names a `.k5board` (an agent's shell, another client), debounced: never on a timer.
//
// Only K2's own Board widget gets it, and only while the server says `board-file-v1` (`board_file_preview`): with the
// switch off the frame's hello has no `board-file-list-v1` and the Garden looks exactly as before.

import type { ServerScope } from '@/kessel/server-scope'
import { daemonCliGet } from '@/lib/daemon-cli'
import { onBoardFileChanged, onFsChanged } from '@/stores/session-events'
import { primaryProjectForRoot, primaryRoom } from '@/stores/room'
import { usePageViewStore } from '@/stores/page-view'
import { exitZen } from '@/lib/zen/zen-view'
import { withThreadWorkspaceShowing } from '@/lib/thread-file-link-open'
import type { ZenCustomBoardFiles } from '@/lib/zen/zen-custom-bridge'

/** The widget that lists board files: K2's own Board, never a custom widget (an agent's widget could post the names). */
export const BOARD_FILE_LIST_WIDGET = 'k2:board@1'
/** The frame feature that turns `boardFile.files` / `boardFile.openTab` on in a Garden. */
export const BOARD_FILE_LIST_FEATURE = 'board-file-list-v1'
/** Changes closer together than this are one re-list. */
export const BOARD_FILE_RELIST_MS = 400

export interface BoardFileRow {
  path: string
  rel: string
  name: string
  workspace: string
  workspaceName: string
}

export interface BoardFileList {
  boards: BoardFileRow[]
  truncated: boolean
}

/** What the widget gets: no absolute paths cross into a frame (UWA6), so a row has an id this listing keeps. */
export interface BoardFileFrameRow {
  id: string
  name: string
  rel: string
  workspaceName: string
}
export interface BoardFileFrameList {
  boards: BoardFileFrameRow[]
  truncated: boolean
}

const str = (v: unknown): v is string => typeof v === 'string'

/** The route's answer, checked: a row without its fields is a daemon bug, said, never drawn half. */
export function parseBoardFileList(raw: unknown): BoardFileList {
  if (!raw || typeof raw !== 'object') throw new Error('board/file/list: not an object')
  const r = raw as { boards?: unknown; truncated?: unknown }
  if (!Array.isArray(r.boards)) throw new Error('board/file/list: no boards list')
  if (typeof r.truncated !== 'boolean') throw new Error('board/file/list: no truncated flag')
  const boards = r.boards.map((b, i): BoardFileRow => {
    const x = (b ?? {}) as Record<string, unknown>
    if (!str(x.path) || !str(x.rel) || !str(x.name) || !str(x.workspace) || !str(x.workspaceName)) {
      throw new Error(`board/file/list: row ${i} is missing path, rel, name, workspace or workspaceName`)
    }
    return { path: x.path, rel: x.rel, name: x.name, workspace: x.workspace, workspaceName: x.workspaceName }
  })
  return { boards, truncated: r.truncated }
}

const isBoardPath = (p: string): boolean => /\.k5board$/i.test(p)

export interface BoardFileListingDeps {
  load(): Promise<unknown>
  onBoardFileChanged(fn: () => void): () => void
  onFsChanged(fn: (paths: readonly string[]) => void): () => void
  setTimer(fn: () => void, ms: number): unknown
  clearTimer(t: unknown): void
}

export interface BoardFileListing {
  /** The list now, then again after each change that alters it. `fail` once, when the first read is refused. */
  subscribe(push: (list: BoardFileFrameList) => void, fail: (err: unknown) => void): () => void
  /** The daemon's row behind a frame row's id, or null: a tab opens only for a file the daemon listed. */
  row(id: string): BoardFileRow | null
}

export function createBoardFileListing(deps: BoardFileListingDeps): BoardFileListing {
  // Ids stay the same for a path while this listing lives (a re-list doesn't renumber the rows on screen).
  const ids = new Map<string, string>()
  let nextId = 1
  const idOf = (path: string): string => {
    let id = ids.get(path)
    if (!id) {
      id = `bf-${nextId++}`
      ids.set(path, id)
    }
    return id
  }
  let rows = new Map<string, BoardFileRow>()
  return {
    row: (id) => rows.get(id) ?? null,
    subscribe(push, fail) {
      let live = true
      let first = true
      let timer: unknown = null
      let inFlight = false
      let again = false
      let last = ''
      const read = (): void => {
        if (!live) return
        if (inFlight) {
          again = true
          return
        }
        inFlight = true
        deps
          .load()
          .then(parseBoardFileList)
          .then((list) => {
            if (!live) return
            rows = new Map(list.boards.map((b) => [idOf(b.path), b]))
            const out: BoardFileFrameList = {
              boards: list.boards.map((b) => ({ id: idOf(b.path), name: b.name, rel: b.rel, workspaceName: b.workspaceName })),
              truncated: list.truncated,
            }
            const sig = JSON.stringify(out)
            if (first || sig !== last) push(out)
            last = sig
          })
          .catch((err: unknown) => {
            if (!live) return
            if (first) {
              live = false
              stop()
              fail(err)
              return
            }
            // A later read failing keeps the last list on screen; the next change reads again.
            console.warn('[board-file] re-listing board files failed:', err)
          })
          .finally(() => {
            first = false
            inFlight = false
            if (again && live) {
              again = false
              read()
            }
          })
      }
      const soon = (): void => {
        if (!live) return
        if (timer !== null) deps.clearTimer(timer)
        timer = deps.setTimer(() => {
          timer = null
          read()
        }, BOARD_FILE_RELIST_MS)
      }
      const offSaved = deps.onBoardFileChanged(soon)
      const offFs = deps.onFsChanged((paths) => {
        if (paths.some(isBoardPath)) soon()
      })
      function stop(): void {
        offSaved()
        offFs()
        if (timer !== null) deps.clearTimer(timer)
        timer = null
      }
      read()
      return () => {
        if (!live) return
        live = false
        stop()
      }
    },
  }
}

/** The listing on `scope`'s daemon (a Garden's: this computer's). */
export function boardFileListingFor(scope: ServerScope): BoardFileListing {
  return createBoardFileListing({
    load: () => daemonCliGet<unknown>(scope, 'board/file/list'),
    onBoardFileChanged: (fn) => onBoardFileChanged(scope, () => fn()),
    onFsChanged: (fn) => onFsChanged(scope, (e) => fn(e.paths)),
    setTimer: (fn, ms) => window.setTimeout(fn, ms),
    clearTimer: (t) => window.clearTimeout(t as number),
  })
}

/**
 * Leave the Garden for the row's Board tab, in its own workspace on the Agents page. False when this window can't
 * show it: the window is on another server, or its workspace isn't one this window lists.
 */
export function openBoardFileRowInK2(row: BoardFileRow): boolean {
  const room = primaryRoom()
  if (room.scope.isRemote || !primaryProjectForRoot(row.workspace)) return false
  exitZen()
  usePageViewStore.getState().setPage('agents')
  withThreadWorkspaceShowing(room, row.workspace, () => {
    room.tabs.getState().openFileAsTab(row.path)
  })
  return true
}

/** What a Garden frame of K2's own Board gets (`deps.boardFiles`): the listing, the press rule, and a tab by row id. */
export function boardFilesForFrame(scope: ServerScope, pressed: () => boolean): ZenCustomBoardFiles {
  const listing = boardFileListingFor(scope)
  return {
    files: (push, fail) => listing.subscribe(push, fail),
    pressed,
    openTab: async (id) => {
      const row = listing.row(id)
      return row ? openBoardFileRowInK2(row) : false
    },
  }
}
