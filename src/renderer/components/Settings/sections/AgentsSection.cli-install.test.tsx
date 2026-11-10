// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { AgentsSection, injectFlowSummary } from '@/components/Settings/sections/AgentsSection'
import { CLI_INSTALL_COMMANDS, useCliInstallStore } from '@/lib/ensure-cli'
import * as daemonCli from '@/lib/daemon-cli'
import { DaemonHttpError } from '@/lib/daemon-http-error'
import { usePresetsStore, type AgentPreset } from '@/stores/presets'
import { useSettingsStore } from '@/stores/settings'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => null),
}))

vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => {}),
  listen: vi.fn(async () => () => {}),
}))

const realFetch = usePresetsStore.getState().fetchPresets
const realReset = usePresetsStore.getState().resetPresetsToBuiltIns
const realDelete = usePresetsStore.getState().deletePreset
const realUpdate = usePresetsStore.getState().updatePreset
const realConfirm = useConfirmDialogStore.getState().confirm
const realSettings = { defaultAgent: useSettingsStore.getState().defaultAgent, setDefaultAgent: useSettingsStore.getState().setDefaultAgent }

const preset = (id: string, label: string, command: string, extra: Partial<AgentPreset> = {}): AgentPreset => ({
  id, label, command, icon: '', enabled: 1, sortOrder: 0, isBuiltIn: 1, createdAt: 1, inject_flow: null, ...extra,
})
const SAMPLE = [
  preset('b1', 'Claude', 'claude --dangerously-skip-permissions'),
  preset('b2', 'Codex', 'codex --yolo'),
  preset('c1', 'Claude review', 'claude --permission-mode plan', { isBuiltIn: 0 }),
]

type Tool = { program: string; installed: boolean; version: string | null; path: string | null; launches: boolean; error: string | null }
const tool = (program: string, over: Partial<Tool> = {}): Tool => ({ program, installed: false, version: null, path: null, launches: false, error: null, ...over })
const installedTool = (program: string, version: string): Tool => tool(program, { installed: true, version, path: `/u/.local/bin/${program}`, launches: true })
type CliReply = { ok?: true; cacheSecs?: number; tools: Tool[] } | Error | 'hang'
function mockCliStatus(reply: CliReply | (() => CliReply)) {
  return vi.spyOn(daemonCli, 'daemonCliGet').mockImplementation((async (_scope: unknown, route: string) => {
    if (route !== 'agents/cli-status') throw new Error(`unexpected GET ${route}`)
    const r = typeof reply === 'function' ? reply() : reply
    if (r === 'hang') return new Promise(() => {})
    if (r instanceof Error) throw r
    return { ok: true, cacheSecs: 60, ...r }
  }) as typeof daemonCli.daemonCliGet)
}

function setPresets(presets: AgentPreset[], extra: Partial<ReturnType<typeof usePresetsStore.getState>> = {}): void {
  usePresetsStore.setState({ presets, presetsLoaded: true, presetsError: null, showPresetsBar: true, fetchPresets: vi.fn(async () => {}), ...extra })
}

function toolRow(program: string): HTMLElement {
  return screen.getByTestId(`tool-row-${program}`)
}

beforeEach(() => {
  useCliInstallStore.setState({ installing: null })
  setPresets(SAMPLE)
  mockCliStatus({ tools: [installedTool('claude', '2.1.12'), installedTool('codex', '0.58.0'), tool('grok'), tool('gemini')] })
})

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
  useCliInstallStore.setState({ installing: null })
  usePresetsStore.setState({
    presets: [], presetsLoaded: false, presetsError: null, showPresetsBar: true,
    fetchPresets: realFetch, resetPresetsToBuiltIns: realReset, deletePreset: realDelete, updatePreset: realUpdate,
  })
  useConfirmDialogStore.setState({ confirm: realConfirm })
  useSettingsStore.setState(realSettings)
})

