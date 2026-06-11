/**
 * Maps browser KeyboardEvent to terminal byte sequences.
 *
 * This covers the standard xterm/VT100 escape sequences for all common keys.
 * The `mode` parameter carries alacritty_terminal TermMode flags so we can
 * send the correct sequences for APP_CURSOR mode (used by shells like zsh).
 *
 * Reference: alacritty/alacritty/src/input.rs, Zed's keys.rs, xterm ctlseqs.
 */

// ── Terminal mode flags (must match GridUpdate.mode from Rust) ──────────

export const MODE_APP_CURSOR = 1 << 1
export const MODE_APP_KEYPAD = 1 << 2
export const MODE_BRACKETED_PASTE = 1 << 4

// ── Modifier helpers ────────────────────────────────────────────────────

/** Returns the CSI modifier parameter for the given modifier state (1-indexed). */
function csiModifier(e: KeyboardEvent): number {
  let mod = 1
  if (e.shiftKey) mod += 1
  if (e.altKey) mod += 2
  if (e.ctrlKey) mod += 4
  if (e.metaKey) mod += 8
  return mod
}

// ── Arrow keys ──────────────────────────────────────────────────────────

const ARROW_KEYS: Record<string, string> = {
  ArrowUp: 'A',
  ArrowDown: 'B',
  ArrowRight: 'C',
  ArrowLeft: 'D',
}

// ── Function keys ───────────────────────────────────────────────────────

const FUNCTION_KEYS: Record<string, string> = {
  F1: '\x1bOP',
  F2: '\x1bOQ',
  F3: '\x1bOR',
  F4: '\x1bOS',
  F5: '\x1b[15~',
  F6: '\x1b[17~',
  F7: '\x1b[18~',
  F8: '\x1b[19~',
  F9: '\x1b[20~',
  F10: '\x1b[21~',
  F11: '\x1b[23~',
  F12: '\x1b[24~',
}

// CSI codes for function keys (for modifier encoding)
const FUNCTION_KEY_CODES: Record<string, number> = {
  F1: 11, F2: 12, F3: 13, F4: 14,
  F5: 15, F6: 17, F7: 18, F8: 19,
  F9: 20, F10: 21, F11: 23, F12: 24,
}

// ── Main mapper ─────────────────────────────────────────────────────────

/**
 * Convert a KeyboardEvent to the terminal byte sequence.
 * Returns null if the key should not be sent (e.g. Cmd+C for copy).
 *
 * @param e - The keyboard event
 * @param mode - Terminal mode flags from GridUpdate.mode (TermMode bits)
 */
