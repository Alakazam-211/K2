import { daemonCliPost } from '@/lib/daemon-cli'
import { parseCommand } from '@/lib/agent-resolve'
import { useToastStore } from '@/stores/toast'
import { create } from 'zustand'
import { primaryScope } from '@/kessel/server-scope'

/** Installers the daemon will run. Copy buttons use these same strings. */
export const CLI_INSTALL_COMMANDS = {
  claude: 'curl -fsSL https://claude.ai/install.sh | bash',
  codex: 'curl -fsSL https://chatgpt.com/codex/install.sh | CODEX_NON_INTERACTIVE=1 sh',
  grok: 'curl -fsSL https://x.ai/cli/install.sh | bash',
  gemini: 'npm install -g --prefix "$HOME/.local" @google/gemini-cli',
} as const

export type InstallableCli = keyof typeof CLI_INSTALL_COMMANDS

const CLI_INSTALL_LABELS: Record<InstallableCli, string> = {
  claude: 'Claude',
  codex: 'Codex',
  grok: 'Grok',
  gemini: 'Gemini',
}

/** Delay before the GUI says an install is running. A basename that is
 * already on the daemon PATH returns before this, so it never shows. */
export const CLI_INSTALL_NOTICE_MS = 250

export function isInstallableCli(command: string): command is InstallableCli {
  return Object.prototype.hasOwnProperty.call(CLI_INSTALL_COMMANDS, command)
}

export function installingCliLabel(program: InstallableCli): string {
  return `Installing ${CLI_INSTALL_LABELS[program]}…`
}

/**
 * First token of `parseCommand`. A path (`/usr/bin/claude`) or any other
 * basename is not installed.
 */
export function installableCliProgram(commandStr: string): InstallableCli | null {
  const { command } = parseCommand(commandStr)
  if (!command || command.includes('/') || command.includes('\\')) return null
  return isInstallableCli(command) ? command : null
}

interface CliInstallState {
  installing: InstallableCli | null
  setInstalling: (program: InstallableCli | null) => void
}

export const useCliInstallStore = create<CliInstallState>((set) => ({
  installing: null,
  setInstalling: (program) => set({ installing: program }),
}))

export interface EnsureCliResult {
  ok: true
  installed: boolean
}

const inflight = new Map<InstallableCli, Promise<EnsureCliResult>>()

export function resetEnsureCliForTests(): void {
  inflight.clear()
  useCliInstallStore.setState({ installing: null })
}

/**
 * Ask the daemon to install one CLI if it is missing. Throws on installer
 * failure, timeout, or "installed but not on PATH" — callers must not open
 * a tab after a throw. An already-present basename resolves without the
 * installing notice.
 */
export function ensureOneCli(program: InstallableCli): Promise<EnsureCliResult> {
  const existing = inflight.get(program)
  if (existing) return existing
  const pending = runEnsure(program).finally(() => {
    inflight.delete(program)
  })
  inflight.set(program, pending)
  return pending
}

async function runEnsure(program: InstallableCli): Promise<EnsureCliResult> {
  let showed = false
  const timer = setTimeout(() => {
    showed = true
    useCliInstallStore.getState().setInstalling(program)
  }, CLI_INSTALL_NOTICE_MS)
  try {
    const result = await daemonCliPost<EnsureCliResult>(primaryScope(), 'agents/ensure-cli', { program })
    if (result?.installed !== true && result?.installed !== false) {
      throw new Error('ensure-cli returned an unexpected body')
    }
    if (!result.installed) {
      clearTimeout(timer)
      if (showed) useCliInstallStore.getState().setInstalling(null)
    }
    return { ok: true, installed: result.installed }
  } catch (err) {
    clearTimeout(timer)
    const message = err instanceof Error && err.message ? err.message : 'CLI install failed'
    useToastStore.getState().addToast(message, 'error')
    throw err instanceof Error && err.message ? err : new Error(message)
  } finally {
    clearTimeout(timer)
    if (useCliInstallStore.getState().installing === program) {
      useCliInstallStore.getState().setInstalling(null)
    }
  }
}
