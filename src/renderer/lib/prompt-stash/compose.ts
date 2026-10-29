// Prompt stash ↔ composer glue (spec-prompt-stash-v1 "Saving"): the Cmd+S
// chord, reading a draft's attachments into bytes before the box clears,
// and the source stamp. The composers (TerminalComposeBar, the Inbox
// dialog, a Thread message's menu) call these; the store keeps the list.

import type { ServerScope } from '@/kessel/server-scope'
import { daemonCliGet } from '@/lib/daemon-cli'
import { canonicalHostKey, LOCAL_HOME_HOST } from '@/lib/host-key'
import { imageMimeFromPath, loadHostBinary } from '@/lib/load-host-binary'
import { loadThreadViewer, threadViewerNow, type ThreadViewer } from '@/lib/thread-viewer'
import { orNull, type StashFileInput, type StashSource } from './record'

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

// ── source.server_id: the server's host key, the same on every device ──
//
// A saved server's scope already carries it (`rosson.k2.dev`, `ip:port`).
// This computer's daemon is `local` here, which means a different box on
// every device, so it goes by its Connect host instead (`GET
// /cli/tunnel/status` `public_url`, learned once per connection). With no
// tunnel, nothing names it the same everywhere: null.

const localHostByConnection = new Map<string, string | null>()

async function localPublicHostKey(scope: ServerScope): Promise<string | null> {
  if (localHostByConnection.has(scope.connectionKey)) return localHostByConnection.get(scope.connectionKey) ?? null
  let key: string | null = null
  try {
    const r = await daemonCliGet<{ public_url?: unknown }>(scope, 'tunnel/status')
    const url = typeof r?.public_url === 'string' ? r.public_url.trim() : ''
    if (url) key = canonicalHostKey(url)
  } catch (err) {
    console.warn('[stash] no Connect host for this computer:', err)
  }
  localHostByConnection.set(scope.connectionKey, key)
  return key
}

/** The server's host key for `source.server_id`, or null when it has no
 *  name that is the same on every device. */
export async function stashServerId(scope: ServerScope): Promise<string | null> {
  if (scope.hostKey !== LOCAL_HOME_HOST) return scope.hostKey
  return localPublicHostKey(scope)
}

/** The login the server reports for this person: a Connect username; for
 *  the owner, the owner's linked username where known, else `owner`. */
export function stashUserFor(viewer: ThreadViewer | null): string | null {
  if (!viewer) return null
  if (viewer.principal === 'owner') return viewer.aliases.find((a) => a.trim())?.trim() || 'owner'
  return orNull(viewer.principal.trim())
}

/** Where a stash is being written: the server, the person, the agent
 *  (field rules after the Companion 4.1.4 review; absent = null). */
export async function stashSourceFor(scope: ServerScope, agent: string | null | undefined, agentName: string | null | undefined): Promise<StashSource> {
  const serverId = await stashServerId(scope)
  let viewer: ThreadViewer | null = threadViewerNow(scope)
  try {
    viewer = (await loadThreadViewer(scope)) ?? viewer
  } catch (err) {
    console.warn('[stash] who am I on this server:', err)
  }
  // `local`'s label is "This computer": on another device that names the
  // wrong box, so it goes by its host key there too.
  const label = scope.hostKey === LOCAL_HOME_HOST ? serverId : scope.label
  return {
    server_id: serverId,
    server_label: orNull(label),
    user: stashUserFor(viewer),
    agent: orNull(agent),
    agent_name: orNull(agentName),
  }
}

/** Tests: forget the learned Connect hosts. */
export function resetStashServerIdsForTests(): void {
  localHostByConnection.clear()
}
