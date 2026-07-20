/**
 * Web shim for `@tauri-apps/api/core`.
 * Known no-ops resolve; unknown commands warn and resolve null softly.
 */

const KNOWN_NOOPS = new Set([
  'renderer_heartbeat',
  'plugin:event|unlisten',
  'plugin:event|emit',
  'plugin:event|listen',
  'plugin:resources|close',
  'plugin:app|name',
  'plugin:app|version',
  'relaunch_via_open',
  'restart_app',
])

/** Soft defaults for boot-path / parity-style invokes that must not throw. */
function defaultFor(cmd: string): unknown {
  switch (cmd) {
    case 'plugin:app|name':
      return 'K2'
    case 'plugin:app|version':
    case 'get_current_version':
      return (
        (import.meta as ImportMeta & { env?: Record<string, string> }).env
          ?.VITE_APP_VERSION ?? '0.0.0-web'
      )
    case 'plugin:event|listen':
      return 1
    case 'daemon_status':
    case 'daemon_ws_url':
      return { state: 'not_installed', reason: 'web client' }
    case 'cli_install_status':
      return {
        installed: false,
        installedVersion: null,
        bundledVersion: null,
        updateAvailable: false,
      }
    case 'connect_hosts_read':
      return '[]'
    default:
      return null
  }
}

export async function invoke<T = unknown>(
  cmd: string,
  _args?: Record<string, unknown>,
): Promise<T> {
  if (KNOWN_NOOPS.has(cmd) || cmd.startsWith('plugin:')) {
    return defaultFor(cmd) as T
  }
  // Soft resolve: log once-style warn so desktop-only paths don't crash the SPA.
  console.warn(`[web-shim] invoke('${cmd}') — no backend; resolving null`)
  return defaultFor(cmd) as T
}

/** Identity: browser has no asset protocol mapping. */
export function convertFileSrc(filePath: string, _protocol?: string): string {
  return filePath || ''
}

export function transformCallback<T = unknown>(
  callback: (response: T) => void,
  _once = false,
): number {
  // Callbacks are unused in the web build; return a dummy id.
  void callback
  return 0
}

export type { }
