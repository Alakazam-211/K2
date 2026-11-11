// One builder for the text a file path becomes when it lands in a terminal
// or a compose draft from an external drop (Finder, a browser upload), a
// Finder paste or a picked file. Pure: no stores, no DOM. Callers turn a refusal into a toast
// (`drop-refusal.ts`).
//
// Quoting: backslash before every shell special, like iTerm2 and
// Terminal.app (the same in bash, zsh and fish). Image and PDF paths are
// single-quoted instead, see `quotePathForImageDrop`.
//
// A name with a control character (a line break, tab, ESC, NUL...) is
// refused: no quoting works the same in every shell, and a line break
// typed into a shell runs what follows it.

// ── Image detection ──────────────────────────────────────────────────
// Claude Code's `[Image #N]` detector reads the line, strips quotes, and
// fs.exists()s the path. Backslash-escaped spaces (e.g. `Screen\ Shot.png`)
// break that check, so for images we skip the backslash escape and only
// wrap paths with whitespace/specials in single quotes.

const IMAGE_EXTS = [
  '.png', '.jpg', '.jpeg', '.gif', '.webp', '.bmp', '.heic', '.heif', '.pdf',
]

export function isImagePath(p: string): boolean {
  const lower = p.toLowerCase()
  return IMAGE_EXTS.some((ext) => lower.endsWith(ext))
}

/**
 * Quote a path minimally: bare if safe, single-quoted otherwise. Unlike
 * backslash-escaping this preserves the path's literal contents so
 * downstream readers (Claude Code's image detector) can resolve it.
 */
export function quotePathForImageDrop(p: string): string {
  if (/[^a-zA-Z0-9_\-./]/.test(p)) {
    return "'" + p.replace(/'/g, "'\\''") + "'"
  }
  return p
}

// ── Bracketed paste ──────────────────────────────────────────────────
// Interactive apps that enable bracketed paste (Claude Code, zsh, bash,
// vim, etc.) toggle a flag when they see `\e[200~` / `\e[201~`. Claude
// Code's image detector only runs on paste events; raw typed chars
// never trigger it. Terminal.app / iTerm2 wrap drag-drop in these
// escape sequences natively, which is why `[Image #N]` works there.

export const BRACKETED_PASTE_START = '\x1b[200~'
export const BRACKETED_PASTE_END = '\x1b[201~'

/** Wrap text as a bracketed paste so the foreground app's paste handler
 *  sees a single paste event (Claude Code then runs image detection). */
export function bracketPaste(text: string): string {
  return BRACKETED_PASTE_START + text + BRACKETED_PASTE_END
}

// ── Refusal ──────────────────────────────────────────────────────────

/** The toast for a refused path (K2's wording, used for every control
 *  character, not only a line break). */
export const CONTROL_CHAR_REFUSAL = "Can't drop a file whose name has a line break in it."

/** C0 controls (NUL..US, so \n \r \t ESC), DEL, and C1 controls
 *  (0x80-0x9F: 0x9B is a one-byte CSI to some terminals). */
const CONTROL_CHAR = /[\u0000-\u001f\u007f-\u009f]/

export function hasControlChar(p: string): boolean {
  return CONTROL_CHAR.test(p)
}

export type DropText = { ok: true; text: string } | { ok: false; refusal: string }

// ── Quoting ──────────────────────────────────────────────────────────

/** One path, quoted for a POSIX shell. Never call with a control character. */
export function quoteDropPath(p: string): string {
  return isImagePath(p)
    ? quotePathForImageDrop(p)
    : p.replace(/[ '"\\()&|;<>$`!#*?[\]{}~]/g, '\\$&')
}

export interface DropTextOptions {
  /** A space after the last path so typing can go on (default true). */
  trailingSpace?: boolean
}

/** The paths as space-joined text, quoted. No bracketed
 *  paste: for a compose draft or the clipboard. */
export function formatDropPaths(paths: string[], opts: DropTextOptions = {}): DropText {
  if (paths.some(hasControlChar)) return { ok: false, refusal: CONTROL_CHAR_REFUSAL }
  if (paths.length === 0) return { ok: true, text: '' }
  const text = paths.map(quoteDropPath).join(' ')
  return { ok: true, text: opts.trailingSpace === false ? text : text + ' ' }
}

/**
 * The text a terminal receives for dropped or pasted paths. If any path
 * is an image the whole payload is a bracketed paste, so Claude Code's
 * paste-event image detector fires and shows `[Image #N]`. Shells with
 * bracketed paste on (zsh, bash, vim) see a normal paste.
 */
export function buildTerminalDropPayload(paths: string[]): DropText {
  const r = formatDropPaths(paths)
  if (!r.ok) return r
  return { ok: true, text: paths.some(isImagePath) ? bracketPaste(r.text) : r.text }
}

/**
 * Paths for an agent's compose draft: quoted, space-joined, trailing
 * space for continued typing. No bracketed paste (that's a TTY paste
 * event; the composer is a plain text field).
 */
export function buildComposeDropPayload(paths: string[]): DropText {
  return formatDropPaths(paths)
}
