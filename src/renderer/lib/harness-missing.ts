// Plain copy for a chat that can't start because the workspace's harness
// CLI isn't installed on the server it runs on (0.46.10).
//
// The daemon's spawn error is accurate but reads like a stack trace:
//   "spawn failed: v2 spawn failed: ptyd spawn: enoent: codex: not found on the child PATH"
// (k2-ptyd), or portable-pty's
//   "Unable to spawn codex because it doesn't exist on the filesystem and was not found in PATH".
// The chat pane shows this line instead, naming the program and the server.

import { LOCAL_SCOPE_LABEL } from '@/kessel/server-scope'

const PTYD_ENOENT = /enoent:\s*([^\s:]+):\s*not found on the child PATH/i
const PORTABLE_PTY_ENOENT = /Unable to spawn\s+(\S+)\s+because it doesn't exist on the filesystem/i

/** The missing program's name, or null when `raw` isn't a not-installed error. */
export function missingHarnessProgram(raw: string): string | null {
  const m = PTYD_ENOENT.exec(raw) ?? PORTABLE_PTY_ENOENT.exec(raw)
  if (!m) return null
  // A full path names the program by its last segment.
  const prog = m[1].split(/[\\/]/).pop() ?? m[1]
  return prog === '' ? null : prog
}

/** The daemon refused to start a pinned chat it can't find on disk. */
export function isMainChatMissing(raw: string): boolean {
  return raw.includes('main_chat_missing')
}

/** What to tell the person when the pinned chat's history is gone. */
export const MAIN_CHAT_MISSING_MESSAGE =
  "K2 kept this chat pinned and started nothing, rather than open a different one. " +
  'Choose a chat from the menu above, or run k2 sessions main set <id> (k2 sessions --past lists them).'

/** "codex isn't installed on LZTEK. Pick another harness …", or null. */
export function harnessMissingMessage(raw: string, serverLabel: string): string | null {
  const prog = missingHarnessProgram(raw)
  if (!prog) return null
  const where = serverLabel === LOCAL_SCOPE_LABEL ? 'this computer' : serverLabel
  return `${prog} isn't installed on ${where}. Pick another harness for this workspace, or install ${prog} there and retry.`
}
