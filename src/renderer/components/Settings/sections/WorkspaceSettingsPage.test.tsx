// @vitest-environment jsdom
// Workspace settings moved from Settings → General → Workspaces to
// Settings → Workspaces / Agents in 0.46.5 (Rosson 2026-10-27): "a page that
// you can visit on the Workspace / agents page by putting it above the focus
// groups toggle". The entry is at the very top of the list; its page holds
// the same rows (same stores, same daemon writes) and the Canonical Agent
// Flow guide; search and the `agent-skills` deep link land there; General
// no longer has a Workspaces tab.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { readFileSync } from 'node:fs'
import { join } from 'node:path'

const settingsGet = vi.hoisted(() => vi.fn())
const settingsUpdate = vi.hoisted(() => vi.fn())
const settingsReset = vi.hoisted(() => vi.fn())
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => {}), listen: vi.fn(async () => () => {}) }))
vi.mock('@/lib/daemon-settings', () => ({ settingsGet, settingsUpdate, settingsReset }))
vi.mock('@/components/Presence/PresenceKickButton', () => ({ fetchViewerRole: vi.fn(async () => 'owner') }))

import { ProjectsSection } from './ProjectsSection'
import { GENERAL_MANIFEST } from './GeneralSection'
import { AGENT_SKILLS_MANIFEST } from './AgentSkillsSection'
import {
  WORKSPACE_SETTINGS_GROUP,
  WORKSPACE_SETTINGS_MANIFEST,
  WorkspaceSettingsPage,
} from './WorkspaceSettingsPage'
import { searchManifest, searchPickPage, sectionLabel } from '../searchManifest'
import { useSettingsStore } from '@/stores/settings'
import { useProjectsStore } from '@/stores/projects'
import { useFocusGroupsStore } from '@/stores/focus-groups'
import { LS_NEW_AGENT_VIEW, NEW_AGENT_VIEW_DEFAULT } from '@/components/SessionView/sessionViewTab'

/** The rows that used to be General → Workspaces, by search id. */
const MOVED_ROW_IDS = [
  'general.active-window-hours',
  'general.completion-sound',
  'general.completion-sound-choice',
  'general.completion-sound-volume',
  'general.workspace-switch-focus',
  'general.new-agent-view',
  'general.remote-rooms-preview',
]

beforeEach(() => {
  localStorage.clear()
  settingsGet.mockReset()
  settingsUpdate.mockReset()
  settingsUpdate.mockImplementation(async (updates: Record<string, unknown>) => {
    const s = useSettingsStore.getState()
    return { terminal: s.terminal, keybindings: s.keybindings, completionSoundEnabled: true, ...updates }
  })
  useSettingsStore.setState({
    projectsPage: null,
    generalSubTab: null,
    initialProjectId: null,
    completionSoundEnabled: true,
    newAgentView: NEW_AGENT_VIEW_DEFAULT,
  })
  useProjectsStore.setState({ projects: [] } as never)
  useFocusGroupsStore.setState({ focusGroups: [], focusGroupsEnabled: false } as never)
})

afterEach(() => {
  cleanup()
})

function entry(): HTMLElement {
  return screen.getByTestId('workspace-settings-entry')
}

/** True when `a` comes before `b` in document order. */
function before(a: Node, b: Node): boolean {
  return (a.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_FOLLOWING) !== 0
}

async function pick(scope: HTMLElement, trigger: string, option: string): Promise<void> {
  fireEvent.click(within(scope).getByRole('button', { name: trigger }))
  const menu = screen.getByTestId('setting-dropdown-menu')
  await act(async () => {
    fireEvent.click(within(menu).getByRole('button', { name: option }))
  })
}

