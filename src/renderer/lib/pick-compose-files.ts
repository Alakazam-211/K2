// Compose-bar "+" attach picker. WKWebView has no File.path and ignores
// programmatic click() on display:none <input type=file>, so local
// desktop uses the Tauri OS dialog (same host-only seam as
// projects_pick_folder). Remote hosts use the in-app host file picker.

import { invoke } from '@tauri-apps/api/core'
import { isWebClient } from './is-web'
import { useConnectHostStore } from '@/stores/connect-host'
import { useRemoteFolderPickerStore } from '@/stores/remote-folder-picker'

export type ComposeAttachPlan =
  | { kind: 'native' }
  | { kind: 'web-input' }
  | { kind: 'remote' }

export function composeAttachPlan(input?: {
  activeHost?: string
  web?: boolean
}): ComposeAttachPlan {
  const web = input?.web ?? isWebClient()
  const host = input?.activeHost ?? useConnectHostStore.getState().activeHost
  if (host !== 'local') return { kind: 'remote' }
  if (web) return { kind: 'web-input' }
  return { kind: 'native' }
}

/** Native OS multi-file picker. Cancel → null. Empty selection → []. */
export async function pickLocalComposeFiles(): Promise<string[] | null> {
  return invoke<string[] | null>('pick_local_files')
}

export async function pickRemoteComposeFile(): Promise<string | null> {
  return useRemoteFolderPickerStore.getState().open({
    mode: 'file',
    title: 'Attach a file on the host',
  })
}
