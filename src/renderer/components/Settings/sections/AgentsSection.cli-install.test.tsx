// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { AgentsSection } from '@/components/Settings/sections/AgentsSection'
import { CLI_INSTALL_COMMANDS, useCliInstallStore } from '@/lib/ensure-cli'
import { usePresetsStore } from '@/stores/presets'

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => null),
}))

vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => {}),
  listen: vi.fn(async () => () => {}),
}))

const realFetch = usePresetsStore.getState().fetchPresets

function expand(name: RegExp): void {
  fireEvent.click(screen.getByRole('button', { name }))
}

describe('CLI Tools Setup install rows', () => {
  beforeEach(() => {
    useCliInstallStore.setState({ installing: null })
    usePresetsStore.setState({
      presets: [],
      showPresetsBar: true,
      fetchPresets: vi.fn(async () => {}),
    })
  })

  afterEach(() => {
    cleanup()
    useCliInstallStore.setState({ installing: null })
    usePresetsStore.setState({
      presets: [],
      showPresetsBar: true,
      fetchPresets: realFetch,
    })
  })

  it('copies the four commands, not the old package names', () => {
    render(<AgentsSection />)
    expand(/Claude Code/)
    expect(screen.getByText(CLI_INSTALL_COMMANDS.claude)).toBeTruthy()
    expect(screen.getAllByText(/Then add the subscription under Language Models → Subs \/ Tokens\./).length).toBe(1)
    expect(document.body.textContent).not.toMatch(/Node\.js/)
    expect(screen.getByRole('button', { name: 'Install' })).toBeTruthy()
    expand(/Claude Code/)

    expand(/OpenAI Codex/)
    expect(screen.getByText(CLI_INSTALL_COMMANDS.codex)).toBeTruthy()
    expect(screen.getAllByText(/Then add the subscription under Language Models → Subs \/ Tokens\./).length).toBe(1)
    expect(document.body.textContent).not.toContain('codex --login')
    expand(/OpenAI Codex/)

    expand(/^Grok/)
    expect(screen.getByText(CLI_INSTALL_COMMANDS.grok)).toBeTruthy()
    expand(/^Grok/)

    expand(/Gemini CLI/)
    expect(screen.getByText(CLI_INSTALL_COMMANDS.gemini)).toBeTruthy()
    expect(document.body.textContent).not.toContain('@anthropic-ai/gemini-cli')
    expand(/Gemini CLI/)

    expand(/Cursor Agent/)
    expect(screen.queryByRole('button', { name: 'Install' })).toBeNull()
  })

  it('shows Installing Grok… on that row only while Grok is installing', () => {
    useCliInstallStore.setState({ installing: 'grok' })
    render(<AgentsSection />)
    expand(/^Grok/)
    expect(screen.getByRole('button', { name: 'Installing Grok…' })).toBeTruthy()
    expect(screen.queryByRole('button', { name: 'Install' })).toBeNull()
  })
})

describe('Settings → Language Models → Models page (0.46: Models · Subs / Tokens · Usage)', () => {
  beforeEach(() => {
    usePresetsStore.setState({ presets: [], showPresetsBar: true, fetchPresets: vi.fn(async () => {}) })
  })
  afterEach(() => {
    cleanup()
    usePresetsStore.setState({ presets: [], showPresetsBar: true, fetchPresets: realFetch })
  })

  it('keeps setup only: Defaults, launch bar, presets on the left, CLI Tools Setup on the right; Tokens is its own page', () => {
    render(<AgentsSection />)
    const row = document.body.firstElementChild!.firstElementChild as HTMLElement
    expect(row.className).toBe('flex h-full min-h-0')
    expect(row.children.length).toBe(2)
    const [left, right] = [...row.children] as HTMLElement[]
    expect(left.querySelector('[data-settings-id="agents.default-agent"]')).not.toBeNull()
    expect(left.querySelector('[data-settings-id="agents.show-launch-bar"]')).not.toBeNull()
    expect(left.querySelector('[data-settings-id="agents.agent-presets"]')).not.toBeNull()
    expect(left.textContent).not.toContain('CLI Tools Setup')
    expect(right.querySelector('h2')?.textContent).toBe('CLI Tools Setup')
    expect(right.querySelector('[data-settings-id="agents.cli-claude"]')).not.toBeNull()
    // The Tokens column moved to Settings → Language Models → Subs / Tokens.
    expect(document.querySelector('[data-settings-id="agents.accounts"]')).toBeNull()
    expect(document.querySelector('[data-settings-id="agents.add-login"]')).toBeNull()
    expect(screen.queryByRole('heading', { name: 'Subs / Tokens' })).toBeNull()
    expect(document.body.textContent).not.toContain('LLMs → Tokens')
  })
})
