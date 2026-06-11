// Renderer-side wrappers for the daemon's `/cli/terminal/*` lifecycle
// routes. Phase 2 Unit 3 moved PTY ownership from Tauri to the daemon
// so PTYs survive Tauri quit (K2 Connect + Mobile Companion both
// require this). The renderer's old `invoke('terminal_*')` calls
// translate to `daemonCli{Get,Post}` against these new routes.
//
// Event subscriptions (`listen('terminal:grid:<id>')`, etc.) are
// unchanged — the daemon's `terminal_event_sink` broadcasts events
// over `/events` WS and Tauri's `daemon_events.rs` re-emits them
// through `AppHandle::emit` so the renderer contract stays the same.

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'

// Wire shape mirrors `crates/k2so-core/src/terminal/grid_types.rs::GridUpdate`.
// Kept here only as a type re-export hook; consumers can `import type
// { GridUpdate }` from wherever they already do — terminal-daemon.ts
// doesn't own the shape.

export interface CreateOptions {
  id: string
  cwd: string
  command?: string | null
  args?: string[] | null
  cols?: number
  rows?: number
}

/** POST /cli/terminal/create — spawn a new PTY. Returns the id. */
export async function terminalCreate(opts: CreateOptions): Promise<{ id: string }> {
  return daemonCliPost<{ id: string }>('terminal/create', opts)
}

/** POST /cli/terminal/kill — terminate the PTY. */
export async function terminalKill(id: string): Promise<void> {
  await daemonCliPost('terminal/kill', { id })
}

/** POST /cli/terminal/resize — change PTY dimensions. */
export async function terminalResize(id: string, cols: number, rows: number): Promise<void> {
  await daemonCliPost('terminal/resize', { id, cols, rows })
}

/** POST /cli/terminal/kill-foreground — SIGINT the foreground process. */
export async function terminalKillForeground(id: string): Promise<void> {
  await daemonCliPost('terminal/kill-foreground', { id })
}

/** POST /cli/terminal/scroll — scroll viewport by `delta` lines. */
export async function terminalScroll(id: string, delta: number): Promise<void> {
  await daemonCliPost('terminal/scroll', { id, delta })
}

/** POST /cli/terminal/log — optional renderer→daemon debug logging. */
export async function terminalLog(message: string): Promise<void> {
  await daemonCliPost('terminal/log', { message })
}

/**
 * POST /cli/terminal/lifecycle-write — byte-level write for legacy
 * `TerminalManager` PTY ids. The existing `/cli/terminal/write`
 * (GET) route in `terminal_routes.rs` is the session_map UUID-keyed
 * path; this is the parallel path for arbitrary-string IDs that the
 * renderer uses for `terminal_create`-spawned terminals.
 */
export async function terminalWrite(id: string, data: string): Promise<void> {
  await daemonCliPost('terminal/lifecycle-write', { id, data })
}

/** POST /cli/terminal/set-focus — focus/unfocus the PTY. */
export async function terminalSetFocus(id: string, focused: boolean): Promise<void> {
  await daemonCliPost('terminal/set-focus', { id, focused })
}

/** GET /cli/terminal/active-count?path=<workspace>. */
export async function terminalActiveCountForPath(path: string): Promise<number> {
  const r = await daemonCliGet<{ count: number }>('terminal/active-count', { path })
  return r.count
}

/** GET /cli/terminal/foreground-cmd?id=<id>. */
export async function terminalGetForegroundCommand(id: string): Promise<string | null> {
  const r = await daemonCliGet<{ command: string | null }>('terminal/foreground-cmd', { id })
  return r.command
}

/** GET /cli/terminal/exists?id=<id>. */
export async function terminalExists(id: string): Promise<boolean> {
  const r = await daemonCliGet<{ exists: boolean }>('terminal/exists', { id })
  return r.exists
}

/**
 * GET /cli/terminal/get-grid?id=<id>. Returns the full GridUpdate
 * (typed by the caller — see core's `grid_types::GridUpdate`).
 */
export async function terminalGetGrid<T = unknown>(id: string): Promise<T> {
  return daemonCliGet<T>('terminal/get-grid', { id })
}

/**
 * GET /cli/terminal/list-running. Returns
 * `[{terminalId, cwd, command}, ...]` for every live PTY.
 */
export interface RunningTerminalInfo {
  terminalId: string
  cwd: string
  command: string | null
}
export async function terminalListRunning(): Promise<RunningTerminalInfo[]> {
  return daemonCliGet<RunningTerminalInfo[]>('terminal/list-running')
}
