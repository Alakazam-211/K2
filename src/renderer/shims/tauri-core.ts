/**
 * Web shim for `@tauri-apps/api/core`.
 * Known no-ops resolve; secret / host-list commands use localStorage stubs;
 * unknown commands warn and resolve null softly.
 */

import { webSecretStorageKey } from '../web/session-token'

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

const CONNECT_HOSTS_LS_KEY = 'k2so.connect-hosts.v1'

function storageGet(key: string): string | null {
  try {
    if (typeof localStorage === 'undefined') return null
    return localStorage.getItem(key)
  } catch {
    return null
  }
}

function storageSet(key: string, value: string): void {
  try {
    if (typeof localStorage === 'undefined') return
    localStorage.setItem(key, value)
  } catch {
    /* quota / private mode */
  }
}

function storageRemove(key: string): void {
  try {
    if (typeof localStorage === 'undefined') return
    localStorage.removeItem(key)
  } catch {
    /* ignore */
  }
}

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
      // Web never uses the local daemon path (boot-host forces remote).
      return { state: 'not_installed', reason: 'web client' }
    case 'cli_install_status':
      return {
        installed: false,
        installedVersion: null,
        bundledVersion: null,
        updateAvailable: false,
      }
    case 'connect_hosts_read':
      // Prefer the same localStorage key the connect-host store uses so a
      // non-web-aware hydrate path (if re-enabled) sees the SPA book.
      return storageGet(CONNECT_HOSTS_LS_KEY) ?? '[]'
    default:
      return null
  }
}

export async function invoke<T = unknown>(
  cmd: string,
  args?: Record<string, unknown>,
): Promise<T> {
  // Web storage stubs for keychain + durable host list (Stage B token auth).
  if (cmd === 'k2_secret_set') {
    const service = String(args?.service ?? '')
    const account = String(args?.account ?? '')
    const secret = String(args?.secret ?? args?.value ?? '')
    if (service && account) {
      storageSet(webSecretStorageKey(service, account), secret)
    }
    return undefined as T
  }
  if (cmd === 'k2_secret_get') {
    const service = String(args?.service ?? '')
    const account = String(args?.account ?? '')
    if (!service || !account) return null as T
    return (storageGet(webSecretStorageKey(service, account)) ?? null) as T
  }
  if (cmd === 'k2_secret_delete') {
    const service = String(args?.service ?? '')
    const account = String(args?.account ?? '')
    if (service && account) {
      storageRemove(webSecretStorageKey(service, account))
    }
    return undefined as T
  }
  if (cmd === 'connect_hosts_write') {
    const json = String(args?.json ?? '[]')
    storageSet(CONNECT_HOSTS_LS_KEY, json)
    return undefined as T
  }
  if (cmd === 'connect_hosts_read') {
    return defaultFor(cmd) as T
  }

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