export function keyEventToSequence(e: KeyboardEvent, mode: number = 0): string | null {
  const { key } = e
  const appCursor = (mode & MODE_APP_CURSOR) !== 0

  // ── Meta (Cmd) shortcuts — let browser/app handle ─────────────────
  // Don't intercept Cmd+C, Cmd+V, Cmd+A, Cmd+T, Cmd+W, etc.
  if (e.metaKey && !e.ctrlKey && !e.altKey) {
    // Only allow Cmd+arrow (for natural text editing, handled elsewhere)
    if (!key.startsWith('Arrow')) {
      return null
    }
  }

  // ── Arrow keys ────────────────────────────────────────────────────
  // Critical: shells like zsh enable APP_CURSOR mode, which changes
  // arrow key sequences from CSI (\x1b[) to SS3 (\x1bO) format.
  // Sending the wrong format causes the shell to not recognize cursor
  // movement, which breaks backspace from arbitrary positions.
  if (key in ARROW_KEYS) {
    const dir = ARROW_KEYS[key]
    const mod = csiModifier(e)
    if (mod > 1) {
      // Modifiers always use CSI format
      return `\x1b[1;${mod}${dir}`
    }
    // APP_CURSOR mode: use SS3 format (\x1bO)
    // Normal mode: use CSI format (\x1b[)
    return appCursor ? `\x1bO${dir}` : `\x1b[${dir}`
  }

  // ── Function keys ─────────────────────────────────────────────────
  if (key in FUNCTION_KEYS) {
    const mod = csiModifier(e)
    if (mod > 1) {
      const code = FUNCTION_KEY_CODES[key]
      return `\x1b[${code};${mod}~`
    }
    return FUNCTION_KEYS[key]
  }

  // ── Navigation keys ───────────────────────────────────────────────
  // Home/End also respect APP_CURSOR mode
  switch (key) {
    case 'Home': {
      const mod = csiModifier(e)
      if (mod > 1) return `\x1b[1;${mod}H`
      return appCursor ? '\x1bOH' : '\x1b[H'
    }
    case 'End': {
      const mod = csiModifier(e)
      if (mod > 1) return `\x1b[1;${mod}F`
      return appCursor ? '\x1bOF' : '\x1b[F'
    }
    case 'Insert': return '\x1b[2~'
    case 'Delete': {
      if (e.altKey) return '\x1bd' // handled by natural text editing, but fallback
      return '\x04' // Ctrl+D — matches iTerm2's forward delete (delete char at cursor)
    }
    case 'PageUp': return '\x1b[5~'
    case 'PageDown': return '\x1b[6~'
  }

  // ── Special keys ──────────────────────────────────────────────────
  switch (key) {
    case 'Enter':
      if (e.shiftKey) return '\n'  // Shift+Enter: newline (for multi-line input in Claude CLI etc.)
      return '\r'
    case 'Backspace':
      // Backspace always sends DEL (0x7f) — correct for modern terminals.
      // Ctrl+Backspace → ESC+DEL: bound to backward-kill-word in both
      // zsh's default emacs keymap and GNU readline (bash). Sending
      // \x08 used to be the old Unix convention but modern shells
      // bind it identically to DEL (backward-delete-char).
      // Option+Backspace → same ESC+DEL, matches macOS "Natural Text
      // Editing" iTerm2 preset which users expect.
      if (e.ctrlKey) return '\x1b\x7f'
      if (e.altKey) return '\x1b\x7f'
      return '\x7f'
    case 'Tab':
      if (e.shiftKey) return '\x1b[Z'  // Shift+Tab (backtab)
      return '\t'
    case 'Escape':
      return '\x1b'
  }

  // ── Ctrl+key → control characters ─────────────────────────────────
  if (e.ctrlKey && !e.altKey && !e.metaKey && key.length === 1) {
    const upper = key.toUpperCase()
    const code = upper.charCodeAt(0)
    // Ctrl+A (0x01) through Ctrl+Z (0x1A)
    if (code >= 65 && code <= 90) {
      return String.fromCharCode(code - 64)
    }
    switch (upper) {
      case '[': return '\x1b'
      case '\\': return '\x1c'
      case ']': return '\x1d'
      case '^': return '\x1e'
      case '_': return '\x1f'
      case '@': return '\x00'
      case '/': return '\x1f'
    }
  }

  // ── Alt+key → ESC prefix ──────────────────────────────────────────
  if (e.altKey && !e.ctrlKey && !e.metaKey && key.length === 1) {
    return `\x1b${key}`
  }

  // ── Printable characters ──────────────────────────────────────────
  if (key.length === 1 && !e.ctrlKey && !e.metaKey) {
    return key
  }

  // ── Dead keys, unrecognized — don't send ──────────────────────────
  return null
}

// ── Natural text editing sequences ──────────────────────────────────────

/**
 * Map macOS natural text editing shortcuts to readline escape sequences.
 * Returns the escape sequence if matched, null otherwise.
 * This should be called BEFORE keyEventToSequence.
 */
export function naturalTextEditingSequence(e: KeyboardEvent): string | null {
  // Option+Arrow = word movement (ESC+b / ESC+f — standard readline)
  if (e.altKey && !e.ctrlKey && !e.metaKey) {
    if (e.key === 'ArrowLeft') return '\x1bb'    // backward word (ESC+b)
    if (e.key === 'ArrowRight') return '\x1bf'   // forward word (ESC+f)
    // Option+Backspace: send ESC+DEL (0x1b 0x7f) — matches iTerm2's
    // "Natural Text Editing" preset exactly.
    if (e.key === 'Backspace') return '\x1b\x7f'  // backward-kill-word (ESC+DEL)
    // Option+Delete: send ESC+d — standard kill-word-forward
    if (e.key === 'Delete') return '\x1bd'        // kill-word (ESC+d)
  }

  // Cmd+Arrow = line movement
  if (e.metaKey && !e.ctrlKey && !e.altKey) {
    if (e.key === 'ArrowLeft') return '\x01'     // beginning of line (Ctrl+A)
    if (e.key === 'ArrowRight') return '\x05'    // end of line (Ctrl+E)
    if (e.key === 'Backspace') return '\x15'     // delete to beginning (Ctrl+U)
    if (e.key === 'Delete') return '\x0b'        // delete to end (Ctrl+K)
  }

  return null
}
