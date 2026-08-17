// Mock-daemon plumbing for the parity harness.
//
// The renderer normally needs BOTH a Tauri backend (window.__TAURI_INTERNALS__)
// and a k2 daemon (HTTP /cli/* + /boot-status + WS). Neither exists in a plain
// Playwright browser context, and depending on the user's production daemon
// would make the diff nondeterministic (and impossible in CI). Strategy:
//
//   1. A minimal __TAURI_INTERNALS__ shim (addInitScript) so @tauri-apps/api
//      module init + event listens don't crash. `daemon_ws_url` reports
//      not_installed, so the LOCAL daemon path is dead by construction.
//   2. Playwright route interception on a fake REMOTE host
//      (http://127.0.0.1:45999 — nothing listens there; routes are fulfilled
//      before the network). Canned, timestamp-free responses.
//   3. The app is steered onto that remote host programmatically: Vite dev
//      serves the renderer's modules by URL, so `import('/stores/…')` from
//      page.evaluate returns the LIVE zustand store singletons the app uses.
//      We add a ConnectHost (with an in-memory token) and selectHost() it —
//      the ConnectionGate's remote policy (protocol range-check, whoami
//      probe) accepts against the canned responses and mounts the full App.
import type { Page } from '@playwright/test'

/** Fake remote-daemon address. Nothing must listen here — every request is
 *  fulfilled by route interception. */
export const MOCK_HOST = '127.0.0.1'
export const MOCK_PORT = 45999

/** CSS injected before every capture: kills animations, transitions and the
 *  text caret so a mid-keyframe raster can never produce a phantom diff. */
export const FREEZE_CSS = `
*, *::before, *::after {
  animation: none !important;
  transition: none !important;
  caret-color: transparent !important;
}
`

/**
 * Canned response for a given /cli path. Shapes matter only where a caller
 * would crash or render differently on a wrong shape:
 *   - /boot-status: remote acceptance policy needs protocol>=1 + phase ready.
 *   - whoami: the gate's first-connect session probe needs a 2xx.
 *   - list-ish routes: callers .map() immediately — must be arrays.
 * Everything else gets `{}` — stores treat missing fields as defaults, which
 * is exactly the deterministic "fresh server" surface we want. IMPORTANT:
 * nothing here may carry a timestamp or anything else run-dependent.
 */
/** A fresh daemon's `/cli/settings/get` snapshot (mirrors Rust
 *  `AppSettings::default()` in crates/k2-core/src/app_settings.rs). The
 *  settings store hydrates field-by-field WITHOUT fallbacks for `terminal`
 *  and `keybindings`, so this must be a complete object — `{}` crashes
 *  Settings → Terminal on `terminal.fontFamily`. */
export const DEFAULT_SETTINGS = {
  terminal: {
    fontFamily: 'MesloLGM Nerd Font',
    fontSize: 13,
    cursorStyle: 'bar',
    scrollback: 5000,
    naturalTextEditing: true,
  },
  keybindings: {},
  projectSettings: {},
  focusGroupsEnabled: false,
  activeFocusGroupId: null,
  sidebarCollapsed: false,
  leftPanelOpen: true,
  rightPanelOpen: true,
  leftPanelActiveTab: 'files',
  rightPanelActiveTab: 'chat',
  leftPanelTabs: ['files'],
  rightPanelTabs: ['chat'],
  defaultAgent: 'claude',
  aiAssistantEnabled: false,
  timer: { visible: true, skipMemo: false, timezone: '' },
  agenticSystemsEnabled: true,
  keepDaemonOnQuit: true,
  claudeAuthAutoRefresh: false,
  lastActiveProjectId: null,
  lastActiveWorkspaceId: null,
  editor: {
    tabSize: 2,
    wordWrap: false,
    showWhitespace: false,
    fontSize: 12,
    indentGuides: true,
    foldGutter: true,
    autocomplete: true,
    bracketMatching: true,
    lineNumbers: true,
    highlightActiveLine: true,
    stickyScroll: false,
    minimap: false,
    theme: 'k2so-dark',
    fontFamily: 'MesloLGM Nerd Font',
    fontLigatures: false,
    cursorStyle: 'bar',
    cursorBlink: true,
    scrollPastEnd: false,
    scrollbarAnnotations: true,
    diffStyle: 'gutter',
    formatOnSave: false,
    vimMode: false,
  },
  // Style System P3 — mirrors Rust `StyleSettings::default()`. Present in
  // every real daemon response (typed struct), so the mock carries it too;
  // the renderer's read-back applies it to <html> data-* attributes.
  style: { id: 'square', palette: 'charcoal', scheme: 'dark', gaps: '' },
  // F4 — typed AppSettings.completion_sound_enabled (default ON).
  completionSoundEnabled: true,
  // Workspace-switch focus target (default: terminal).
  workspaceSwitchFocus: 'terminal',
}

