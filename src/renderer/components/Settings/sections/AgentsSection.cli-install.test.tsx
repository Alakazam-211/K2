// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { AgentsSection, injectFlowSummary } from '@/components/Settings/sections/AgentsSection'
import { CLI_INSTALL_COMMANDS, useCliInstallStore } from '@/lib/ensure-cli'
import * as daemonCli from '@/lib/daemon-cli'
import { usePresetsStore, type AgentPreset } from '@/stores/presets'
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

const preset = (id: string, label: string, command: string, extra: Partial<AgentPreset> = {}): AgentPreset => ({
  id, label, command, icon: '', enabled: 1, sortOrder: 0, isBuiltIn: 1, createdAt: 1, inject_flow: null, ...extra,
})
const SAMPLE = [
  preset('b1', 'Claude', 'claude --dangerously-skip-permissions'),
  preset('b2', 'Codex', 'codex --yolo'),
  preset('c1', 'Claude review', 'claude --permission-mode plan', { isBuiltIn: 0 }),
]

type CliReply = { tools: { program: string; installed: boolean; version?: string | null }[] } | Error | 'hang'
function mockCliStatus(reply: CliReply): void {
  vi.spyOn(daemonCli, 'daemonCliGet').mockImplementation((async (_scope: unknown, route: string) => {
    if (route !== 'agents/cli-status') throw new Error(`unexpected GET ${route}`)
    if (reply === 'hang') return new Promise(() => {})
    if (reply instanceof Error) throw reply
    return reply
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
  mockCliStatus({ tools: [{ program: 'claude', installed: true, version: '2.1.12' }, { program: 'codex', installed: true, version: null }] })
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
})

describe('Settings → Language Models → Providers / Models page', () => {
  it('is one page: a header, then Default agent, Launch presets and Agent tools card groups', async () => {
    render(<AgentsSection />)
    expect(screen.getByRole('heading', { level: 2, name: 'Providers / Models' })).toBeTruthy()
    const headings = screen.getAllByRole('heading', { level: 3 }).map((h) => h.textContent)
    expect(headings).toEqual(['Default agent', 'Launch presets', 'Agent tools on this computer'])
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

  it('sums up the message keys in one line', () => {
    expect(injectFlowSummary([{ key: 'paste', waitMs: 0 }, { key: 'return', waitMs: 150 }])).toBe('Paste message, then Return')
  })
})

describe('Agent tools on this computer', () => {
  it('lists installed tools first with their version, the rest folded under More agents', async () => {
    render(<AgentsSection />)
    await screen.findByTestId('tool-row-claude')
    expect(toolRow('claude').textContent).toContain('Version 2.1.12')
    expect(toolRow('codex').textContent).toContain('Version unknown')
    expect(screen.queryByTestId('tool-row-grok')).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: /More agents you can install/ }))
    expect(toolRow('grok').textContent).toContain('Not installed. K2 installs it the first time you start it.')
    expect(toolRow('aider').textContent).toContain('Not installed')
  })

  it('shows the four installers K2 runs, and Cursor’s own installer', async () => {
    mockCliStatus({ tools: [] })
    render(<AgentsSection />)
    await screen.findByTestId('tools-none')
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

    fireEvent.click(screen.getByRole('button', { name: 'Show how to install GitHub Copilot CLI' }))
    expect(within(screen.getByTestId('tool-help-copilot')).getByText('npm install -g @github/copilot')).toBeTruthy()
  })

  it('shows Installing Grok… on that row only while Grok is installing', async () => {
    mockCliStatus({ tools: [] })
    useCliInstallStore.setState({ installing: 'grok' })
    render(<AgentsSection />)
    await screen.findByTestId('tools-none')
    fireEvent.click(screen.getByRole('button', { name: 'Show how to install Grok' }))
    expect(screen.getByRole('button', { name: 'Installing Grok…' })).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Show how to install OpenAI Codex' }))
    expect(screen.getByRole('button', { name: 'Install OpenAI Codex' })).toBeTruthy()
  })

  it('says when the check fails, and offers Try again', async () => {
    mockCliStatus(new Error('timed out'))
    render(<AgentsSection />)
    const err = await screen.findByTestId('tools-error')
    expect(err.textContent).toContain('Couldn’t check which tools are installed')
    mockCliStatus({ tools: [{ program: 'claude', installed: true, version: '2.1.12' }] })
    await act(async () => { fireEvent.click(within(err).getByRole('button', { name: 'Try again' })) })
    expect((await screen.findByTestId('tool-row-claude')).textContent).toContain('Version 2.1.12')
  })

  it('says plainly when this server can’t check, instead of guessing', async () => {
    mockCliStatus(new Error('route_unclassified'))
    render(<AgentsSection />)
    expect((await screen.findByTestId('tools-unsupported')).textContent).toContain('can’t check which tools are installed')
    expect(screen.queryByText(/^Installed$/)).toBeNull()
  })

  it('says Checking… while the check runs', () => {
    mockCliStatus('hang')
    render(<AgentsSection />)
    expect(screen.getByTestId('tools-loading').textContent).toContain('Checking which agent tools are on this computer')
  })
})
