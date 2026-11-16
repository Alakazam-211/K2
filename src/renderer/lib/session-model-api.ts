// One chat's model (0.46.13): what the Thread's `/model` picker reads and
// sends. The daemon owns the switch (`POST /cli/sessions/model`): it types
// `/model <id>` into that session and puts the CLI's saved default back.

import { daemonCliGet, daemonCliPost } from './daemon-cli'
import type { ServerScope } from '@/kessel/server-scope'

/** `GET /cli/sessions/model/options`. */
export interface SessionModelOptions {
  sessionId: string
  /** The CLI binary the session runs (`claude`, `codex`, …). */
  harness: string
  /** Whether K2 can switch it live (Claude today). */
  switchable: boolean
  /** The model it's on when K2 knows (its last switch, or its argv). */
  current: string | null
  serverDefault: string | null
  workspaceDefault: string | null
}

/** `POST /cli/sessions/model`'s answer. */
export interface SessionModelSwitch {
  ok: true
  sessionId: string
  model: string
  default: { outcome: string; default?: string | null; error?: string }
  note: string
  noted: boolean
}

export function fetchSessionModelOptions(scope: ServerScope, sessionId: string): Promise<SessionModelOptions> {
  return daemonCliGet<SessionModelOptions>(scope, 'sessions/model/options', { session_id: sessionId })
}

/** Switch this chat only. `thread`: the Thread the note goes on. */
export function switchSessionModel(
  scope: ServerScope,
  sessionId: string,
  model: string,
  thread?: string,
): Promise<SessionModelSwitch> {
  return daemonCliPost<SessionModelSwitch>(scope, 'sessions/model', {
    session_id: sessionId,
    model,
    ...(thread ? { thread } : {}),
  })
}