describe('Workspaces / Agents: the Workspace settings entry', () => {
  it('is at the very top of the list, above the Focus Groups toggle', () => {
    render(<ProjectsSection />)
    const e = entry()
    expect(e.textContent).toContain('Workspace settings')
    const focusLabel = screen.getByText('Focus Groups')
    expect(before(e, focusLabel)).toBe(true)
    // Nothing interactive sits above it in the left panel.
    const panel = e.closest('.w-60')
    if (!panel) throw new Error('left panel not found')
    const firstButton = panel.querySelector('button')
    expect(firstButton).toBe(e)
    // Not open until picked.
    expect(screen.queryByTestId('workspace-settings-page')).toBeNull()
    expect(e.getAttribute('aria-current')).toBeNull()
  })

  it('opens a page with every moved row and the Canonical Agent Flow guide', () => {
    render(<ProjectsSection />)
    fireEvent.click(entry())
    const page = screen.getByTestId('workspace-settings-page')
    expect(entry().getAttribute('aria-current')).toBe('page')
    expect(within(page).getByRole('heading', { name: 'Workspace settings' })).toBeTruthy()
    const ids = Array.from(page.querySelectorAll('[data-settings-id]')).map((el) => el.getAttribute('data-settings-id'))
    for (const id of MOVED_ROW_IDS) expect(ids).toContain(id)
    expect(ids).toContain('agent-skills.canonical-flow')
    expect(screen.queryByText('Select a workspace to view its settings')).toBeNull()
  })

  it('edits a local setting there: New agents open in', async () => {
    render(<ProjectsSection />)
    fireEvent.click(entry())
    const row = screen.getByTestId('workspace-settings-page').querySelector<HTMLElement>(
      '[data-settings-id="general.new-agent-view"]',
    )
    if (!row) throw new Error('New agents open in row missing from the page')
    await pick(row, 'New agents open in', 'Thread')
    expect(useSettingsStore.getState().newAgentView.view).toBe('thread')
    expect(JSON.parse(localStorage.getItem(LS_NEW_AGENT_VIEW) ?? 'null')).toEqual({
      view: 'thread',
      left: 'terminal',
      right: 'thread',
    })
  })

  it('edits a daemon setting there with the same write as before: the completion sound switch', async () => {
    render(<WorkspaceSettingsPage />)
    const sw = screen.getByRole('switch', { name: 'Play a sound when an agent finishes unwatched' })
    expect(sw.getAttribute('aria-checked')).toBe('true')
    fireEvent.click(sw)
    await waitFor(() => expect(settingsUpdate).toHaveBeenCalledWith({ completionSoundEnabled: false }))
    expect(settingsUpdate).toHaveBeenCalledTimes(1)
    expect(useSettingsStore.getState().completionSoundEnabled).toBe(false)
  })

  it('picking a workspace leaves the page; the entry brings it back', () => {
    useProjectsStore.setState({
      projects: [{ id: 'p1', name: 'ops', path: '/w/ops', workspaces: [], pinned: 0 }],
    } as never)
    render(<ProjectsSection />)
    fireEvent.click(entry())
    expect(screen.getByTestId('workspace-settings-page')).toBeTruthy()
    const row = document.querySelector<HTMLElement>('[data-settings-project-id="p1"]')
    if (!row) throw new Error('workspace row not rendered')
    fireEvent.click(row)
    expect(screen.queryByTestId('workspace-settings-page')).toBeNull()
    expect(entry().getAttribute('aria-current')).toBeNull()
    fireEvent.click(entry())
    expect(screen.getByTestId('workspace-settings-page')).toBeTruthy()
  })

  it('a one-shot projectsPage (deep link or search) opens the page and is cleared', async () => {
    render(<ProjectsSection />)
    expect(screen.queryByTestId('workspace-settings-page')).toBeNull()
    act(() => useSettingsStore.setState({ projectsPage: 'workspace-settings' }))
    await waitFor(() => expect(screen.getByTestId('workspace-settings-page')).toBeTruthy())
    expect(useSettingsStore.getState().projectsPage).toBeNull()
  })
})

describe('search and deep links find Workspace settings in the new place', () => {
  const all = [...GENERAL_MANIFEST, ...WORKSPACE_SETTINGS_MANIFEST, ...AGENT_SKILLS_MANIFEST]

  it('every moved row is a Workspaces / Agents → Workspace settings entry, not a General one', () => {
    for (const id of MOVED_ROW_IDS) {
      const e = WORKSPACE_SETTINGS_MANIFEST.find((x) => x.id === id)
      if (!e) throw new Error(`${id} missing from WORKSPACE_SETTINGS_MANIFEST`)
      expect([e.section, e.group]).toEqual(['projects', WORKSPACE_SETTINGS_GROUP])
      expect(GENERAL_MANIFEST.find((x) => x.id === id)).toBeUndefined()
    }
    expect(GENERAL_MANIFEST.filter((e) => e.group === 'Workspaces')).toEqual([])
    for (const e of AGENT_SKILLS_MANIFEST) expect([e.section, e.group]).toEqual(['projects', WORKSPACE_SETTINGS_GROUP])
    expect(sectionLabel('projects')).toBe('Workspaces / Agents')
  })

  it('searching finds the page and its rows, and a pick opens the page', () => {
    const page = searchManifest(all, 'workspace settings')[0]
    expect(page?.id).toBe('projects.workspace-settings')
    const newAgents = searchManifest(all, 'new agents open in')[0]
    expect(newAgents?.id).toBe('general.new-agent-view')
    const sound = searchManifest(all, 'completion sound volume')[0]
    expect(sound?.id).toBe('general.completion-sound-volume')
    const flow = searchManifest(all, 'canonical agent flow')[0]
    expect(flow?.id).toBe('agent-skills.canonical-flow')
    for (const e of [page, newAgents, sound, flow]) {
      if (!e) throw new Error('search returned nothing')
      expect(e.section).toBe('projects')
      expect(searchPickPage(e)).toEqual({ projectsPage: 'workspace-settings' })
    }
    // General rows still open their own tab.
    expect(searchPickPage({ section: 'general', group: 'Server' })).toEqual({ generalSubTab: 'server' })
    expect(searchPickPage({ section: 'general', group: 'Workspaces' })).toEqual({})
  })

  it("openSettings('agent-skills') lands on Workspaces / Agents → Workspace settings", () => {
    useSettingsStore.getState().openSettings('agent-skills')
    const s = useSettingsStore.getState()
    expect(s.activeSection).toBe('projects')
    expect(s.projectsPage).toBe('workspace-settings')
    expect(s.generalSubTab).toBeNull()
    useSettingsStore.getState().closeSettings()
  })

  it('General has no Workspaces tab and no Workspaces search mapping', () => {
    const dir = __dirname
    const general = readFileSync(join(dir, 'GeneralSection.tsx'), 'utf8')
    const search = readFileSync(join(dir, '..', 'searchManifest.ts'), 'utf8')
    const store = readFileSync(join(dir, '..', '..', '..', 'stores', 'settings.ts'), 'utf8')
    expect(general).not.toMatch(/id: 'workspaces'/)
    expect(general).not.toMatch(/tab === 'workspaces'/)
    expect(general).not.toMatch(/WorkspaceBehaviorRows|AgentSkillsSection/)
    expect(search).not.toMatch(/Workspaces: 'workspaces'/)
    expect(store).not.toMatch(/'workspaces'/)
  })
})
