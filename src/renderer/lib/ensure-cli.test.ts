import { afterEach, describe, expect, it, vi } from 'vitest'
import * as daemonCli from '@/lib/daemon-cli'
import { useToastStore } from '@/stores/toast'
import { primaryScope } from '@/kessel/server-scope'
import {
  CLI_INSTALL_COMMANDS,
  CLI_INSTALL_NOTICE_MS,
  ensureOneCli,
  installableCliProgram,
  installingCliLabel,
  resetEnsureCliForTests,
  useCliInstallStore,
} from '@/lib/ensure-cli'

vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => {}),
  listen: vi.fn(async () => () => {}),
}))

afterEach(() => {
  vi.useRealTimers()
  vi.restoreAllMocks()
  resetEnsureCliForTests()
  useToastStore.setState({ toasts: [] })
})

describe('CLI install commands', () => {
  it('copies the four installer strings and no other package', () => {
    expect(CLI_INSTALL_COMMANDS.claude).toBe('curl -fsSL https://claude.ai/install.sh | bash')
    expect(CLI_INSTALL_COMMANDS.codex).toBe(
      'curl -fsSL https://chatgpt.com/codex/install.sh | CODEX_NON_INTERACTIVE=1 sh',
    )
    expect(CLI_INSTALL_COMMANDS.grok).toBe('curl -fsSL https://x.ai/cli/install.sh | bash')
    expect(CLI_INSTALL_COMMANDS.gemini).toBe(
      'npm install -g --prefix "$HOME/.local" @google/gemini-cli',
    )
    expect(CLI_INSTALL_COMMANDS.gemini).not.toContain('@anthropic-ai/gemini-cli')
    expect(CLI_INSTALL_COMMANDS.grok).not.toContain('claude.ai')
    expect(CLI_INSTALL_COMMANDS.grok).not.toContain('chatgpt.com')
    expect(installingCliLabel('claude')).toBe('Installing Claude…')
    expect(installingCliLabel('codex')).toBe('Installing Codex…')
    expect(installingCliLabel('grok')).toBe('Installing Grok…')
    expect(installingCliLabel('gemini')).toBe('Installing Gemini…')
  })

  it('matches grok --always-approve and ignores a path token', () => {
    expect(installableCliProgram('grok --always-approve')).toBe('grok')
    expect(installableCliProgram('claude --dangerously-skip-permissions')).toBe('claude')
    expect(installableCliProgram('codex --yolo')).toBe('codex')
    expect(installableCliProgram('gemini --yolo')).toBe('gemini')
    expect(installableCliProgram('/usr/bin/claude')).toBeNull()
    expect(installableCliProgram('/opt/homebrew/bin/grok --always-approve')).toBeNull()
    expect(installableCliProgram('aider --model gpt-4')).toBeNull()
  })
})

describe('ensureOneCli notice', () => {
  it('does not show Installing when the binary is already present', async () => {
    vi.useFakeTimers()
    vi.spyOn(daemonCli, 'daemonCliPost').mockResolvedValue({ ok: true, installed: false })
    const pending = ensureOneCli(primaryScope(), 'grok')
    await pending
    expect(useCliInstallStore.getState().installing).toBeNull()
    await vi.advanceTimersByTimeAsync(CLI_INSTALL_NOTICE_MS + 1000)
    expect(useCliInstallStore.getState().installing).toBeNull()
  })

  it('shows Installing Grok… only while that installer is in flight', async () => {
    vi.useFakeTimers()
    let resolvePost: (value: { ok: true; installed: true }) => void = () => {}
    vi.spyOn(daemonCli, 'daemonCliPost').mockImplementation(
      () =>
        new Promise((resolve) => {
          resolvePost = resolve
        }),
    )
    const pending = ensureOneCli(primaryScope(), 'grok')
    expect(useCliInstallStore.getState().installing).toBeNull()
    await vi.advanceTimersByTimeAsync(CLI_INSTALL_NOTICE_MS)
    expect(useCliInstallStore.getState().installing).toBe('grok')
    expect(installingCliLabel('grok')).toBe('Installing Grok…')
    resolvePost({ ok: true, installed: true })
    await pending
    expect(useCliInstallStore.getState().installing).toBeNull()
  })

  it('toasts the daemon error and does not claim success', async () => {
    vi.spyOn(daemonCli, 'daemonCliPost').mockRejectedValue(
      new Error('grok installed but not on PATH'),
    )
    await expect(ensureOneCli(primaryScope(), 'grok')).rejects.toThrow('installed but not on PATH')
    expect(useToastStore.getState().toasts.map((t) => t.message)).toContain(
      'grok installed but not on PATH',
    )
    expect(useCliInstallStore.getState().installing).toBeNull()
  })
})
