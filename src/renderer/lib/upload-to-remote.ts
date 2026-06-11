// K2 Connect remote-files Phase 2 — upload substrate (client half).
//
// Tauri's native drag-drop gives the renderer a LOCAL file path on the
// user's machine but no bytes. When the active daemon is remote (K2
// Connect) it can't read that path — the file is on the client's disk.
// So we read the bytes locally (base64 via the `read_local_file_base64`
// Tauri command), then POST them to the active daemon's
// `/cli/fs/upload-binary` route (host-aware via `daemonCliPost`).
//
// This is the foundation Phase 3 (terminal/drop wiring) builds on; there
// is NO drop wiring here yet.

import { invoke } from '@tauri-apps/api/core'

import { daemonCliPost } from './daemon-cli'

/** Last path segment of a local path, tolerating BOTH `/` and `\`
 *  separators (a Windows client path dropped onto a unix daemon). */
function basename(localPath: string): string {
  const seg = localPath.split(/[/\\]/).pop()
  return seg && seg.length > 0 ? seg : localPath
}

/**
 * Move a local file's bytes onto the active daemon's disk.
 *
 * @param localPath absolute path to a file on the CLIENT machine (what
 *   Tauri drag-drop hands us).
 * @param destDir   destination directory ON THE DAEMON (e.g.
 *   `<workspace>/.k2so/downloads` for the terminal-drop case).
 * @returns the final absolute remote path the daemon wrote (collision
 *   handling may have appended ` (1)`, ` (2)`, …).
 */
export async function uploadToRemote(
  localPath: string,
  destDir: string,
): Promise<string> {
  const base64 = await invoke<string>('read_local_file_base64', {
    path: localPath,
  })
  const res = await daemonCliPost<{ path: string }>('fs/upload-binary', {
    dir: destDir,
    filename: basename(localPath),
    base64,
  })
  return res.path
}
