// Every board file session in this window, and the guards that keep held edits from going silently.
//
// Held edits live in the tab's memory for dogfood, on K2 Cores' three conditions (they must survive a restart before
// `board_file_preview` turns on for customers):
//   1. The tab says the edit isn't saved while it's held (`status()` in the session, the chip in BoardFilePane).
//   2. Closing the tab or quitting asks first (`askBeforeBoardFileLoss`), and K2's auto-update or restart doesn't
//      discard one silently (`update.ts` asks through the same door).
//      If K2 still goes down with one (a crash, a force quit), a note says so on reopen (`note*` below).
//   3. Save a copy works when the original location is the problem (`saveCopy` in the session).
//
// A session outlives its pane while it holds an edit, so a workspace switch or a tab closed by a door that doesn't ask
// (those unmount the pane) doesn't lose it: the next pane on that file picks it up.

import { invoke } from '@tauri-apps/api/core'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { getFileCategory } from '@/components/FileViewerPane/fileCategory'
import { createBoardFileSession, type BoardFileApi, type BoardFileSession } from './board-file-session'

interface Entry {
  key: string
  path: string
  session: BoardFileSession
  panes: number
}

const entries = new Map<string, Entry>()

const NOTE_PREFIX = 'k2.boardFile.unsaved:'
const noteKey = (key: string): string => `${NOTE_PREFIX}${key}`

function readNote(key: string): { edits: number } | null {
  try {
    const raw = localStorage.getItem(noteKey(key))
    if (!raw) return null
    const v = JSON.parse(raw) as { edits?: unknown }
    return typeof v.edits === 'number' && v.edits > 0 ? { edits: v.edits } : null
  } catch (err) {
    console.warn('[board-file] the unsaved note can’t be read:', err)
    return null
  }
}
/** What this tab couldn't save yet. A note, never the edits: it only lets a reopened tab say something went. */
function writeNote(key: string, edits: number): void {
  try {
    if (edits > 0) localStorage.setItem(noteKey(key), JSON.stringify({ edits }))
    else localStorage.removeItem(noteKey(key))
  } catch (err) {
    console.warn('[board-file] the unsaved note can’t be written:', err)
  }
}

export interface AcquireOptions {
  connectionKey: string
  path: string
  api: BoardFileApi
  visible(): boolean
}

/** The session for a file in this window: the one still holding edits, or a new one. */
export function acquireBoardFileSession(o: AcquireOptions): BoardFileSession {
  const key = `${o.connectionKey}\n${o.path}`
  const have = entries.get(key)
  if (have) {
    have.panes += 1
    return have.session
  }
  const session = createBoardFileSession({
    api: o.api,
    path: o.path,
    setTimer: (fn, ms) => window.setTimeout(fn, ms),
    clearTimer: (t) => window.clearTimeout(t as number),
    now: () => Date.now(),
    visible: o.visible,
  })
  const entry: Entry = { key, path: o.path, session, panes: 1 }
  entries.set(key, entry)
  // K2 went down since this file last had edits it couldn't save, and nothing here holds them now.
  const note = readNote(key)
  if (note) session.setLostNote(note)
  session.setDirtyListener(() => {
    writeNote(key, session.heldCount())
    tellShell()
  })
  return session
}

/** A pane is gone. Queued edits flush; the session stays while anything is still unsaved. */
export async function releaseBoardFileSession(connectionKey: string, path: string, opts: { discard: boolean }): Promise<void> {
  const key = `${connectionKey}\n${path}`
  const e = entries.get(key)
  if (!e) return
  e.panes -= 1
  if (e.panes > 0) return
  if (opts.discard) e.session.discard()
  else await e.session.flush().catch((err: unknown) => console.warn('[board-file] the flush on close failed:', err))
  if (e.panes > 0) return
  if (e.session.pendingCount() + e.session.heldCount() === 0) {
    writeNote(key, 0)
    e.session.dispose()
    entries.delete(key)
  }
}

