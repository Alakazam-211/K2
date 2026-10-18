// Thread file links against a server without `POST /cli/fs/resolve-links`
// (any daemon before 0.46.1). 0.46.1 RC smoke (Rosson 2026-10-18): the RC
// window showed this Mac's Threads (a 0.46.0 server) and every path in them
// was dead text, because the resolve-links call failed and the resolver
// gave up on the whole Thread.
//
// On such a server the same answer is built from routes 0.46.0 already has:
//   - the Thread's workspace root: `GET /cli/workspace/resolve?q=<ws>`;
//   - the home folder for `~/…`: `GET /cli/fs/info`;
//   - whether the path is a file or a folder: `GET /cli/fs/read-dir` of its
//     parent (the Files drawer's own listing), one listing per folder.
// Containment is lexical (`.` and `..` folded, then the path must be the
// root or under it). The daemon's realpath check (symlink escapes) is the
// 0.46.1 route's; here a link only ever opens what the Files drawer of the
// same workspace already lists, through the ordinary file-open route.

import type { FileLinkAnswer } from '@/components/common/file-links'
import { DaemonHttpError } from '@/lib/daemon-http-error'

export interface LegacyResolveDeps {
  /** The workspace's root path for a workspace token (`workspace/resolve`). */
  resolveWorkspace: (workspace: string) => Promise<string>
  /** The server's home folder (`fs/info`), or null. */
  home: () => Promise<string | null>
  /** A folder's entries (`fs/read-dir`, hidden files included). Throws when
   *  the folder is missing. */
  readDir: (path: string) => Promise<Array<{ name: string; isDirectory: boolean }>>
}

export interface LegacyResolveResponse {
  root: string
  prefixes: string[]
  links: FileLinkAnswer[]
}

/** Was this error a server that does not have `fs/resolve-links` at all
 *  (its top-level method guard, or a login's route-policy miss), as opposed
 *  to the route refusing this request (an unknown workspace)? */
export function isMissingResolveLinksRoute(err: unknown): boolean {
  if (!(err instanceof DaemonHttpError)) return false
  if (err.status === 405 && err.message.includes('method not allowed for this route')) return true
  if (err.status === 404 && (err.message === 'route_unclassified' || err.code === 'route_unclassified')) return true
  return false
}

/** `/a/./b/../c/` → `/a/c`; null when `..` walks above `/`. */
export function normalizeAbsolute(path: string): string | null {
  if (!path.startsWith('/')) return null
  const out: string[] = []
  for (const seg of path.split('/')) {
    if (seg === '' || seg === '.') continue
    if (seg === '..') {
      if (out.length === 0) return null
      out.pop()
      continue
    }
    out.push(seg)
  }
  return `/${out.join('/')}`
}

/** The workspace token of a Thread address: `k2/2` → `k2`. */
export function workspaceTokenOf(addr: string): string {
  const t = addr.trim()
  if (t.startsWith('/')) return t
  return t.split('/')[0]
}

/** The root's spellings a printed path may start with (the daemon's
 *  `root_prefixes`): the root, and `~/…` when it is under home. */
export function legacyRootPrefixes(root: string, home: string | null): string[] {
  const out = [root]
  const h = home ? home.replace(/\/+$/, '') : ''
  if (h && root.startsWith(`${h}/`)) out.push(`~/${root.slice(h.length + 1)}`)
  return out
}

/** Resolve a batch the way `POST /cli/fs/resolve-links` does, from the
 *  routes an older server has. Throws only when the workspace itself is
 *  unknown (every candidate then stays plain, as with the new route). */
export async function legacyResolveLinks(
  deps: LegacyResolveDeps,
  workspace: string,
  paths: string[],
): Promise<LegacyResolveResponse> {
  const rawRoot = (await deps.resolveWorkspace(workspaceTokenOf(workspace))).trim()
  const root = normalizeAbsolute(rawRoot)
  if (!root || root === '/') throw new Error(`workspace has no usable root: ${rawRoot}`)
  let home: string | null = null
  try {
    home = await deps.home()
  } catch {
    home = null
  }
  const listings = new Map<string, Promise<Map<string, boolean> | null>>()
  const listing = (dir: string): Promise<Map<string, boolean> | null> => {
    let p = listings.get(dir)
    if (!p) {
      p = deps
        .readDir(dir)
        .then((entries) => new Map((Array.isArray(entries) ? entries : []).map((e) => [e.name, e.isDirectory === true])))
        .catch(() => null)
      listings.set(dir, p)
    }
    return p
  }

  const resolveOne = async (input: string): Promise<FileLinkAnswer> => {
    const refused: FileLinkAnswer = { input, linkable: false }
    const raw = input.trim()
    if (!raw || raw.length > 4096 || raw.includes('\0')) return refused
    let joined: string
    if (raw === '~' || raw.startsWith('~/')) {
      if (!home || !home.startsWith('/')) return refused
      joined = `${home.replace(/\/+$/, '')}/${raw.replace(/^~\/?/, '')}`
    } else if (raw.startsWith('~')) {
      return refused
    } else if (raw.startsWith('/')) {
      joined = raw
    } else {
      joined = `${root}/${raw}`
    }
    const abs = normalizeAbsolute(joined)
    if (!abs) return refused
    if (abs === root) return { input, linkable: true, kind: 'directory', rel: '', path: rawRoot }
    if (!abs.startsWith(`${root}/`)) return refused
    const rel = abs.slice(root.length + 1)
    const slash = abs.lastIndexOf('/')
    const parent = abs.slice(0, slash) || '/'
    const name = abs.slice(slash + 1)
    const entries = await listing(parent)
    if (!entries || !entries.has(name)) return refused
    const kind = entries.get(name) ? 'directory' : 'file'
    return { input, linkable: true, kind, rel, path: `${rawRoot.replace(/\/+$/, '')}/${rel}` }
  }

  const links = await Promise.all(paths.map(resolveOne))
  return { root: rawRoot, prefixes: legacyRootPrefixes(root, home), links }
}
