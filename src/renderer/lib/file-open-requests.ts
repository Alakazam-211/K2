// One-shot requests a Thread file link leaves for the file tab and the Files
// drawer it opens (Thread file links, Rosson 2026-10-18):
//
//   - a line: the next code editor that mounts (or is already showing) the
//     file on that server jumps to it;
//   - a reveal: the Files drawer of that server expands down to a folder,
//     selects it and scrolls it into view.
//
// Requests are keyed by server (`hostKey`) + path and expire after a few
// seconds, so a jump nobody took (a Markdown file opened in its rendered
// view) never fires later.

const TTL_MS = 5_000
/** A background open (Option/Alt-click) leaves its tab unshown: the line
 *  waits until the user switches to it, so it lives much longer. */
const BACKGROUND_TTL_MS = 30 * 60_000

export interface LineRequest {
  line: number
  col?: number
  /** Asked by a background open: apply only once the editor is showing,
   *  and never take focus. */
  background?: boolean
}

type Key = string
const keyOf = (hostKey: string, path: string): Key => `${hostKey}\n${path}`

const lines = new Map<Key, LineRequest & { at: number }>()
const lineListeners = new Set<(key: Key) => void>()

/** Ask the editor showing `path` on `hostKey` to jump to `line` (1-based).
 *  `background`: the file was opened without showing its tab. */
export function requestFileLine(
  hostKey: string,
  path: string,
  line: number,
  col?: number,
  opts?: { background?: boolean },
): void {
  const key = keyOf(hostKey, path)
  lines.set(key, {
    line,
    ...(col !== undefined ? { col } : {}),
    ...(opts?.background ? { background: true } : {}),
    at: Date.now(),
  })
  for (const fn of lineListeners) fn(key)
}

function fresh(key: Key): (LineRequest & { at: number }) | null {
  const req = lines.get(key)
  if (!req) return null
  if (Date.now() - req.at > (req.background ? BACKGROUND_TTL_MS : TTL_MS)) {
    lines.delete(key)
    return null
  }
  return req
}

/** A fresh line request for `path` on `hostKey`, left in place. */
export function peekFileLine(hostKey: string, path: string): LineRequest | null {
  const req = fresh(keyOf(hostKey, path))
  if (!req) return null
  const { at: _at, ...out } = req
  return out
}

/** Take (and clear) a fresh line request for `path` on `hostKey`. */
export function takeFileLine(hostKey: string, path: string): LineRequest | null {
  const key = keyOf(hostKey, path)
  const req = fresh(key)
  lines.delete(key)
  if (!req) return null
  return {
    line: req.line,
    ...(req.col !== undefined ? { col: req.col } : {}),
    ...(req.background ? { background: true } : {}),
  }
}

/** Called whenever a line is requested for `path` on `hostKey`. */
export function onFileLineRequest(hostKey: string, path: string, fn: () => void): () => void {
  const key = keyOf(hostKey, path)
  const listener = (k: Key): void => {
    if (k === key) fn()
  }
  lineListeners.add(listener)
  return () => lineListeners.delete(listener)
}

/** A 1-based line (and column) as an offset in a CodeMirror doc, clamped
 *  to the doc and the line. */
export function lineColToPos(
  doc: { lines: number; line(n: number): { from: number; length: number } },
  line: number,
  col?: number,
): number {
  const n = Math.min(Math.max(1, Math.floor(line)), doc.lines)
  const l = doc.line(n)
  const c = col && col > 0 ? Math.floor(col) - 1 : 0
  return l.from + Math.min(c, l.length)
}

const reveals = new Map<string, { path: string; at: number }>()
const revealListeners = new Set<(hostKey: string) => void>()

/** Ask the Files drawer on `hostKey` to reveal `path` (a folder). */
export function requestFileTreeReveal(hostKey: string, path: string): void {
  reveals.set(hostKey, { path, at: Date.now() })
  for (const fn of revealListeners) fn(hostKey)
}

/** Take (and clear) a fresh reveal request for `hostKey` that lies under
 *  `root` (the asking tree's root). A request for another tree's root is
 *  left for that tree. */
export function takeFileTreeReveal(hostKey: string, root: string): string | null {
  const req = reveals.get(hostKey)
  if (!req) return null
  if (Date.now() - req.at > TTL_MS) {
    reveals.delete(hostKey)
    return null
  }
  if (req.path !== root && !req.path.startsWith(`${root.replace(/\/+$/, '')}/`)) return null
  reveals.delete(hostKey)
  return req.path
}

/** Called whenever a reveal is requested on `hostKey`. */
export function onFileTreeReveal(hostKey: string, fn: () => void): () => void {
  const listener = (k: string): void => {
    if (k === hostKey) fn()
  }
  revealListeners.add(listener)
  return () => revealListeners.delete(listener)
}
