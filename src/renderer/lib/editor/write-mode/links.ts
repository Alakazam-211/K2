// Links and images in Write and Read (build-editors.md E2). A relative link
// or image resolves against the markdown file's folder; the host decides how
// a file opens (a tab in the room) and how image bytes load (the room's
// server). External links go to the browser through the host as well, so
// nothing here reaches Tauri or the network on its own.

import { Facet } from '@codemirror/state'

export interface WriteLinks {
  /** The markdown file's own path: relative links and images resolve against its folder. */
  filePath: string
  /** An image's bytes as a URL the page can show (an object URL), for a resolved path. */
  loadImage?: (path: string) => Promise<string>
  /** Open a file link (resolved, with any #anchor dropped). */
  openFile?: (path: string) => void
  /** Open an http, https or mailto link outside K2. */
  openExternal?: (url: string) => void
}

export const writeLinks = Facet.define<WriteLinks, WriteLinks>({
  combine: (v) => v[0] ?? { filePath: '' },
})

const EXTERNAL = /^(https?:|mailto:)/i
const SCHEME = /^[a-z][a-z0-9+.-]*:/i

/** An http, https or mailto link? */
export const isExternal = (href: string): boolean => EXTERNAL.test(href.trim())

/** `href` as a path on the file's computer, resolved against the file's folder,
 *  or null for an anchor, an empty link or another scheme. */
export function resolveLinkPath(filePath: string, href: string): string | null {
  let h = href.trim()
  if (h.startsWith('<') && h.endsWith('>')) h = h.slice(1, -1)
  h = h.split('#')[0].split('?')[0]
  if (!h || (SCHEME.test(h) && !DRIVE.test(h))) return null
  try {
    h = decodeURIComponent(h)
  } catch {
    // keep it as typed
  }
  const sep = DRIVE.test(filePath) || (!filePath.includes('/') && filePath.includes('\\')) ? '\\' : '/'
  const absolute = h.startsWith('/') || DRIVE.test(h)
  const start = absolute ? splitRoot(h, sep) : splitRoot(filePath, sep)
  const segs = absolute ? [] : start.segs.slice(0, -1)
  for (const p of absolute ? start.segs : h.split(/[\\/]/)) {
    if (p === '' || p === '.') continue
    if (p === '..') segs.pop()
    else segs.push(p)
  }
  return start.root + segs.join(sep)
}

const DRIVE = /^[a-z]:[\\/]/i
function splitRoot(p: string, sep: string): { root: string; segs: string[] } {
  if (DRIVE.test(p)) return { root: p.slice(0, 2) + sep, segs: p.slice(3).split(/[\\/]/) }
  if (p.startsWith('/')) return { root: '/', segs: p.slice(1).split(/[\\/]/) }
  return { root: '', segs: p.split(/[\\/]/) }
}

/** Open a link the way the host says: external, then file. */
export function openLink(links: WriteLinks, href: string): boolean {
  if (isExternal(href)) {
    if (!links.openExternal) return false
    links.openExternal(href.trim())
    return true
  }
  const path = resolveLinkPath(links.filePath, href)
  if (!path || !links.openFile) return false
  links.openFile(path)
  return true
}
