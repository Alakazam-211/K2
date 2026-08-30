// Composer Phase 1b — pure, dependency-free helpers for the compose bar.
// Kept in their own module (no React, no daemon client, no stores) so the
// keybinding + MsgResponse→status mapping are unit-testable in isolation
// (fail-loud) without dragging in the host-aware fetch client.

// ── Wire shape — mirrors `crates/k2-daemon/src/workspace_msg.rs::MsgResponse`
// (the canonical `{success, target_session_id, attempts, reason, hint}`
// JSON). The send-message route returns `success` / `pty_died` /
// `pty_stalled` from `send_message_to_session`, plus a `worker_join`
// failure shape if the blocking task panics (dispatcher.rs).
export interface MsgResponse {
  success: boolean
  target_session_id: string | null
  attempts: number
  reason: string | null
  hint: string | null
}

// ── Status lane state ────────────────────────────────────────────────
// Drives the small status row beneath the textarea:
//   injecting… → delivered ✓ / pty_died ⚠ / pty_stalled ⚠ (try again) / busy ⚠
export type ComposeStatus =
  | { kind: 'idle' }
  | { kind: 'injecting' }
  | { kind: 'delivered' }
  | { kind: 'pty_died'; hint: string | null }
  | { kind: 'pty_stalled'; hint: string | null }
  | { kind: 'busy'; reason: string | null; hint: string | null }
  | { kind: 'error'; message: string }

/**
 * Map a daemon `MsgResponse` onto the status-lane state. Pure + exported
 * so the mapping is unit-testable without rendering the component.
 *
 * `success` → delivered. Otherwise dispatch on the canonical reason code;
 * anything that is not `pty_died`/`pty_stalled` (e.g. the dispatcher's
 * `worker_join`) collapses to the generic `busy` lane so a new/unknown
 * reason code can never silently render as "delivered".
 */
export function mapMsgResponseToStatus(resp: MsgResponse): ComposeStatus {
  if (resp.success) return { kind: 'delivered' }
  switch (resp.reason) {
    case 'pty_died':
      return { kind: 'pty_died', hint: resp.hint }
    case 'pty_stalled':
      return { kind: 'pty_stalled', hint: resp.hint }
    default:
      return { kind: 'busy', reason: resp.reason, hint: resp.hint }
  }
}

/**
 * Enter = send, Shift+Enter = newline. Pure + exported so the keybinding
 * is unit-testable without a DOM. IME composition (`isComposing`) must
 * NOT submit — a CJK candidate-commit Enter is part of composing the
 * text, not a send.
 */
export function shouldSendOnKey(e: {
  key: string
  shiftKey: boolean
  isComposing: boolean
}): boolean {
  return e.key === 'Enter' && !e.shiftKey && !e.isComposing
}

/**
 * Esc / Ctrl+C / Ctrl+B from the compose box inject the same PTY bytes
 * the terminal would send. Compose stays focused — no focus flip.
 * Cmd+C is copy (not interrupt). Cmd+B is not STX.
 * Empty Enter/Return is a separate PTY CR (`composeEmptyEnterSequence`).
 * Mid-IME composition must not fire.
 */
export function composeInterruptSequence(e: {
  key: string
  ctrlKey: boolean
  metaKey: boolean
  altKey: boolean
  isComposing: boolean
}): string | null {
  if (e.isComposing) return null
  if (e.key === 'Escape' && !e.ctrlKey && !e.metaKey && !e.altKey) return '\x1b'
  if ((e.key === 'c' || e.key === 'C') && e.ctrlKey && !e.metaKey && !e.altKey) {
    return '\x03'
  }
  if ((e.key === 'b' || e.key === 'B') && e.ctrlKey && !e.metaKey && !e.altKey) {
    return '\x02'
  }
  return null
}

/**
 * Empty Enter/Return from compose injects CR into the PTY (confirm a
 * TUI prompt) without sending a message. Shift+Enter stays a newline;
 * a non-empty draft (or a selected slash command) still sends.
 */
