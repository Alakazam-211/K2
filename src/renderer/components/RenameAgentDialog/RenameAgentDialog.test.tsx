// @vitest-environment jsdom
//
// Sidebar right-click → "Rename agent…": the menu row (no "coming soon"
// stub any more) and the dialog's save / cancel flow. The save goes through
// the projects store's `renameAgentDisplayName` (display name only; the
// handle is shown read-only and never written from here).

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { render, screen, fireEvent, cleanup, act, waitFor } from '@testing-library/react'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const h = vi.hoisted(() => ({
  renameAgentDisplayName: vi.fn(),
  openSettings: vi.fn(),
  agentHandle: vi.fn(),
}))

vi.mock('@/stores/projects', () => ({
  useProjectsStore: {
    getState: () => ({ renameAgentDisplayName: h.renameAgentDisplayName }),
  },
}))
vi.mock('@/stores/settings', () => ({
  useSettingsStore: { getState: () => ({ openSettings: h.openSettings }) },
}))
vi.mock('@/lib/workspace-agent', () => ({
  agentHandle: (...a: unknown[]) => h.agentHandle(...a),
}))

import RenameAgentDialog from './RenameAgentDialog'
import {
  RENAME_AGENT_MENU_ID,
  RENAME_AGENT_MENU_LABEL,
  RENAME_AGENT_ROUTE,
  renameAgentMenuItem,
  useRenameAgentDialogStore,
  validateAgentDisplayName,
} from '@/stores/rename-agent-dialog'
import { primaryScope, type ServerScope } from '@/kessel/server-scope'

const dir = dirname(fileURLToPath(import.meta.url))
const sidebarSrc = readFileSync(join(dir, '../Sidebar/Sidebar.tsx'), 'utf8')

function openFor(over: Partial<{ currentName: string; handle: string }> = {}): void {
  act(() => {
    useRenameAgentDialogStore.getState().open({
      projectId: 'p1',
      projectPath: '/ws/k2',
      currentName: over.currentName ?? 'K2',
      handle: over.handle ?? 'k2',
    })
  })
}

function input(): HTMLInputElement {
  return screen.getByLabelText('Agent Name') as HTMLInputElement
}

beforeEach(() => {
  h.renameAgentDisplayName.mockReset()
  h.openSettings.mockReset()
  h.agentHandle.mockReset()
  useRenameAgentDialogStore.getState().close()
})

afterEach(() => {
  cleanup()
})

