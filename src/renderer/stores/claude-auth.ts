import { create } from 'zustand'
import { getDaemonWs, daemonHttpBase } from '@/kessel/daemon-ws'

// Phase 2 Unit 5 — Claude Auth scheduler lives in k2so-daemon
// (`/cli/claude-auth/*`). The Tauri-side `claude_auth_*` commands
// were deleted along with `src-tauri/src/commands/claude_auth.rs`.
// Helpers below call the daemon directly so the Settings panel
// keeps working with no `invoke` shim in the middle.

export type ClaudeAuthState = 'valid' | 'expiring' | 'expired' | 'missing' | 'unknown'

interface ClaudeAuthStatusResponse {
  state: string
  expiresAt: number | null
  secondsRemaining: number | null
  schedulerInstalled: boolean
}

interface ClaudeAuthStore {
  state: ClaudeAuthState
  expiresAt: number | null
  secondsRemaining: number | null
  schedulerInstalled: boolean
  refreshing: boolean
  lastError: string | null

  fetchStatus: () => Promise<void>
  refresh: () => Promise<void>
  installScheduler: () => Promise<void>
  uninstallScheduler: () => Promise<void>
}

async function daemonGet(pathSuffix: string): Promise<Response> {
  const creds = await getDaemonWs()
  return fetch(
    `${daemonHttpBase(creds)}/cli/claude-auth/${pathSuffix}?token=${creds.token}`,
    { method: 'GET' },
  )
}

async function daemonPost(pathSuffix: string): Promise<Response> {
  const creds = await getDaemonWs()
  return fetch(
    `${daemonHttpBase(creds)}/cli/claude-auth/${pathSuffix}?token=${creds.token}`,
    {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: '{}',
    },
  )
}

async function readDaemonError(res: Response, fallback: string): Promise<string> {
  const text = await res.text()
  try {
    const parsed = JSON.parse(text)
    if (parsed && typeof parsed.error === 'string') return parsed.error
  } catch {
    /* keep raw text */
  }
  return text || fallback
}

async function statusFromDaemon(): Promise<ClaudeAuthStatusResponse> {
  const res = await daemonGet('status')
  if (!res.ok) {
    throw new Error(await readDaemonError(res, `claude-auth status ${res.status}`))
  }
  return (await res.json()) as ClaudeAuthStatusResponse
}

async function refreshOnDaemon(): Promise<ClaudeAuthStatusResponse> {
  const res = await daemonPost('refresh-now')
  if (!res.ok) {
    throw new Error(await readDaemonError(res, `claude-auth refresh-now ${res.status}`))
  }
  return (await res.json()) as ClaudeAuthStatusResponse
}

async function installSchedulerOnDaemon(): Promise<void> {
  const res = await daemonPost('install-scheduler')
  if (!res.ok) {
    throw new Error(await readDaemonError(res, `claude-auth install-scheduler ${res.status}`))
  }
}

async function uninstallSchedulerOnDaemon(): Promise<void> {
  const res = await daemonPost('uninstall-scheduler')
  if (!res.ok) {
    throw new Error(await readDaemonError(res, `claude-auth uninstall-scheduler ${res.status}`))
  }
}

export const useClaudeAuthStore = create<ClaudeAuthStore>((set) => ({
  state: 'unknown',
  expiresAt: null,
  secondsRemaining: null,
  schedulerInstalled: false,
  refreshing: false,
  lastError: null,

  fetchStatus: async () => {
    try {
      const status = await statusFromDaemon()
      set({
        state: status.state as ClaudeAuthState,
        expiresAt: status.expiresAt,
        secondsRemaining: status.secondsRemaining,
        schedulerInstalled: status.schedulerInstalled,
        lastError: null,
      })
    } catch (e) {
      set({ lastError: String(e) })
    }
  },

  refresh: async () => {
    set({ refreshing: true, lastError: null })
    try {
      const status = await refreshOnDaemon()
      set({
        state: status.state as ClaudeAuthState,
        expiresAt: status.expiresAt,
        secondsRemaining: status.secondsRemaining,
        refreshing: false,
        lastError: null,
      })
    } catch (e) {
      set({ refreshing: false, lastError: String(e) })
    }
  },

  installScheduler: async () => {
    try {
      await installSchedulerOnDaemon()
      set({ schedulerInstalled: true, lastError: null })
    } catch (e) {
      set({ lastError: String(e) })
      throw e
    }
  },

  uninstallScheduler: async () => {
    try {
      await uninstallSchedulerOnDaemon()
      set({ schedulerInstalled: false, lastError: null })
    } catch (e) {
      set({ lastError: String(e) })
      throw e
    }
  },
}))
