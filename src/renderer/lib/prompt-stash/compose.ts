// Prompt stash ↔ composer glue (spec-prompt-stash-v1 "Saving"): the Cmd+S
// chord, reading a draft's attachments into bytes before the box clears,
// and the source stamp. The composers (TerminalComposeBar, the Inbox
// dialog, a Thread message's menu) call these; the store keeps the list.

import type { ServerScope } from '@/kessel/server-scope'
import { imageMimeFromPath, loadHostBinary } from '@/lib/load-host-binary'
import { threadViewerNow } from '@/lib/thread-viewer'
import type { StashFileInput, StashSource } from './record'

/** Cmd+S on macOS, Ctrl+S elsewhere; no Shift or Alt. No K2 binding uses
 *  this chord (shared/hotkeys, the native menu); the file editor's own
 *  Cmd+S only listens inside the editor. */
export function isStashChord(
  e: { key: string; metaKey: boolean; ctrlKey: boolean; shiftKey: boolean; altKey: boolean },
  isMac: boolean,
): boolean {
  if (e.shiftKey || e.altKey) return false
  if (e.key.toLowerCase() !== 's') return false
  return isMac ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey
}

/** The chord as shown in titles. */
export function stashChordLabel(isMac: boolean): string {
  return isMac ? '⌘S' : 'Ctrl+S'
}

function baseName(path: string): string {
  return path.split('/').pop()?.split('\\').pop() || path
}

/** A draft's attached files (paths on the composer's server) as bytes for
 *  the stash. Throws when any can't be read: the draft must then stay in
 *  the box, never half-saved. */
export async function readComposeFiles(scope: ServerScope, paths: readonly string[]): Promise<StashFileInput[]> {
  const out: StashFileInput[] = []
  for (const path of paths) {
    const bytes = await loadHostBinary(scope, path)
    const mime = imageMimeFromPath(path)
    const type = mime === 'application/octet-stream' ? '' : mime
    out.push({ name: baseName(path), type, bytes: new Blob([bytes as BlobPart], type ? { type } : undefined) })
  }
  return out
}

/** Where a stash is being written: the server, the person, the agent. */
export function stashSourceFor(scope: ServerScope, agent: string, agentName: string): StashSource {
  return {
    server_id: scope.hostKey,
    server_label: scope.label,
    user: threadViewerNow(scope)?.principal ?? '',
    agent,
    agent_name: agentName,
  }
}