describe('Settings → Language Models → Providers / Models page', () => {
  it('is one page: a header, then Starting agents, Launch presets and Agent tools card groups', async () => {
    render(<AgentsSection />)
    expect(screen.getByRole('heading', { level: 2, name: 'Providers / Models' })).toBeTruthy()
    const headings = screen.getAllByRole('heading', { level: 3 }).map((h) => h.textContent)
    expect(headings).toEqual(['Starting agents', 'Launch presets', 'Agent tools on this computer'])
    // The header bridges the nav name to what the page holds.
    expect(screen.getByRole('heading', { level: 2 }).nextElementSibling?.textContent).toContain('the providers behind them')
    for (const id of ['agents.default-agent', 'agents.show-launch-bar', 'agents.agent-presets', 'agents.add-preset', 'agents.reset-built-ins']) {
      expect(document.querySelector(`[data-settings-id="${id}"]`), id).not.toBeNull()
    }
    expect(document.body.textContent).not.toContain('CLI Tools Setup')
    // Tokens is its own page: Settings → Language Models → Subs / Tokens.
    expect(document.querySelector('[data-settings-id="agents.accounts"]')).toBeNull()
    await screen.findByTestId('tool-row-claude')
  })

  it('shows each preset with its command, an on switch and a menu; only your own presets can be deleted', async () => {
    render(<AgentsSection />)
    const row = screen.getByTestId('preset-row-b1')
    expect(within(row).getByText('claude --dangerously-skip-permissions')).toBeTruthy()
    expect(within(row).getByRole('switch', { name: 'Show Claude on the launch bar' }).getAttribute('aria-checked')).toBe('true')
    fireEvent.click(within(row).getByRole('button', { name: 'More actions for Claude' }))
    expect(screen.queryByRole('menuitem', { name: /Delete/ })).toBeNull()
    expect(screen.getByRole('menuitem', { name: /Move up/ }).getAttribute('aria-disabled')).toBe('true')
    fireEvent.keyDown(screen.getByRole('menu'), { key: 'Escape' })

    fireEvent.click(within(screen.getByTestId('preset-row-c1')).getByRole('button', { name: 'More actions for Claude review' }))
    expect(screen.getByRole('menuitem', { name: /Delete/ })).toBeTruthy()
    expect(within(screen.getByTestId('preset-row-c1')).getByText('Custom')).toBeTruthy()
  })

  it('asks before deleting, and says so when the delete fails', async () => {
    const confirm = vi.fn(async () => true)
    useConfirmDialogStore.setState({ confirm })
    const deletePreset = vi.fn(async () => { throw new Error('database is locked') })
    usePresetsStore.setState({ deletePreset })
    render(<AgentsSection />)
    fireEvent.click(screen.getByRole('button', { name: 'More actions for Claude review' }))
    fireEvent.click(screen.getByRole('menuitem', { name: /Delete/ }))
    await waitFor(() => expect(deletePreset).toHaveBeenCalledWith('c1'))
    expect(confirm).toHaveBeenCalledWith(expect.objectContaining({ title: 'Delete Claude review?', destructive: true }))
    expect((await screen.findByRole('alert')).textContent).toContain('K2 couldn’t delete Claude review. database is locked')
  })

  it('restores the built-ins only after you confirm, then says Restored', async () => {
    const resetPresetsToBuiltIns = vi.fn(async () => {})
    usePresetsStore.setState({ resetPresetsToBuiltIns })
    const confirm = vi.fn(async () => false)
    useConfirmDialogStore.setState({ confirm })
    render(<AgentsSection />)
    fireEvent.click(screen.getByRole('button', { name: 'Restore…' }))
    await waitFor(() => expect(confirm).toHaveBeenCalledTimes(1))
    expect(resetPresetsToBuiltIns).not.toHaveBeenCalled()

    confirm.mockResolvedValue(true)
    fireEvent.click(screen.getByRole('button', { name: 'Restore…' }))
    await waitFor(() => expect(resetPresetsToBuiltIns).toHaveBeenCalledTimes(1))
    expect(await screen.findByText('Restored')).toBeTruthy()
  })

  it('designs loading, empty and error states for the preset list', async () => {
    setPresets([], { presetsLoaded: false })
    const { unmount } = render(<AgentsSection />)
    expect(screen.getByTestId('presets-loading').textContent).toContain('Loading your presets')
    unmount()

    setPresets([])
    const empty = render(<AgentsSection />)
    expect(screen.getByTestId('presets-empty').textContent).toContain('No presets yet')
    expect(within(screen.getByTestId('presets-empty')).getByRole('button', { name: 'Restore…' })).toBeTruthy()
    empty.unmount()

    const fetchPresets = vi.fn(async () => {})
    setPresets([], { presetsError: 'database is locked', fetchPresets })
    render(<AgentsSection />)
    const err = screen.getByTestId('presets-load-error')
    expect(err.textContent).toContain('Couldn’t load your presets')
    // No Add preset while the list can't be read.
    expect(screen.queryByRole('button', { name: 'Add preset' })).toBeNull()
    fireEvent.click(within(err).getByRole('button', { name: 'Try again' }))
    expect(fetchPresets).toHaveBeenCalled()
  })

  it('the add form needs a name and a command, and Esc closes it', async () => {
    render(<AgentsSection />)
    fireEvent.click(screen.getByRole('button', { name: 'Add preset' }))
    const form = screen.getByTestId('preset-form')
    const add = within(form).getByRole('button', { name: 'Add preset' }) as HTMLButtonElement
    expect(add.disabled).toBe(true)
    expect(form.textContent).toContain('Enter a name and a command to save.')
    fireEvent.change(within(form).getByLabelText('Name'), { target: { value: 'Aider' } })
    fireEvent.change(within(form).getByLabelText('Command'), { target: { value: 'aider' } })
    expect(add.disabled).toBe(false)
    fireEvent.keyDown(within(form).getByLabelText('Name'), { key: 'Escape' })
    expect(screen.queryByTestId('preset-form')).toBeNull()
  })

  it('marks the preset the default agent uses, and confirms a new default with Saved', async () => {
    useSettingsStore.setState({ defaultAgent: 'b1', setDefaultAgent: async (id: string) => { useSettingsStore.setState({ defaultAgent: id }) } } as never)
    render(<AgentsSection />)
    expect(within(screen.getByTestId('preset-row-b1')).getByText('Default')).toBeTruthy()
    expect(within(screen.getByTestId('preset-row-b2')).queryByText('Default')).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Default agent' }))
    fireEvent.click(within(screen.getByTestId('setting-dropdown-menu')).getByRole('button', { name: /Codex/ }))
    expect(await screen.findByText('Saved')).toBeTruthy()
    expect(within(screen.getByTestId('preset-row-b2')).getByText('Default')).toBeTruthy()
  })

  it('says a failed default agent save out loud', async () => {
    useSettingsStore.setState({ defaultAgent: 'b1', setDefaultAgent: async () => {} } as never)
    render(<AgentsSection />)
    fireEvent.click(screen.getByRole('button', { name: 'Default agent' }))
    fireEvent.click(within(screen.getByTestId('setting-dropdown-menu')).getByRole('button', { name: /Codex/ }))
    expect((await screen.findByRole('alert')).textContent).toContain('Couldn’t save')
  })

  it('a preset added while the launch bar is off says where it is, with the switch one click away', async () => {
    const created = preset('n1', 'Aider', 'aider', { isBuiltIn: 0 })
    setPresets(SAMPLE, { showPresetsBar: false, createPreset: vi.fn(async () => created) } as never)
    const setShowLaunchBar = vi.fn((shown: boolean) => usePresetsStore.setState({ showPresetsBar: shown }))
    usePresetsStore.setState({ setShowLaunchBar } as never)
    render(<AgentsSection />)
    fireEvent.click(screen.getByRole('button', { name: 'Add preset' }))
    const form = screen.getByTestId('preset-form')
    fireEvent.change(within(form).getByLabelText('Name'), { target: { value: 'Aider' } })
    fireEvent.change(within(form).getByLabelText('Command'), { target: { value: 'aider' } })
    fireEvent.click(within(form).getByRole('button', { name: 'Add preset' }))
    const note = await screen.findByTestId('preset-added-launch-bar-off')
    expect(note.textContent).toContain('Aider is added')
    expect(note.textContent).toContain('It’s in the tab + menu. Turn on Show launch bar to see it as a button.')
    fireEvent.click(within(note).getByRole('button', { name: 'Show launch bar' }))
    expect(setShowLaunchBar).toHaveBeenCalledWith(true)
    expect(screen.queryByTestId('preset-added-launch-bar-off')).toBeNull()
  })

  it('sums up the message keys in one line', () => {
    expect(injectFlowSummary([{ key: 'paste', waitMs: 0 }, { key: 'return', waitMs: 150 }])).toBe('Paste message, then Return')
  })
})

