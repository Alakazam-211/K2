// Single host-aware chokepoint for "pick a folder to open as a workspace".
//
// When the app is connected to a REMOTE daemon, the native OS folder
// dialog runs on THIS machine and returns a local path that is
// meaningless on the host. So every "add workspace" entry point must
// branch on the active host:
//
//   - activeHost === 'local' → native OS folder dialog (projects_pick_folder)
//   - remote                 → in-app RemoteFolderPicker over the host's fs
//
// Every caller (sidebar "+", icon rail, the File menu, ⌘O) MUST go through
// this helper rather than calling `projects_pick_folder` directly — that's
// what keeps them all host-aware and prevents the "one entry point still
// shows the local picker on a remote" class of bug.

import { invoke } from '@tauri-apps/api/core'
import { useConnectHostStore } from '@/stores/connect-host'
import { useRemoteFolderPickerStore } from '@/stores/remote-folder-picker'

/**
 * Pick a folder to open as a workspace, on the ACTIVE host. Returns the
 * chosen absolute path (local OR remote, matching the active daemon), or
 * `null` if the user cancelled.
 */
export async function pickWorkspaceFolder(): Promise<string | null> {
  const activeHost = useConnectHostStore.getState().activeHost
  if (activeHost === 'local') {
    return invoke<string | null>('projects_pick_folder')
  }
  return useRemoteFolderPickerStore.getState().open()
}
