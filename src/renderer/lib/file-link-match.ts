// Thread file links (Rosson 2026-10-18): find path-shaped text in a Thread
// message. This is TOKENIZING only: what it finds is a candidate, and the
// daemon (`POST /cli/fs/resolve-links`, `k2_core::file_links`) decides
// whether each one is a file or folder inside the message's workspace. A
// candidate the daemon refuses stays plain text.
//
// Shapes:
//   - absolute            /Users/me/ws/src/a.ts
//   - home                ~/ws/src/a.ts
//   - workspace-relative  src/a.ts, ./a.ts, ../x/a.ts, crates/x/y.rs:42
//   - any of them with :line or :line:col
// In prose a relative path needs a slash AND a file extension, a trailing
// slash, a :line, or a ./ ../ start ("and/or" and "client/server" are not
// paths). An inline code span is matched whole, so `Cargo.toml` and
// `src/renderer` count there, and an absolute path in backticks may hold
// spaces ("Alakazam Labs").

export interface FilePathMatch {
  /** Offsets of the whole match (path + :line:col) in the scanned text. */
  start: number
  end: number
  /** The path as printed, without :line:col. */
  path: string
  line?: number
  col?: number
}

export interface MatchOptions {
  /** Spellings of the workspace root (from the daemon) that may contain
   *  spaces: a match may start with one verbatim. */
  prefixes?: readonly string[]
}

const MAX_LINE = 10_000_000

/** Characters a path segment is made of (no slash, no space, no colon). */
function isSegChar(c: string): boolean {
  return /[A-Za-z0-9._\-@+%=~]/.test(c)
}

/** May a path start right after this character? */
function isBoundaryBefore(c: string | undefined): boolean {
  if (c === undefined) return true
  if (/\s/.test(c)) return true
  return '([{<"\'`*_,;|>'.includes(c)
}

/** May a path end right before this character? */
function isBoundaryAfter(c: string | undefined): boolean {
  if (c === undefined) return true
  if (/\s/.test(c)) return true
  return ')]}>"\'`*_,;|.!?:'.includes(c)
}

const COMMON_TLDS = new Set([
  'com', 'org', 'net', 'io', 'dev', 'ai', 'app', 'co', 'gov', 'edu', 'me', 'sh', 'xyz', 'us', 'uk', 'de', 'info', 'biz', 'cloud', 'so', 'gg', 'tv',
])

/** `github.com`, `k2.dev`, `www.x.org`: the first segment of a URL
 *  written without its scheme. */
