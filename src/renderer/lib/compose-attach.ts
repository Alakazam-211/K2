// The "message the agent" attach path, shared by the Agents page compose bar
// and Zen's compose (prd-zen-mode-v1, Rosson's 2026-10-04 answer 6: Zen
// reuses the existing image and file upload, local and cross-server).
//
// Files picked or dropped on THIS computer become text in the message: the
// paths the agent can open.
//   - The agent's server is this computer: the local paths, as they are.
//   - Another server: each file is uploaded into that workspace's
//     `.k2/downloads/` on the agent's own server first (the terminal-drop
//     destination, `executeRemoteDrop`), and that server's paths are used.
//   - Browser `File`s (no path, hosted web or an HTML5 drop): always
//     uploaded the same way (`executeBrowserFileDrop`).
// Returns the draft text to insert (shell-escaped, space-joined, trailing
// space), or null when nothing was attached (cancelled, or an upload failed:
// the drop path already toasted why).

import type { ServerScope } from '@/kessel/server-scope'
import { buildComposeDropPayload } from '@/lib/external-drop-router'
import { executeBrowserFileDrop, executeRemoteDrop } from '@/lib/handle-remote-drop'

export interface ComposeAttachInput {
  /** Paths on THIS computer (the native picker, a Tauri drop). */
  paths?: string[]
  /** Browser files with no path. */
  files?: File[]
  /** The agent's workspace on its server: uploads land in its `.k2/downloads`. */
  workspacePath?: string
}

export async function composeAttachPayload(scope: ServerScope, input: ComposeAttachInput): Promise<string | null> {
  const ctx = { workspacePath: input.workspacePath || undefined }
  if (input.paths && input.paths.length > 0) {
    if (!scope.isRemote) return buildComposeDropPayload(input.paths)
    return executeRemoteDrop(scope, input.paths, { kind: 'terminal' }, ctx, buildComposeDropPayload)
  }
  if (input.files && input.files.length > 0) {
    return executeBrowserFileDrop(scope, input.files, { kind: 'terminal' }, ctx, buildComposeDropPayload)
  }
  return null
}