describe('Agent tools on this computer', () => {
  const FOUR = [installedTool('claude', '2.1.12'), installedTool('codex', '0.58.0'), tool('grok'), tool('gemini')]

  it('lists installed tools first with their version, the rest folded under More agents', async () => {
    mockCliStatus({ tools: FOUR })
    render(<AgentsSection />)
    await screen.findByTestId('tool-row-claude')
    expect(within(toolRow('claude')).getByText('Installed')).toBeTruthy()
    expect(screen.getByTestId('tool-status-claude').textContent).toBe('Version 2.1.12')
    expect(screen.getByTestId('tool-status-codex').textContent).toBe('Version 0.58.0')
    expect(screen.queryByTestId('tool-row-grok')).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: /More agents you can install/ }))
    expect(screen.getByTestId('tool-status-grok').textContent).toBe('Not installed. K2 installs it the first time you start it.')
    expect(within(toolRow('grok')).queryByText('Installed')).toBeNull()
    expect(within(toolRow('grok')).getByRole('button', { name: 'Show how to install Grok' })).toBeTruthy()
  })

  it('a broken tool says it is installed but didn’t answer, with the reason quietly and a next step', async () => {
    const recheck = mockCliStatus({ tools: [installedTool('claude', '2.1.12'), tool('codex', { installed: true, path: '/usr/local/bin/codex', launches: true, error: '--version exited 1: bad option' }), tool('grok'), tool('gemini')] })
    render(<AgentsSection />)
    await screen.findByTestId('tool-row-codex')
    expect(within(toolRow('codex')).getByText('Not working')).toBeTruthy()
    expect(within(toolRow('codex')).queryByText('Installed')).toBeNull()
    expect(screen.getByTestId('tool-status-codex').textContent).toBe('Installed, but it didn’t answer when K2 checked.')
    fireEvent.click(screen.getByRole('button', { name: 'Details for OpenAI Codex' }))
    const broken = screen.getByTestId('tool-broken-codex')
    expect(broken.textContent).toContain('didn’t answer when K2 asked for its version')
    expect(broken.textContent).toContain('Found at /usr/local/bin/codex. It said: --version exited 1: bad option.')
    expect(screen.getByTestId('tool-help-codex').textContent).toContain('to reinstall it, then check again')
    // Installing again wouldn't help (it's already there); checking again does.
    expect(screen.queryByRole('button', { name: 'Install OpenAI Codex' })).toBeNull()
    const calls = recheck.mock.calls.length
    await act(async () => { fireEvent.click(within(screen.getByTestId('tool-help-codex')).getByRole('button', { name: 'Check again' })) })
    expect(recheck.mock.calls.length).toBe(calls + 1)
  })

  it('a missing tool says Not installed and offers Install', async () => {
    mockCliStatus({ tools: [installedTool('claude', '2.1.12'), tool('codex'), tool('grok'), tool('gemini')] })
    render(<AgentsSection />)
    await screen.findByTestId('tool-row-claude')
    fireEvent.click(screen.getByRole('button', { name: /More agents/ }))
    expect(screen.getByTestId('tool-status-gemini').textContent).toContain('Not installed')
    fireEvent.click(screen.getByRole('button', { name: 'Show how to install Gemini CLI' }))
    expect(screen.getByText(CLI_INSTALL_COMMANDS.gemini)).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Install Gemini CLI' })).toBeTruthy()
  })

  it('tools the server doesn’t list get no row, only one quiet Not supported line; never Unknown or Not installed', async () => {
    mockCliStatus({ tools: FOUR })
    render(<AgentsSection />)
    const line = await screen.findByTestId('tools-not-supported')
    expect(line.textContent).toContain('Not supported on this computer')
    expect(line.textContent).toContain('Cursor Agent, Pi, Hermes and 6 more')
    fireEvent.click(screen.getByRole('button', { name: /More agents/ }))
    for (const p of ['cursor-agent', 'pi', 'aider', 'copilot']) expect(screen.queryByTestId(`tool-row-${p}`), p).toBeNull()
    expect(document.body.textContent).not.toMatch(/Unknown/i)
    // Search still lands somewhere for a tool that isn't listed.
    expect(line.querySelector('[data-settings-id="agents.cli-cursor-agent"]')).not.toBeNull()
  })

  it('a listed tool beyond the four gets its own row', async () => {
    mockCliStatus({ tools: [...FOUR, installedTool('cursor-agent', '2025.11.1')] })
    render(<AgentsSection />)
    expect((await screen.findByTestId('tool-status-cursor-agent')).textContent).toBe('Version 2025.11.1')
    expect(screen.getByTestId('tools-not-supported').textContent).not.toContain('Cursor Agent')
  })

  it('an older daemon without the route: one quiet notice, and no row claims an install state', async () => {
    mockCliStatus(new DaemonHttpError('route not found', 404, null))
    render(<AgentsSection />)
    expect((await screen.findByTestId('tools-unsupported')).textContent).toContain('K2 can’t check which tools are installed on this computer until it’s updated')
    fireEvent.click(screen.getByRole('button', { name: /More agents/ }))
    expect(document.body.textContent).not.toContain('Not installed')
    expect(screen.queryByText('Installed')).toBeNull()
    expect(screen.queryByTestId('tools-not-supported')).toBeNull()
    expect(screen.getByTestId('tool-row-cursor-agent')).toBeTruthy()
    expect(screen.queryByTestId('tool-status-cursor-agent')).toBeNull()
    expect(screen.queryAllByRole('alert')).toHaveLength(0)
  })

  it('a failed check says so once, with the reason and Try again', async () => {
    let reply: CliReply = new DaemonHttpError('timed out', 500, null)
    mockCliStatus(() => reply)
    render(<AgentsSection />)
    const err = await screen.findByTestId('tools-error')
    expect(err.getAttribute('role')).toBe('alert')
    expect(err.textContent).toContain('Couldn’t check which agent tools are installed')
    expect(err.textContent).toContain('The server said: timed out.')
    expect(screen.getAllByRole('alert')).toHaveLength(1)
    reply = { tools: [installedTool('claude', '2.1.12')] }
    await act(async () => { fireEvent.click(within(err).getByRole('button', { name: 'Try again' })) })
    expect((await screen.findByTestId('tool-status-claude')).textContent).toBe('Version 2.1.12')
  })

  it('says Checking… in each row while the check runs, and doesn’t guess', () => {
    mockCliStatus('hang')
    render(<AgentsSection />)
    expect(screen.getByTestId('tools-loading').textContent).toContain('Checking this computer')
    expect(screen.getByTestId('tool-status-claude').textContent).toBe('Checking whether it’s installed…')
    expect(screen.getByRole('button', { name: 'Details for Claude Code' })).toBeTruthy()
    // The fold waits for the answer, so it never lists a tool that isn't supported here.
    expect(screen.queryByRole('button', { name: /More agents/ })).toBeNull()
    expect(document.body.textContent).not.toContain('Not installed')
  })

  it('checks once on open, with no polling', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true })
    try {
      const get = mockCliStatus({ tools: FOUR })
      render(<AgentsSection />)
      await screen.findByTestId('tool-row-claude')
      await act(async () => { vi.advanceTimersByTime(5 * 60_000) })
      expect(get.mock.calls.filter((c) => c[1] === 'agents/cli-status')).toHaveLength(1)
    } finally {
      vi.useRealTimers()
    }
  })

  it('shows the four installers K2 runs, and Cursor’s own installer when it can’t tell', async () => {
    mockCliStatus(new DaemonHttpError('route_unclassified', 404, null))
    render(<AgentsSection />)
    await screen.findByTestId('tools-unsupported')
    fireEvent.click(screen.getByRole('button', { name: 'Show how to install Claude Code' }))
    expect(screen.getByText(CLI_INSTALL_COMMANDS.claude)).toBeTruthy()
    expect(screen.getByText(/add your subscription in Language Models → Subs \/ Tokens/)).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Install Claude Code' })).toBeTruthy()
    for (const [name, cmd] of [['OpenAI Codex', CLI_INSTALL_COMMANDS.codex], ['Grok', CLI_INSTALL_COMMANDS.grok], ['Gemini CLI', CLI_INSTALL_COMMANDS.gemini]] as const) {
      fireEvent.click(screen.getByRole('button', { name: `Show how to install ${name}` }))
      expect(screen.getByText(cmd)).toBeTruthy()
    }
    expect(document.body.textContent).not.toContain('@anthropic-ai/gemini-cli')

    fireEvent.click(screen.getByRole('button', { name: /More agents/ }))
    fireEvent.click(screen.getByRole('button', { name: 'Show how to install Cursor Agent' }))
    const help = screen.getByTestId('tool-help-cursor-agent')
    // Cursor's own installer; the npm `cursor-agent` package is a third party's.
    expect(within(help).getByText('curl https://cursor.com/install -fsS | bash')).toBeTruthy()
    expect(within(help).queryByRole('button', { name: /^Install/ })).toBeNull()
  })

  it('shows Installing Grok… on that row only while Grok is installing', async () => {
    mockCliStatus({ tools: [tool('claude'), tool('codex'), tool('grok'), tool('gemini')] })
    useCliInstallStore.setState({ installing: 'grok' })
    render(<AgentsSection />)
    await screen.findByTestId('tools-none')
    fireEvent.click(screen.getByRole('button', { name: 'Show how to install Grok' }))
    expect(screen.getByRole('button', { name: 'Installing Grok…' })).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Show how to install OpenAI Codex' }))
    expect(screen.getByRole('button', { name: 'Install OpenAI Codex' })).toBeTruthy()
  })

  it('after Install, checks again and the row says Installed with its version and the next step', async () => {
    let done = false
    mockCliStatus(() => ({ tools: [installedTool('claude', '2.1.12'), tool('codex'), done ? installedTool('grok', '0.9.3') : tool('grok'), tool('gemini')] }))
    vi.spyOn(daemonCli, 'daemonCliPost').mockImplementation((async () => { done = true; return { ok: true, installed: true } }) as typeof daemonCli.daemonCliPost)
    render(<AgentsSection />)
    await screen.findByTestId('tool-row-claude')
    fireEvent.click(screen.getByRole('button', { name: /More agents/ }))
    fireEvent.click(screen.getByRole('button', { name: 'Show how to install Grok' }))
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Install Grok' })) })
    await waitFor(() => expect(screen.getByTestId('tool-status-grok').textContent).toBe('Version 0.9.3'))
    expect(within(toolRow('grok')).getByText('Installed')).toBeTruthy()
    expect(screen.getByTestId('tool-help-grok').textContent).toContain('Grok is installed. Turn its preset on to put it on the launch bar.')
  })
})