describe('sidebar menu row', () => {
  it('is a working "Rename agent…" row — the "coming soon" stub is gone', () => {
    const item = renameAgentMenuItem(primaryScope())
    expect(item).toEqual({ id: 'rename', label: 'Rename agent…', enabled: true })
    expect(item.badge).toBeUndefined()
    expect(RENAME_AGENT_MENU_ID).toBe('rename')
    expect(RENAME_AGENT_MENU_LABEL).toBe('Rename agent…')
    expect(RENAME_AGENT_ROUTE).toBe('workspace/set-agent-display-name')

    expect(sidebarSrc.toLowerCase()).not.toContain('coming soon')
    expect(sidebarSrc).toContain('renameAgentMenuItem(primaryScope())')
    expect(sidebarSrc).toContain('clickedId === RENAME_AGENT_MENU_ID')
    expect(sidebarSrc).toContain('useRenameAgentDialogStore.getState().open(')
    // Exactly one rename row in the workspace menu.
    expect(sidebarSrc.match(/renameAgentMenuItem\(/g)).toHaveLength(1)
  })

  it('is disabled (never badged) when the server would refuse the write', () => {
    const viewOnly = { ...primaryScope(), viewOnly: true } as unknown as ServerScope
    const item = renameAgentMenuItem(viewOnly)
    expect(item).toEqual({ id: 'rename', label: 'Rename agent…', enabled: false })
  })
})

describe('validateAgentDisplayName', () => {
  it('trims, refuses empty, and mirrors the daemon rules', () => {
    expect(validateAgentDisplayName('  Sales Team  ')).toEqual({ name: 'Sales Team' })
    expect(validateAgentDisplayName('   ')).toEqual({ error: 'Name must not be empty.' })
    expect(validateAgentDisplayName('a/b')).toEqual({ error: "Name must not contain '/'." })
    expect(validateAgentDisplayName('a::host')).toEqual({
      error: "Name must not contain ':' (addresses use handle::host).",
    })
    expect(validateAgentDisplayName('x'.repeat(65))).toEqual({
      error: 'Name must be at most 64 characters.',
    })
  })
})

describe('RenameAgentDialog', () => {
  it('prefills the name and shows the handle read-only with the settings pointer', () => {
    render(<RenameAgentDialog />)
    openFor()
    expect(input().value).toBe('K2')
    expect(screen.getByTestId('rename-agent-handle').textContent).toBe(
      'Handle stays @k2; change it in workspace settings.',
    )
    expect(h.agentHandle).not.toHaveBeenCalled()
  })

  it('Enter saves the trimmed display name and closes', async () => {
    h.renameAgentDisplayName.mockResolvedValueOnce(undefined)
    render(<RenameAgentDialog />)
    openFor()
    fireEvent.change(input(), { target: { value: '  Sales Team  ' } })
    fireEvent.keyDown(input(), { key: 'Enter' })
    await waitFor(() => expect(useRenameAgentDialogStore.getState().isOpen).toBe(false))
    expect(h.renameAgentDisplayName).toHaveBeenCalledTimes(1)
    expect(h.renameAgentDisplayName).toHaveBeenCalledWith('p1', 'Sales Team')
    expect(screen.queryByRole('dialog')).toBeNull()
  })

  it('Esc cancels without saving', () => {
    render(<RenameAgentDialog />)
    openFor()
    fireEvent.change(input(), { target: { value: 'Something Else' } })
    fireEvent.keyDown(input(), { key: 'Escape' })
    expect(useRenameAgentDialogStore.getState().isOpen).toBe(false)
    expect(h.renameAgentDisplayName).not.toHaveBeenCalled()
  })

  it('Cancel button closes without saving', () => {
    render(<RenameAgentDialog />)
    openFor()
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))
    expect(useRenameAgentDialogStore.getState().isOpen).toBe(false)
    expect(h.renameAgentDisplayName).not.toHaveBeenCalled()
  })

  it('empty (whitespace) is not allowed: Save disabled, Enter shows the error and stays open', () => {
    render(<RenameAgentDialog />)
    openFor()
    fireEvent.change(input(), { target: { value: '   ' } })
    expect((screen.getByRole('button', { name: 'Save' }) as HTMLButtonElement).disabled).toBe(true)
    fireEvent.keyDown(input(), { key: 'Enter' })
    expect(screen.getByText('Name must not be empty.')).toBeTruthy()
    expect(useRenameAgentDialogStore.getState().isOpen).toBe(true)
    expect(h.renameAgentDisplayName).not.toHaveBeenCalled()
  })

  it('an unchanged name closes without a daemon write', () => {
    render(<RenameAgentDialog />)
    openFor()
    fireEvent.change(input(), { target: { value: ' K2 ' } })
    fireEvent.keyDown(input(), { key: 'Enter' })
    expect(useRenameAgentDialogStore.getState().isOpen).toBe(false)
    expect(h.renameAgentDisplayName).not.toHaveBeenCalled()
  })

  it('a daemon refusal shows the error and keeps the dialog open', async () => {
    h.renameAgentDisplayName.mockRejectedValueOnce(new Error('role_required'))
    render(<RenameAgentDialog />)
    openFor()
    fireEvent.change(input(), { target: { value: 'New Name' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save' }))
    await waitFor(() => expect(screen.getByText('role_required')).toBeTruthy())
    expect(useRenameAgentDialogStore.getState().isOpen).toBe(true)
    expect(input().disabled).toBe(false)
  })

  it('reads the handle from the daemon when the row has none yet', async () => {
    h.agentHandle.mockResolvedValueOnce('sales-team')
    render(<RenameAgentDialog />)
    openFor({ handle: '' })
    await waitFor(() =>
      expect(screen.getByTestId('rename-agent-handle').textContent).toBe(
        'Handle stays @sales-team; change it in workspace settings.',
      ),
    )
    expect(h.agentHandle).toHaveBeenCalledWith(primaryScope(), '/ws/k2')
  })

  it('"workspace settings" closes and opens that workspace’s settings', () => {
    render(<RenameAgentDialog />)
    openFor()
    fireEvent.click(screen.getByRole('button', { name: 'workspace settings' }))
    expect(useRenameAgentDialogStore.getState().isOpen).toBe(false)
    expect(h.openSettings).toHaveBeenCalledWith('projects', 'p1')
  })
})