/** The file's session, if this window has one (the tab bar's close door). */
export function boardFileSessionFor(connectionKey: string, path: string): BoardFileSession | null {
  return entries.get(`${connectionKey}\n${path}`)?.session ?? null
}

/** The shell's Quit asks only while something here can't be saved yet (Cmd+Q never reaches the page by itself). */
let toldShell = false
function tellShell(): void {
  const unsaved = unsavedBoardFiles().length > 0
  if (unsaved === toldShell) return
  toldShell = unsaved
  invoke('set_board_file_unsaved', { unsaved }).catch((err: unknown) => console.warn('[board-file] the shell didn’t hear about unsaved edits:', err))
}

/** The sessions of a tab's board files that still hold something unsaved (the doors that close a tab). */
export function unsavedBoardSessionsInTab(
  tab: { paneGroups: Map<string, { items: readonly { type: string; data?: unknown }[] }> } | undefined,
  connectionKey: string,
): BoardFileSession[] {
  if (!tab) return []
  const out: BoardFileSession[] = []
  for (const pg of tab.paneGroups.values()) {
    for (const item of pg.items) {
      if (item.type !== 'file-viewer') continue
      const path = (item.data as { filePath?: unknown } | undefined)?.filePath
      if (typeof path !== 'string' || getFileCategory(path) !== 'board') continue
      const s = boardFileSessionFor(connectionKey, path)
      if (s && s.pendingCount() + s.heldCount() > 0) out.push(s)
    }
  }
  return out
}

export function unsavedBoardFiles(): { path: string; queued: number; held: number; session: BoardFileSession }[] {
  const out = []
  for (const e of entries.values()) {
    const queued = e.session.pendingCount()
    const held = e.session.heldCount()
    if (queued + held > 0) out.push({ path: e.path, queued, held, session: e.session })
  }
  return out
}

const baseName = (p: string): string => p.split(/[\\/]/).pop() ?? p

/**
 * Before something would drop what isn't saved: flush what can go, then ask about the rest. True to go on, and then the
 * edits are let go at the person's say-so; false to stay. Only `sessions` are asked about (default: every open file).
 */
export async function askBeforeBoardFileLoss(
  what: 'close' | 'quit' | 'update',
  only?: readonly BoardFileSession[],
): Promise<boolean> {
  const inScope = (s: BoardFileSession): boolean => !only || only.includes(s)
  await Promise.all(
    unsavedBoardFiles()
      .filter((f) => inScope(f.session))
      .map((f) =>
        f.session.flush().catch((err: unknown) => console.warn(`[board-file] flushing ${f.path} failed:`, err)),
      ),
  )
  const left = unsavedBoardFiles().filter((f) => inScope(f.session))
  if (left.length === 0) return true
  const count = left.reduce((n, f) => n + f.queued + f.held, 0)
  const names = left.map((f) => baseName(f.path)).slice(0, 3).join(', ')
  const edits = count === 1 ? '1 edit isn’t saved' : `${count} edits aren’t saved`
  const action = what === 'close' ? 'Close without saving' : what === 'quit' ? 'Quit without saving' : 'Update without saving'
  const go = await useConfirmDialogStore.getState().confirm({
    title: edits,
    message: `${names}: ${count === 1 ? 'this edit' : 'these edits'} will be lost. Stay to resolve ${count === 1 ? 'it' : 'them'} or use Save a copy.`,
    confirmLabel: action,
    destructive: true,
  })
  if (go) for (const f of left) f.session.discard()
  return go
}

/** Tests: the one open file's session, whichever server the test's room is on. */
export function onlyBoardFileSessionForTests(): BoardFileSession {
  const all = [...entries.values()]
  if (all.length !== 1) throw new Error(`${all.length} board file sessions are open`)
  return all[0].session
}

/** Tests. */
export function resetBoardFileTabsForTests(): void {
  for (const e of entries.values()) e.session.dispose()
  entries.clear()
}