function looksLikeDomain(segment: string): boolean {
  const m = /^[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*\.([A-Za-z]{2,6})$/.exec(segment)
  return !!m && COMMON_TLDS.has(m[1].toLowerCase())
}

/** A last segment with a file extension (`a.ts`, `.env.local`, `Makefile`
 *  does not count). The extension starts with a letter: `0.46.1` is a
 *  version, not a file. */
function hasExtension(segment: string): boolean {
  return /[^./]\.[A-Za-z][A-Za-z0-9_-]{0,9}$/.test(segment) || /^\.[A-Za-z][A-Za-z0-9_.-]*$/.test(segment)
}

/** Parse `:line[:col]` at `at`. */
function parseSuffix(text: string, at: number): { line?: number; col?: number; end: number } {
  const m = /^:(\d{1,8})(?::(\d{1,8}))?/.exec(text.slice(at))
  if (!m) return { end: at }
  const line = Number(m[1])
  const col = m[2] !== undefined ? Number(m[2]) : undefined
  if (!(line >= 1 && line <= MAX_LINE)) return { end: at }
  if (col !== undefined && !(col >= 1 && col <= MAX_LINE)) return { line, end: at + 1 + m[1].length }
  return { line, col, end: at + m[0].length }
}

type Shape = 'absolute' | 'home' | 'relative'

function shapeOf(path: string): Shape {
  if (path.startsWith('/')) return 'absolute'
  if (path.startsWith('~/')) return 'home'
  return 'relative'
}

function segmentsOf(path: string): string[] {
  return path.split('/').filter((s) => s.length > 0)
}

/** Shared checks for any candidate (prose or code). */
function plausible(path: string): boolean {
  if (!path || path.includes('//') || path.includes('://')) return false
  if (path.startsWith('~') && !path.startsWith('~/')) return false
  const segs = segmentsOf(path.startsWith('~/') ? path.slice(2) : path)
  if (segs.length === 0) return false
  // Every segment needs a letter or digit, except the . and .. walk.
  if (segs.some((s) => s !== '.' && s !== '..' && !/[A-Za-z0-9]/.test(s))) return false
  return true
}

/** Is `path` (with `hasLine` when a :line followed) a path in prose? */
function proseCandidate(path: string, hasLine: boolean): boolean {
  if (!plausible(path)) return false
  const segs = segmentsOf(path)
  switch (shapeOf(path)) {
    case 'absolute':
      return segs.length >= 2
    case 'home':
      return segs.length >= 2
    case 'relative': {
      if (!path.includes('/')) return false
      if (path.startsWith('./') || path.startsWith('../')) return segs.some((s) => s !== '.' && s !== '..')
      if (looksLikeDomain(segs[0])) return false
      if (segs.length < 2) return false
      return hasExtension(segs[segs.length - 1]) || path.endsWith('/') || hasLine
    }
  }
}

/** Every path candidate in a run of prose. */
export function findFilePaths(text: string, opts: MatchOptions = {}): FilePathMatch[] {
  const prefixes = [...(opts.prefixes ?? [])].filter((p) => p.length > 1).sort((a, b) => b.length - a.length)
  const out: FilePathMatch[] = []
  let i = 0
  while (i < text.length) {
    const c = text[i]
    const startable = c === '/' || c === '~' || c === '.' || isSegChar(c)
    if (!startable || !isBoundaryBefore(text[i - 1])) {
      i++
      continue
    }
    let j = i
    for (const p of prefixes) {
      if (text.startsWith(p, i) && (text[i + p.length] === '/' || isBoundaryAfter(text[i + p.length]))) {
        j = i + p.length
        break
      }
    }
    while (j < text.length && (isSegChar(text[j]) || text[j] === '/')) j++
    let path = text.slice(i, j)
    // Sentence punctuation is not part of the path ("see src/a.ts.").
    while (path.length > 1 && /[.]$/.test(path) && !/(^|\/)\.\.?$/.test(path)) path = path.slice(0, -1)
    const pathEnd = i + path.length
    const suffix = pathEnd === j ? parseSuffix(text, j) : { end: pathEnd }
    const end = suffix.end
    if (j > i && isBoundaryAfter(text[end]) && proseCandidate(path, suffix.line !== undefined)) {
      out.push({
        start: i,
        end,
        path,
        ...(suffix.line !== undefined ? { line: suffix.line } : {}),
        ...(suffix.col !== undefined ? { col: suffix.col } : {}),
      })
      i = end
      continue
    }
    // Skip the whole token: nothing inside a rejected token (a URL, a
    // word with a slash) starts a path.
    i = Math.max(j, i + 1)
  }
  return out
}

/** An inline code span that is a path and nothing else, or null. */
export function matchWholeFilePath(code: string): Omit<FilePathMatch, 'start' | 'end'> | null {
  const text = code.trim()
  if (!text || text.length > 4096 || text.includes('\n')) return null
  const m = /^(.*?)(?::(\d{1,8})(?::(\d{1,8}))?)?$/s.exec(text)
  if (!m) return null
  const path = m[1]
  const line = m[2] !== undefined ? Number(m[2]) : undefined
  const col = m[3] !== undefined ? Number(m[3]) : undefined
  if (line !== undefined && !(line >= 1 && line <= MAX_LINE)) return null
  if (col !== undefined && !(col >= 1 && col <= MAX_LINE)) return null
  if (!plausible(path)) return null
  const shape = shapeOf(path)
  if (shape === 'absolute' || shape === 'home') {
    // Spaces allowed (a root like "Alakazam Labs"), but not a command line.
    if (/\s{2,}|\t/.test(path) || /\s-/.test(path) || /\s$/.test(path)) return null
    if (![...path].every((ch) => isSegChar(ch) || ch === '/' || ch === ' ')) return null
    if (segmentsOf(path).length < 2) return null
  } else {
    if (![...path].every((ch) => isSegChar(ch) || ch === '/')) return null
    const segs = segmentsOf(path)
    if (looksLikeDomain(segs[0]) && segs.length > 1) return null
    if (!path.includes('/')) {
      // A bare file name needs an extension: `Cargo.toml`, not `npm`.
      if (!hasExtension(path)) return null
    }
    if (path.startsWith('./') || path.startsWith('../')) {
      if (!segs.some((s) => s !== '.' && s !== '..')) return null
    }
  }
  return { path, ...(line !== undefined ? { line } : {}), ...(col !== undefined ? { col } : {}) }
}
