// Move a workspace's pinned chat to another harness (0.46.6, `k2 agent
// harness`). The daemon owns the move (`POST /cli/agent/harness`): it ends
// the pinned chat, starts the new harness with a handover summary, repoints
// heartbeats, Threads and messages, and keeps the old chat in Chat history.
// The renderer only asks and shows the answer.

import { daemonCliPost } from '@/lib/daemon-cli'
import type { ServerScope } from '@/kessel/server-scope'

/** The harnesses a pinned chat can move to (the daemon's list). */
export const PINNED_HARNESSES = ['claude', 'codex', 'grok', 'gemini', 'cursor', 'pi', 'hermes'] as const

const COMMAND_TO_HARNESS: Record<string, string> = {
  claude: 'claude',
  codex: 'codex',
  grok: 'grok',
  gemini: 'gemini',
  'cursor-agent': 'cursor',
  cursor: 'cursor',
  pi: 'pi',
  hermes: 'hermes',
}

const LABELS: Record<string, string> = {
  claude: 'Claude',
  codex: 'Codex',
  grok: 'Grok',
  gemini: 'Gemini',
  cursor: 'Cursor',
  pi: 'Pi',
  hermes: 'Hermes',
}

/** The harness a preset command runs (`grok --always-approve` → `grok`),
 *  or null for a custom program the pinned chat can't move to. */
export function harnessOfCommand(command: string | null | undefined): string | null {
  const first = (command ?? '').trim().split(/\s+/)[0] ?? ''
  const base = (first.split(/[/\\]/).pop() ?? first).replace(/\.exe$/i, '').toLowerCase()
  return COMMAND_TO_HARNESS[base] ?? null
}

/** Display name for a harness key. */
export function harnessLabel(harness: string): string {
  return LABELS[harness] ?? harness
}

export interface MovePinnedChatResult {
  switched: boolean
  alreadyOn?: boolean
  harness: string
  fromHarness?: string
  workspace?: string
  sessionId?: string
  oldChatInHistory?: boolean | null
  ownSessionHeartbeats?: string[]
}

/** Ask the daemon to move `workspace`'s pinned chat (handle, path or id). */
export function movePinnedChat(scope: ServerScope, workspace: string, harness: string): Promise<MovePinnedChatResult> {
  return daemonCliPost<MovePinnedChatResult>(scope, 'agent/harness', { workspace, harness })
}

/** One line for the person after a move. */
export function movePinnedChatMessage(r: MovePinnedChatResult): string {
  const to = harnessLabel(r.harness)
  if (r.alreadyOn) return `The pinned chat is already on ${to}.`
  const own = r.ownSessionHeartbeats ?? []
  const tail = own.length > 0
    ? ` Heartbeats with their own session stay where they are: ${own.join(', ')}.`
    : ''
  return `The pinned chat now runs on ${to}; heartbeats, Threads and messages go there. The old chat is in Chat history.${tail}`
}
