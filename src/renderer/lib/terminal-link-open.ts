// A URL clicked in a terminal (Home M5).
//
// Web links (http, https, mailto): the window's own room asks its daemon
// to open it (`fs/open-external`), as before. A room on another server
// must not: that route opens a browser on THAT server's machine, and it
// is not on the room write allowlist (`kessel/room-writes.ts`). The person
// clicking is at this computer, so a remote room opens the link here.
//
// A `file://` link opens in K2's own file viewer when it names a file
// inside one of the room's workspaces (the ones this person can see);
// the daemon's opener never takes paths (`open_external_target`). Any
// other link type is refused with a short toast, never silently.

import { daemonCliPost } from '@/lib/daemon-cli'
import { scopeMayWrite, type ServerScope } from '@/kessel/server-scope'
import type { RoomProjectsStore } from '@/stores/room'
import { useToastStore } from '@/stores/toast'

export const TERMINAL_LINK_REFUSED = "K2 opens web links from the terminal; this link type isn't opened"

/** What a clicked terminal URL becomes. */
export type TerminalUrlPlan = { kind: 'web' } | { kind: 'file'; path: string } | { kind: 'refused' }

const WEB_SCHEMES = new Set(['http:', 'https:', 'mailto:'])

function slashes(p: string): string {
  return p.replace(/\\/g, '/')
}

/** The local path a `file://` URL names, or null: another host (a UNC
 *  share), no path, or not a file URL. `file:///C:/x` → `C:/x`. */
export function fileUrlPath(url: string): string | null {
  let u: URL
  try {
    u = new URL(url)
  } catch {
    return null
  }
  if (u.protocol !== 'file:') return null
  if (u.host !== '' && u.host.toLowerCase() !== 'localhost') return null
  let path: string
  try {
    path = decodeURIComponent(u.pathname)
  } catch {
    return null
  }
  if (/^\/[A-Za-z]:\//.test(path)) path = path.slice(1)
  return path && path !== '/' ? path : null
}

/** Is `path` a file under one of `roots` (no `..` steps, no root itself)?
 *  Windows paths compare case-insensitively, as the file system does. */
export function insideAnyRoot(path: string, roots: readonly string[]): boolean {
  const p = slashes(path)
  if (p.split('/').some((seg) => seg === '..' || seg === '.')) return false
  const fold = (s: string) => (/^[A-Za-z]:\//.test(s) ? s.toLowerCase() : s)
  return roots.some((root) => {
    const r = slashes(root).replace(/\/+$/, '')
    if (!r) return false
    return fold(p).startsWith(`${fold(r)}/`) && p.length > r.length + 1
  })
}

/** The folders a room's terminal links may open files in: each
 *  workspace and its worktrees. */
export function terminalWorkspaceRoots(
  projects: readonly { path: string; workspaces?: readonly { worktreePath: string | null }[] }[],
): string[] {
  const out: string[] = []
  for (const p of projects) {
    if (p.path) out.push(p.path)
    for (const w of p.workspaces ?? []) if (w.worktreePath) out.push(w.worktreePath)
  }
  return out
}

/** Pure: what to do with a clicked terminal URL. */
export function planTerminalUrl(url: string, roots: readonly string[]): TerminalUrlPlan {
  let protocol: string
  try {
    protocol = new URL(url).protocol
  } catch {
    return { kind: 'refused' }
  }
  if (WEB_SCHEMES.has(protocol)) return { kind: 'web' }
  if (protocol === 'file:') {
    const path = fileUrlPath(url)
    return path && insideAnyRoot(path, roots) ? { kind: 'file', path } : { kind: 'refused' }
  }
  return { kind: 'refused' }
}

export function openTerminalUrl(
  room: { isPrimary: boolean; scope: ServerScope; projects: RoomProjectsStore },
  url: string,
  openFile: (path: string) => void,
): void {
  const plan = planTerminalUrl(url, terminalWorkspaceRoots(room.projects.getState().projects))
  if (plan.kind === 'refused') {
    useToastStore.getState().addToast(TERMINAL_LINK_REFUSED, 'info')
    return
  }
  if (plan.kind === 'file') {
    openFile(plan.path)
    return
  }
  if (!room.isPrimary && !scopeMayWrite(room.scope, 'fs/open-external')) {
    void import('@tauri-apps/plugin-opener')
      .then(({ openUrl }) => openUrl(url))
      .catch((err: unknown) => console.warn('[terminal-link] open here failed', err))
    return
  }
  daemonCliPost(room.scope, 'fs/open-external', { target: url }).catch((err: unknown) =>
    console.warn('[terminal-link]', err),
  )
}
