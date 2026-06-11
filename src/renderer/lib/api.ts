/**
 * Tauri API wrapper — replaces the old tRPC + Electron IPC layer.
 *
 * All backend calls go through `invoke()` from @tauri-apps/api/core.
 * All event subscriptions go through `listen()` from @tauri-apps/api/event.
 */
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'

// Re-export for direct use
export { invoke, listen }
export type { UnlistenFn }
