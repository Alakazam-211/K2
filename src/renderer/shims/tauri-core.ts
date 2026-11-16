/**
 * Web shim for `@tauri-apps/api/core`.
 * Known no-ops resolve; the host list uses localStorage (it holds no
 * secrets); unknown commands warn and resolve null softly.
 *
 * Secrets: the browser has no keychain, and no long-lived secret may sit
 * in persistent browser storage. `k2_secret_*` keeps a Connect session
 * token in memory for this page only (the store mirrors the live one into
 * sessionStorage for a reload of the same tab); passwords, the account
 * session and any other secret are dropped. Desktop never loads this file.
 */

import {
  readConnectHostsStorage,
  writeConnectHostsStorage,
} from '../lib/connect-hosts-storage'
import { WEB_SECRET_STORAGE_PREFIX } from '../web/session-token'

/**
 * The one keychain service the web build keeps (in memory): Connect
 * session tokens. Equal to `K2_CONNECT_KEYCHAIN_SERVICE` in
 * stores/connect-host.ts (not imported: that store imports this shim).
 */
export const WEB_MEMORY_SECRET_SERVICE = 'dev.k2.connect.host-token'

/** Page-lifetime secrets, keyed `<service>:<account>`. */
const memorySecrets = new Map<string, string>()

/** Test-only: forget every in-memory secret (a page reload). */
export function __clearWebMemorySecretsForTests(): void {
  memorySecrets.clear()
}

const KNOWN_NOOPS = new Set([
  'renderer_heartbeat',
  'plugin:event|unlisten',
  'plugin:event|emit',
  'plugin:event|listen',
  'plugin:resources|close',
  'plugin:app|name',
  'plugin:app|version',
  'relaunch_via_open',
  'stop_bundled_daemon_for_update',
  'restart_app',
  'open_app_devtools',
  // No in-browser app updates: null = no update.
  'app_update_check',
])

function storageRemove(key: string): void {
  try {
    if (typeof localStorage === 'undefined') return
    localStorage.removeItem(key)
  } catch {
    /* ignore */
  }
}

/** Dual-read connect-hosts JSON from localStorage (`k2.*` → `k2so.*`). */
function storageGetConnectHosts(): string | null {
  try {
    if (typeof localStorage === 'undefined') return null
    return readConnectHostsStorage(localStorage)
  } catch {
    return null
  }
}

/** Write canonical `k2.connect-hosts.v1` and drop legacy `k2so.*`. */
function storageSetConnectHosts(json: string): void {
  try {
    if (typeof localStorage === 'undefined') return
    writeConnectHostsStorage(localStorage, json)
  } catch {
    /* quota / private mode */
  }
}

/**
 * Soft defaults for invokes that must not throw. Prefer empty arrays /
 * zeros over `null` so call sites that do `result.length` after
 * `setState(await invoke(...))` do not crash the SPA (AppErrorBoundary).
 */
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
      // Dual-read: k2.connect-hosts.v1 → legacy k2so.connect-hosts.v1.
      return storageGetConnectHosts() ?? '[]'
    // ── list-shaped desktop commands → empty array ─────────────────
    case 'projects_get_editors':
    case 'projects_get_all_editors':
    case 'k2_agents_list':
    case 'k2_agents_preview_schedule':
    case 'k2_agents_teardown_workspace':
    case 'k2_heartbeat_list_all':
    case 'k2_heartbeat_fires_list_all':
    case 'fs_search_tree':
    case 'fs_list_dir':
    case 'fs_list':
      return []
    // ── count-shaped ───────────────────────────────────────────────
    case 'k2_inbox_count':
    case 'renderer_memory_status':
      return 0
    // ── object-shaped ──────────────────────────────────────────────
    case 'k2_agents_get_editor_context':
    case 'k2_agents_build_launch':
    case 'permissions_get_status':
      return {}
    case 'fs_watch_dir':
    case 'fs_unwatch_dir':
    case 'k2_agents_update_heartbeat_projects':
    case 'k2_agents_install_heartbeat':
      return undefined
    default:
      // Prefer [] over null when the command name looks list-like.
      if (
        cmd.includes('_list') ||
        cmd.endsWith('_list') ||
        cmd.includes('get_editors') ||
        cmd.includes('search')
      ) {
        return []
      }
      return null
  }
}

export async function invoke<T = unknown>(
  cmd: string,
  args?: Record<string, unknown>,
): Promise<T> {
  // Keychain stand-in: a session token in memory only; nothing else kept.
  if (cmd === 'k2_secret_set') {
    const service = String(args?.service ?? '')
    const account = String(args?.account ?? '')
    const secret = String(args?.secret ?? args?.value ?? '')
    if (service === WEB_MEMORY_SECRET_SERVICE && account && secret) {
      memorySecrets.set(`${service}:${account}`, secret)
    }
    return undefined as T
  }
  if (cmd === 'k2_secret_get') {
    const service = String(args?.service ?? '')
    const account = String(args?.account ?? '')
    if (service !== WEB_MEMORY_SECRET_SERVICE || !account) return null as T
    return (memorySecrets.get(`${service}:${account}`) ?? null) as T
  }
  if (cmd === 'k2_secret_delete') {
    const service = String(args?.service ?? '')
    const account = String(args?.account ?? '')
    if (service && account) {
      memorySecrets.delete(`${service}:${account}`)
      // Belt: a copy an older build stored (boot also sweeps them all).
      storageRemove(`${WEB_SECRET_STORAGE_PREFIX}${service}:${account}`)
    }
    return undefined as T
  }
  // The k2.dev account session (a refresh token) is never kept on the web.
  if (cmd === 'k2_account_session_get') return null as T
  if (cmd === 'k2_account_session_set' || cmd === 'k2_account_session_clear') {
    return undefined as T
  }
  if (cmd === 'connect_hosts_write') {
    const json = String(args?.json ?? '[]')
    storageSetConnectHosts(json)
    return undefined as T
  }
  if (cmd === 'connect_hosts_read') {
    return defaultFor(cmd) as T
  }
  // Desktop mirrors Connect tokens into ~/.k2/connect-tokens.json for the
  // bash CLI. Web has no CLI tray path — no-op so hydrate/login don't warn.
  if (cmd === 'connect_cli_token_upsert' || cmd === 'connect_cli_token_delete') {
    return undefined as T
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

/** Tauri's IPC Channel (video sinks import it). The web build has no Rust side,
 *  so nothing ever sends on one: `stores/video.ts` loads no engine on the web. */
export class Channel<T = unknown> {
  id = 0
  onmessage: (response: T) => void
  constructor(onmessage?: (response: T) => void) {
    this.onmessage = onmessage ?? (() => undefined)
  }
  toJSON(): string {
    return `__CHANNEL__:${this.id}`
  }
}

export type { }
