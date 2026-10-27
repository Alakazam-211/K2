// @vitest-environment jsdom
// "New agents open in" (0.46.0, Rosson): Workspaces / Agents → Workspace
// settings since 0.46.5 (General → Workspaces before), under the auto-select
// row. Split view adds two side pickers that never match.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => {}), listen: vi.fn(async () => () => {}) }))

import { WORKSPACE_SETTINGS_MANIFEST, WorkspaceBehaviorRows } from './WorkspaceSettingsPage'
import { useSettingsStore } from '@/stores/settings'
import {
  LS_NEW_AGENT_VIEW,
  NEW_AGENT_VIEW_DEFAULT,
  readNewAgentView,
} from '@/components/SessionView/sessionViewTab'

beforeEach(() => {
  localStorage.clear()
  useSettingsStore.setState({ newAgentView: NEW_AGENT_VIEW_DEFAULT })
})

afterEach(() => {
  cleanup()
})

async function pick(scope: HTMLElement, trigger: string, option: string): Promise<void> {
  fireEvent.click(within(scope).getByRole('button', { name: trigger }))
  const menu = screen.getByTestId('setting-dropdown-menu')
  await act(async () => {
    fireEvent.click(within(menu).getByRole('button', { name: option }))
  })
}

function row(): HTMLElement {
  const el = document.querySelector<HTMLElement>('[data-settings-id="general.new-agent-view"]')
  if (!el) throw new Error('New agents open in row not rendered')
  return el
}

describe('New agents open in', () => {
  it('renders right after the auto-select row', () => {
    render(<WorkspaceBehaviorRows />)
    const ids = Array.from(document.querySelectorAll('[data-settings-id]')).map((el) =>
      el.getAttribute('data-settings-id'),
    )
    const focus = ids.indexOf('general.workspace-switch-focus')
    expect(focus).toBeGreaterThanOrEqual(0)
    expect(ids[focus + 1]).toBe('general.new-agent-view')
    expect(within(row()).getByText('New agents open in')).toBeTruthy()
    expect(within(row()).getByRole('button', { name: 'New agents open in' }).textContent).toContain('Terminal')
  })

  it('is in the settings search manifest under Workspace settings', () => {
    const entry = WORKSPACE_SETTINGS_MANIFEST.find((e) => e.id === 'general.new-agent-view')
    expect(entry?.group).toBe('Workspace settings')
    expect(entry?.section).toBe('projects')
    expect(entry?.label).toBe('New agents open in')
    const order = WORKSPACE_SETTINGS_MANIFEST.map((e) => e.id)
    expect(order.indexOf('general.new-agent-view')).toBe(
      order.indexOf('general.workspace-switch-focus') + 1,
    )
  })

  it('the search text does not offer Chat as a view (Chat view deleted 0.46.5)', () => {
    const entry = WORKSPACE_SETTINGS_MANIFEST.find((e) => e.id === 'general.new-agent-view')
    if (!entry) throw new Error('general.new-agent-view is missing from WORKSPACE_SETTINGS_MANIFEST')
    expect(entry.description).not.toMatch(/\bchat\b/i)
    expect(entry.keywords).not.toContain('chat')
    render(<WorkspaceBehaviorRows />)
    expect(row().textContent).not.toMatch(/\bchat\b/i)
  })

  it('offers the four views (no Chat) and saves the choice', async () => {
    render(<WorkspaceBehaviorRows />)
    fireEvent.click(within(row()).getByRole('button', { name: 'New agents open in' }))
    const menu = screen.getByTestId('setting-dropdown-menu')
    const labels = within(menu).getAllByRole('button').map((b) => b.textContent?.trim())
    expect(labels).toEqual(['Terminal', 'Thread', 'Chatter', 'Split view'])
    await act(async () => {
      fireEvent.click(within(menu).getByRole('button', { name: 'Thread' }))
    })
    expect(useSettingsStore.getState().newAgentView.view).toBe('thread')
    expect(JSON.parse(localStorage.getItem(LS_NEW_AGENT_VIEW) ?? 'null')).toEqual({
      view: 'thread',
      left: 'terminal',
      right: 'thread',
    })
    expect(readNewAgentView().view).toBe('thread')
  })

  it('shows the side pickers only for Split view, with different sides', async () => {
    render(<WorkspaceBehaviorRows />)
    expect(screen.queryByTestId('new-agent-view-split')).toBeNull()
    expect(screen.queryByRole('button', { name: 'Split view left side' })).toBeNull()

    await pick(row(), 'New agents open in', 'Split view')
    const split = screen.getByTestId('new-agent-view-split')
    expect(within(split).getByRole('button', { name: 'Split view left side' }).textContent).toContain('Terminal')
    expect(within(split).getByRole('button', { name: 'Split view right side' }).textContent).toContain('Thread')

    // The right side's current view cannot be picked on the left.
    fireEvent.click(within(split).getByRole('button', { name: 'Split view left side' }))
    const menu = screen.getByTestId('setting-dropdown-menu')
    const labels = within(menu).getAllByRole('button').map((b) => b.textContent?.trim())
    expect(labels).toEqual(['Terminal', 'Thread', 'Chatter'])
    const thread = within(menu).getByRole('button', { name: 'Thread' }) as HTMLButtonElement
    expect(thread.disabled).toBe(true)
    await act(async () => {
      fireEvent.click(within(menu).getByRole('button', { name: 'Chatter' }))
    })
    await pick(split, 'Split view right side', 'Terminal')

    expect(useSettingsStore.getState().newAgentView).toEqual({
      view: 'split',
      split: { left: 'chatter', right: 'terminal' },
    })
    expect(readNewAgentView()).toEqual({
      view: 'split',
      split: { left: 'chatter', right: 'terminal' },
    })

    await pick(row(), 'New agents open in', 'Chatter')
    expect(screen.queryByTestId('new-agent-view-split')).toBeNull()
  })

  it('a stored pref with equal sides is repaired, never kept equal', () => {
    localStorage.setItem(LS_NEW_AGENT_VIEW, JSON.stringify({ view: 'split', left: 'thread', right: 'thread' }))
    expect(readNewAgentView()).toEqual({ view: 'split', split: { left: 'thread', right: 'terminal' } })
  })

  it('a saved Chat pick (the deleted Chat view) shows and saves as Thread', () => {
    localStorage.setItem(LS_NEW_AGENT_VIEW, JSON.stringify({ view: 'chat', left: 'terminal', right: 'thread' }))
    useSettingsStore.setState({ newAgentView: readNewAgentView() })
    expect(JSON.parse(localStorage.getItem(LS_NEW_AGENT_VIEW) ?? 'null')).toEqual({
      view: 'thread',
      left: 'terminal',
      right: 'thread',
    })
    render(<WorkspaceBehaviorRows />)
    expect(within(row()).getByRole('button', { name: 'New agents open in' }).textContent).toContain('Thread')
  })
})