/** One-level-nested deep merge matching the daemon's `deep_merge`
 *  closely enough for settings echoes (object values merge, everything
 *  else replaces). Keeps `/cli/settings/update` faithful: the real
 *  daemon echoes the POST-MERGE snapshot, and the renderer's style
 *  read-back re-applies whatever comes back — echoing pristine defaults
 *  would visibly revert a just-committed style switch. Deterministic:
 *  output depends only on DEFAULT_SETTINGS + the request body. */
function mergeSettingsEcho(partial: unknown): unknown {
  if (typeof partial !== 'object' || partial === null) return DEFAULT_SETTINGS
  const out: Record<string, unknown> = { ...DEFAULT_SETTINGS }
  for (const [k, v] of Object.entries(partial as Record<string, unknown>)) {
    const base = out[k]
    out[k] =
      typeof v === 'object' && v !== null && !Array.isArray(v) &&
      typeof base === 'object' && base !== null && !Array.isArray(base)
        ? { ...(base as Record<string, unknown>), ...(v as Record<string, unknown>) }
        : v
  }
  return out
}

function cannedResponse(pathname: string, postBody?: unknown): { status: number; json: unknown } {
  if (pathname === '/boot-status') {
    return { status: 200, json: { version: '0.0.0-parity', protocol: 1, phase: 'ready', detail: '' } }
  }
  if (pathname === '/cli/auth/whoami') {
    return { status: 200, json: { username: 'parity', role: 'owner' } }
  }
  if (pathname === '/cli/projects/active') {
    return { status: 200, json: { projectId: null } }
  }
  if (pathname === '/cli/settings/get') {
    return { status: 200, json: DEFAULT_SETTINGS }
  }
  if (pathname === '/cli/settings/update') {
    return { status: 200, json: mergeSettingsEcho(postBody) }
  }
  // Settings → General/Terminal render ClaudeAuthRefreshRow, which
  // destructures a status-indicator config keyed by `state`. 'unknown'
  // renders NO indicator — the only timestamp-free, deterministic option
  // (any other state renders a "Valid (Nm)" countdown).
  if (pathname === '/cli/claude-auth/status') {
    return {
      status: 200,
      json: { state: 'unknown', expiresAt: null, secondsRemaining: null, schedulerInstalled: false },
    }
  }
  // Array-shaped collection routes (callers map/filter without guards).
  if (/\/(list|list-running|load-all|running|pinned|custom-names|history)$/.test(pathname)) {
    return { status: 200, json: [] }
  }
  return { status: 200, json: {} }
}

/** Intercept ALL traffic to the fake daemon host — HTTP and WS. */
export async function installMockDaemon(page: Page): Promise<void> {
  await page.route(`http://${MOCK_HOST}:${MOCK_PORT}/**`, async (route) => {
    const { pathname } = new URL(route.request().url())
    let postBody: unknown
    try {
      postBody = route.request().postDataJSON()
    } catch {
      postBody = undefined
    }
    const { status, json } = cannedResponse(pathname, postBody)
    await route.fulfill({ status, json })
  })
  // Terminal panes / event streams open WebSockets. Accept the socket but
  // never speak: panes sit in their quiet initial state, no data ever
  // arrives to mutate the UI mid-capture.
  await page.routeWebSocket(new RegExp(`^ws://${MOCK_HOST}:${MOCK_PORT}/`), () => {
    /* swallow */
  })
}