export function composeEmptyEnterSequence(e: {
  key: string
  shiftKey: boolean
  isComposing: boolean
  canSend: boolean
}): string | null {
  if (e.canSend) return null
  if (!shouldSendOnKey(e)) return null
  return '\r'
}

/**
 * Composer 1c (D4) + #67 per-workspace — renderer-hide predicate. Mirrors
 * the DAEMON's capability gate (`authorize_send_message` +
 * `remote_instruct_opt_in_for_session`): the composer is permitted iff
 *   • the active host is LOCAL (the owner is always allowed), OR
 *   • the app-level `allowRemoteInstruct` master is ON (global, back-compat), OR
 *   • the ACTIVE WORKSPACE opted into remote instruction
 *     (`perWorkspaceAllow`, default OFF).
 *
 * This is DEFENSE-IN-DEPTH only — the daemon rejects an unauthorized send
 * with a 403 regardless of what the renderer shows, and enforces the
 * decision PER-WORKSPACE server-side. A connect-user's role is never below
 * Member (Member is the floor), so the renderer needs no role input; the
 * daemon enforces the `>= Member` floor and a future sub-Member role slots
 * in there. Pure + exported so the mapping is unit-testable without
 * mounting the component.
 */
export function composerPermitted(_input: {
  isLocalHost: boolean
  allowRemoteInstruct: boolean
  perWorkspaceAllow?: boolean
}): boolean {
  // The composer ALWAYS renders — including on remote hosts. Authorization is
  // enforced SERVER-SIDE: the daemon 403s an unauthorized send regardless of
  // what the renderer shows (the owner is always allowed; a connect-user is
  // gated per-workspace). Hiding the bar on remote just made it vanish, which
  // was confusing — so we keep it visible and let the daemon be the single
  // gate. (Rosson 2026-06-27: leave the composer enabled by default on remote
  // machines.) Input kept for call-site stability + the unit-test contract.
  return true
}

/**
 * Whether TerminalPane should mount the agent compose bar.
 *
 * Mount on the first paint (idle / spawning / connecting / ready / error)
 * so measure-first spawn sees the bar's real height — it is not a fixed
 * px (editor font size changes the row). Hide only on exited: the PTY
 * is gone. Send stays disabled until `sessionId` exists (the bar chrome
 * is still there). Soft-resync keeps the bar mounted (`ready` →
 * `connecting`) so focus/draft are not interrupted.
 */
export function shouldShowTerminalComposeBar(phase: {
  kind: string
  sessionId?: string
}): boolean {
  return phase.kind !== 'exited'
}

/** Raster/vector stills we can thumbnail in the composer (not PDF). */
const COMPOSE_IMAGE_PREVIEW_RE =
  /\.(png|jpe?g|gif|webp|bmp|heic|heif|svg)$/i

export function isComposePreviewImagePath(path: string): boolean {
  return COMPOSE_IMAGE_PREVIEW_RE.test(path.trim())
}

/** Strip compose-drop quoting (`'path with space.png'` or backslash escapes). */
export function unquoteComposePath(token: string): string {
  const t = token.trim()
  if (t.length >= 2 && ((t.startsWith("'") && t.endsWith("'")) || (t.startsWith('"') && t.endsWith('"')))) {
    return t.slice(1, -1)
  }
  return t.replace(/\\(.)/g, '$1')
}

/**
 * Image file paths currently sitting in the compose draft (quoted or bare).
 * Order preserved; duplicates dropped.
 */
