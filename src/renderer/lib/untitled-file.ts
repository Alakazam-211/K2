// New "Untitled" files (Cmd+N, the tab + menu). A new document is a tab, not a
// file, until its first save. The daemon owns the naming:
//   - `fs/untitled-name` suggests the name when the tab opens: free on disk
//     and not an open tab here. It writes nothing.
//   - `fs/create-untitled` is the first save: it creates the file atomically,
//     under the tab's name if that's still free, else the next free one. It
//     never overwrites, so two clients saving the same name both keep their
//     text. The answer's `name` is what the tab becomes.
//
// A server older than 0.46.8 has neither route: naming stays local and the
// first save is a plain write, as they always were there.

import { daemonCliPost, isHostSwitchedError } from '@/lib/daemon-cli'
import { isMissingResolveLinksRoute as isMissingRoute } from '@/lib/file-link-legacy'
import type { ServerScope } from '@/kessel/server-scope'

const UNTITLED_NAME = /^Untitled(?:-\d+)?(?:\.[^/\\]+)?$/

export function pathBaseName(path: string): string {
  return path.split(/[\\/]/).pop() ?? path
}

export function isUntitledFileName(name: string): boolean {
  return UNTITLED_NAME.test(name)
}

/** This tab holds a document that may not be a file yet: an Untitled name the
 *  pane has never read as text. Its first write must not overwrite. */
export function isNewUntitledFile(path: string, lastReadPath: string | null): boolean {
  return lastReadPath !== path && isUntitledFileName(pathBaseName(path))
}

interface TabLike {
  id: string
  title: string
  paneGroups: Map<string, { items: Array<{ type: string; data: unknown }> }>
}

/** The Untitled names this window's tabs hold (they may not be files yet), but
 *  for `exceptTabId`, which is the one asking. */
export function untitledNamesInUse(tabs: TabLike[], exceptTabId?: string): string[] {
  const names = new Set<string>()
  for (const tab of tabs) {
    if (tab.id === exceptTabId) continue
    if (isUntitledFileName(tab.title)) names.add(tab.title)
    for (const pg of tab.paneGroups.values()) {
      for (const item of pg.items) {
        if (item.type !== 'file-viewer') continue
        const name = pathBaseName((item.data as { filePath: string }).filePath)
        if (isUntitledFileName(name)) names.add(name)
      }
    }
  }
  return [...names]
}

/** The first of `Untitled`, `Untitled-2`, ... not in `taken`. Doesn't know the
 *  disk: only for when the daemon can't say. */
export function localUntitledName(taken: string[]): string {
  if (!taken.includes('Untitled')) return 'Untitled'
  let n = 2
  while (taken.includes(`Untitled-${n}`)) n += 1
  return `Untitled-${n}`
}

function answeredName(res: unknown): string {
  const name = (res as { name?: unknown } | null)?.name
  if (typeof name !== 'string' || !name) throw new Error('the daemon answered without a name')
  return name
}

/** The name for a new document in `dir`. Never fails over a daemon problem: a
 *  person pressing Cmd+N still gets a tab, and its first save is still safe.
 *  Throws only when the window has left this host (no tab belongs here). */
export async function pickUntitledName(scope: ServerScope, dir: string, taken: string[]): Promise<string> {
  try {
    return answeredName(await daemonCliPost(scope, 'fs/untitled-name', { dir, taken }))
  } catch (err) {
    if (isHostSwitchedError(err)) throw err
    if (!isMissingRoute(err)) console.warn('[untitled] the daemon gave no name, numbering from the open tabs:', err)
    return localUntitledName(taken)
  }
}

/** The first save of a new document. Returns the path it landed on, which isn't
 *  `filePath` when someone took that name since the tab opened. */
export async function saveNewUntitledFile(
  scope: ServerScope,
  filePath: string,
  content: string,
  taken: string[],
): Promise<string> {
  const name = pathBaseName(filePath)
  const prefix = filePath.slice(0, filePath.length - name.length)
  const dir = prefix.length > 1 ? prefix.slice(0, -1) : prefix
  try {
    const res = await daemonCliPost(scope, 'fs/create-untitled', { dir, prefer: name, taken, content })
    return `${prefix}${answeredName(res)}`
  } catch (err) {
    if (!isMissingRoute(err)) throw err
    await daemonCliPost(scope, 'fs/write-file', { path: filePath, content })
    return filePath
  }
}