/**
 * Minimal Tauri-internals shim. Only what @tauri-apps/api actually touches:
 * metadata (window/webview labels), transformCallback/callbacks, invoke.
 * invoke() resolves benign defaults for the boot-path commands and REJECTS
 * everything else — an unexpected Tauri dependency should surface as a
 * console error, not silently succeed with a wrong shape.
 */
export async function installTauriShim(page: Page): Promise<void> {
  await page.addInitScript(() => {
    let nextId = 1
    const callbacks = new Map<number, { cb: (...a: unknown[]) => void; once: boolean }>()
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    ;(window as any).__TAURI_INTERNALS__ = {
      metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
      plugins: {},
      callbacks,
      transformCallback(cb: (...a: unknown[]) => void, once = false) {
        const id = nextId++
        callbacks.set(id, { cb, once })
        return id
      },
      runCallback(id: number, ...args: unknown[]) {
        const c = callbacks.get(id)
        if (c) {
          c.cb(...args)
          if (c.once) callbacks.delete(id)
        }
      },
      unregisterCallback(id: number) {
        callbacks.delete(id)
      },
      convertFileSrc(p: string) {
        return p
      },
      invoke(cmd: string): Promise<unknown> {
        switch (cmd) {
          case 'renderer_heartbeat':
          case 'plugin:event|unlisten':
          case 'plugin:event|emit':
          case 'plugin:resources|close':
            return Promise.resolve(null)
          case 'plugin:event|listen':
            return Promise.resolve(nextId++)
          case 'plugin:app|version':
          case 'get_current_version':
            // FIXED fake version: the real one could legitimately differ
            // between the two git states being compared.
            return Promise.resolve('0.0.0-parity')
          case 'daemon_status':
            // 'running' would render a PID + live "up 3m" uptime — the
            // not_installed branch is the only timestamp-free one.
            return Promise.resolve({ state: 'not_installed', reason: 'parity harness' })
          case 'cli_install_status':
            return Promise.resolve({
              installed: false,
              installedVersion: null,
              bundledVersion: null,
              updateAvailable: false,
            })
          case 'plugin:app|name':
            return Promise.resolve('K2')
          case 'connect_hosts_read':
            return Promise.resolve('[]')
          case 'daemon_ws_url':
            // Local daemon is "not installed" — the ONLY reachable daemon is
            // the mocked remote host, so a running production daemon on this
            // machine can never leak into a capture.
            return Promise.resolve({ state: 'not_installed', reason: 'parity harness' })
          default:
            return Promise.reject(new Error(`parity-shim: unhandled invoke '${cmd}'`))
        }
      },
    }
  })
}

/**
 * Steer the booted gate onto the mocked remote host. Runs INSIDE the page:
 * vite dev resolves '/stores/connect-host.ts' to the same live module
 * instance the app imported, so this drives the real store.
 */
export async function connectToMockDaemon(page: Page): Promise<void> {
  await page.evaluate(
    async ({ hostname, port }) => {
      const m = await import('/stores/connect-host.ts')
      const host = {
        id: 'parity-host',
        label: 'Parity',
        hostname,
        port,
        secure: false,
        username: 'parity',
        token: 'parity-token',
        remember: false,
        lastConnectedAt: null, // MUST stay null — a timestamp would render as a moving "last connected" label
      }
      m.useConnectHostStore.getState().addHost(host)
      m.useConnectHostStore.getState().selectHost(host)
    },
    { hostname: MOCK_HOST, port: MOCK_PORT },
  )
}