export function extractImagePathsFromDraft(draft: string): string[] {
  const found: string[] = []
  const seen = new Set<string>()
  const push = (raw: string) => {
    const p = unquoteComposePath(raw)
    if (!isComposePreviewImagePath(p) || seen.has(p)) return
    seen.add(p)
    found.push(p)
  }
  const quoted = /'([^']+)'/g
  let m: RegExpExecArray | null
  while ((m = quoted.exec(draft)) !== null) push(m[1])
  const unquoted =
    /(?:^|[\s])((?:\/|[A-Za-z]:\\)[^\s']+\.(?:png|jpe?g|gif|webp|bmp|heic|heif|svg))\b/gi
  while ((m = unquoted.exec(draft)) !== null) push(m[1])
  return found
}

/** Drop one attached path (quoted or bare) from the draft. */
export function removePathFromDraft(draft: string, path: string): string {
  const escaped = path.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
  let next = draft.replace(new RegExp(`\\s*'${escaped}'\\s*`, 'g'), ' ')
  next = next.replace(new RegExp(`(?:^|\\s)${escaped}(?=\\s|$)`, 'g'), ' ')
  return next.replace(/[ \t]+/g, ' ').trim()
}

/** Composer `/` picker — only these two go out as a `command` field. */
export const COMPOSE_SLASH_COMMANDS = [
  { command: '/compact', title: 'compact context' },
  { command: '/goal', title: 'set a goal' },
] as const

export type ComposeSlashCommandItem = (typeof COMPOSE_SLASH_COMMANDS)[number]
export type ComposeSlashCommand = ComposeSlashCommandItem['command']

/**
 * Normalize a composer slash-command. Empty/whitespace → null. Optional
 * missing slash, case-insensitive exact match → canonical `/compact` or
 * `/goal`. Unknown (`/exit`, `/loop`, `/compact now`) → null (the daemon
 * 400s those; the picker never offers them).
 */
export function normalizeComposeSlashCommand(
  raw: string | null | undefined,
): ComposeSlashCommand | null {
  const trimmed = (raw ?? '').trim()
  if (!trimmed) return null
  const withSlash = (trimmed.startsWith('/') ? trimmed : `/${trimmed}`).toLowerCase()
  for (const item of COMPOSE_SLASH_COMMANDS) {
    if (item.command === withSlash) return item.command
  }
  return null
}

/**
 * First-token typeahead query. Menu opens only when the draft starts
 * with `/` and the user is still typing that token (no whitespace):
 * `^/[^\s]*$`. Mid-sentence `/tmp/foo` after other text, a leading
 * space (` /c`), or `/c more` must not open the picker.
 */
export function composeSlashTypeaheadQuery(draft: string): string | null {
  if (!/^\/[^\s]*$/.test(draft)) return null
  return draft
}

/**
 * Case-insensitive prefix filter on the command string. `'/'` and
 * empty → both commands. `'/c'` / `'comp'` → `/compact`. Unknown
 * prefixes (`'/x'`, `'/exit'`) → no matches.
 */
export function filterComposeSlashCommands(
  query: string,
): readonly ComposeSlashCommandItem[] {
  const q = query.trim().toLowerCase()
  if (!q || q === '/') return COMPOSE_SLASH_COMMANDS
  const needle = q.startsWith('/') ? q : `/${q}`
  return COMPOSE_SLASH_COMMANDS.filter((item) => item.command.startsWith(needle))
}

/** True when the draft is a typeahead query with at least one match. */
export function composeSlashMenuOpenFromDraft(draft: string): boolean {
  const query = composeSlashTypeaheadQuery(draft)
  if (query == null) return false
  return filterComposeSlashCommands(query).length > 0
}

/**
 * Strip the leading first-token slash word and any following
 * whitespace. Leaves a non-slash draft untouched.
 */
export function consumeComposeSlashToken(draft: string): string {
  return draft.replace(/^\/[^\s]*/, '').replace(/^\s+/, '')
}

/**
 * Exact unique command for the space-commit path. `/compact` / `/goal`
 * (optional trailing rest is ignored) → canonical command. Prefixes
 * (`/c`) and unknowns (`/exit`) → null.
 */
export function composeSlashExactCommand(query: string): ComposeSlashCommand | null {
  const token = query.match(/^\/[^\s]*/)?.[0]
  if (!token || token === '/') return null
  return normalizeComposeSlashCommand(token)
}

/**
 * Space after an exact unique command (/compact or /goal plus a
 * trailing space and optional remainder). A non-exact prefix plus
 * space (e.g. /c) does not commit.
 */
export function composeSlashSpaceCommit(draft: string): {
  command: ComposeSlashCommand
  remainder: string
} | null {
  if (!/^\/[^\s]+ /.test(draft)) return null
  const command = composeSlashExactCommand(draft)
  if (!command) return null
  return { command, remainder: consumeComposeSlashToken(draft) }
}

export type ComposeSlashMenuKeyResult =
  | { kind: 'close' }
  | { kind: 'move'; highlight: number }
  | { kind: 'select' }

/**
 * Menu-open key path. Must run before send (Enter) and before
 * compose send-history (ArrowUp/ArrowDown). No wrap; 0 matches does
 * not steal Enter.
 */
export function composeSlashMenuKeyAction(input: {
  menuOpen: boolean
  matchCount: number
  highlight: number
  key: string
  shiftKey?: boolean
  isComposing?: boolean
}): ComposeSlashMenuKeyResult | null {
  if (!input.menuOpen) return null
  if (input.key === 'Escape') return { kind: 'close' }
  if (input.matchCount < 1) return null
  if (input.key === 'ArrowDown') {
    return {
      kind: 'move',
      highlight: Math.min(input.highlight + 1, input.matchCount - 1),
    }
  }
  if (input.key === 'ArrowUp') {
    return {
      kind: 'move',
      highlight: Math.max(input.highlight - 1, 0),
    }
  }
  if (input.key === 'Enter' && !input.shiftKey && !input.isComposing) {
    return { kind: 'select' }
  }
  return null
}

/** Keyboard highlight when the menu opens: the already-selected command, else 0. */
export function composeSlashInitialHighlight(
  matches: readonly { command: string }[],
  selected: string | null | undefined,
): number {
  if (matches.length === 0) return 0
  if (!selected) return 0
  const i = matches.findIndex((m) => m.command === selected)
  return i >= 0 ? i : 0
}

/** Empty draft + selected slash command + Backspace/Delete → clear the command. */
export function composeSlashBackspaceClearsCommand(input: {
  draft: string
  command: string | null | undefined
  key: string
  isComposing?: boolean
}): boolean {
  if (input.isComposing) return false
  if (input.key !== 'Backspace' && input.key !== 'Delete') return false
  if (input.draft.length > 0) return false
  return normalizeComposeSlashCommand(input.command) != null
}

/** Send is allowed when the draft is non-empty OR a command is selected. */
export function composeCanSend(input: {
  draft: string
  sending: boolean
  command?: string | null
}): boolean {
  if (input.sending) return false
  if (input.draft.trim().length > 0) return true
  return normalizeComposeSlashCommand(input.command) != null
}

/** Placeholder for the docked compose bar. Uses the workspace agent name
 *  when we have one; otherwise the generic "the agent". */
export function composeMessagePlaceholder(agentName: string | undefined | null): string {
  const name = (agentName ?? '').trim()
  return name ? `Message ${name}` : 'Message the agent'
}

/** Resolve the Agents-list name for this pane's workspace path. */
export function composeAgentNameFromProjects(
  projects: Array<{
    path: string
    name: string
    workspaces?: Array<{ worktreePath: string | null }>
  }>,
  workspacePath: string,
): string {
  if (!workspacePath) return ''
  const exact = projects.find((p) => p.path === workspacePath)
  if (exact?.name?.trim()) return exact.name.trim()
  for (const p of projects) {
    if (p.workspaces?.some((w) => w.worktreePath === workspacePath) && p.name?.trim()) {
      return p.name.trim()
    }
  }
  return ''
}

/** Wire shape of `GET /cli/terminal/compose-history`. */
export interface ComposeHistoryItem {
  id: string
  body: string
  author: string
  created_at: number
}

/**
 * Compose-bar send-history key. ArrowUp recalls only when the caret is
 * collapsed at offset 0. Mid-draft Up returns null so the caret can
 * move. ArrowDown always reports `newer`; the caller no-ops at draft
 * index `-1` (does not preventDefault).
 */
/** localStorage key for the compose-bar caret (per PTY session). */
export function composeCaretStorageKey(sessionId: string): string {
  return `k2:composer:caret:${sessionId}`
}

export function clampComposeCaret(offset: number, textLen: number): number {
  if (!Number.isFinite(offset)) return textLen
  return Math.max(0, Math.min(textLen, Math.floor(offset)))
}

/** Missing/invalid caret → end of text (keep typing). Always clamped. */
export function readComposeCaret(
  sessionId: string,
  textLen: number,
): { start: number; end: number } {
  const fallback = { start: textLen, end: textLen }
  if (!sessionId) return fallback
  try {
    const raw = localStorage.getItem(composeCaretStorageKey(sessionId))
    if (!raw) return fallback
    const parsed = JSON.parse(raw) as { start?: unknown; end?: unknown }
    const start = clampComposeCaret(Number(parsed.start), textLen)
    const end = clampComposeCaret(Number(parsed.end), textLen)
    return { start, end: Math.max(start, end) }
  } catch {
    return fallback
  }
}

export function writeComposeCaret(
  sessionId: string,
  start: number,
  end: number,
  textLen: number,
): void {
  if (!sessionId) return
  try {
    const s = clampComposeCaret(start, textLen)
    const e = clampComposeCaret(end, textLen)
    localStorage.setItem(
      composeCaretStorageKey(sessionId),
      JSON.stringify({ start: s, end: Math.max(s, e) }),
    )
  } catch {
    /* storage disabled */
  }
}

export function clearComposeCaret(sessionId: string): void {
  if (!sessionId) return
  try {
    localStorage.removeItem(composeCaretStorageKey(sessionId))
  } catch {
    /* storage disabled */
  }
}

export function composeHistoryKeyAction(input: {
  key: string
  selectionStart: number | null
  selectionEnd: number | null
}): 'older' | 'newer' | null {
  if (input.key === 'ArrowUp') {
    if (input.selectionStart === 0 && input.selectionEnd === 0) return 'older'
    return null
  }
  if (input.key === 'ArrowDown') return 'newer'
  return null
}

/**
 * Walk compose send history. `index` `-1` is the pre-Up draft;
 * `0` is newest. `items` is newest-first. `draft` is the stashed
 * pre-Up text (or empty) restored when walking back to `-1`.
 */
export function applyComposeHistoryNav(input: {
  action: 'older' | 'newer'
  index: number
  draft: string
  items: readonly string[]
}): { index: number; text: string; preventDefault: boolean } {
  const { action, index, draft, items } = input
  if (items.length === 0) {
    return { index: -1, text: draft, preventDefault: false }
  }
  if (action === 'older') {
    const next = index < 0 ? 0 : Math.min(index + 1, items.length - 1)
    return { index: next, text: items[next] ?? draft, preventDefault: true }
  }
  if (index < 0) {
    return { index: -1, text: draft, preventDefault: false }
  }
  if (index === 0) {
    return { index: -1, text: draft, preventDefault: true }
  }
  const next = index - 1
  return { index: next, text: items[next] ?? draft, preventDefault: true }
}

/** Cap before the compose textarea scrolls internally. */
export const COMPOSE_TEXTAREA_MAX_HEIGHT = 160

/**
 * Pixel height for the compose textarea. Empty draft stays one line —
 * never let the placeholder wrap drive `scrollHeight` (cold start /
 * hidden tabs / narrow first layout inflate to the max cap).
 */
export function composeTextareaHeight(opts: {
  value: string
  scrollHeight: number
  fontSize: number
  maxHeight?: number
}): number {
  const font = opts.fontSize > 0 ? opts.fontSize : 12
  const singleLine = Math.round(font * 1.4 + 8)
  const cap = opts.maxHeight ?? COMPOSE_TEXTAREA_MAX_HEIGHT
  if (!opts.value) return singleLine
  return Math.min(Math.max(opts.scrollHeight, singleLine), cap)
}
