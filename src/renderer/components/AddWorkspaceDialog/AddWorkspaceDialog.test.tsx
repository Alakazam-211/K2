// @vitest-environment jsdom
// Add Workspace confirm: the folder, three choices (seed the wiki, write
// cwd AGENTS.md, symlink leftover harness files; the first two on, the
// last off), and the add itself. A refused add shows its message and the
// dialog stays open; a reopened dialog starts from the defaults.

import { afterEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'

// Git on the server is its own banner and test (GitToolsNotice).
vi.mock('@/components/GitTools/GitToolsNotice', () => ({ GitToolsBanner: () => null }))

import AddWorkspaceDialog from './AddWorkspaceDialog'
import { useAddWorkspaceDialogStore } from '../../stores/add-workspace-dialog'

type Opts = { seedWiki: boolean; seedAgentsMd: boolean; fanout: boolean }

function open(onConfirm: (o: Opts) => Promise<void>): void {
  act(() => useAddWorkspaceDialogStore.getState().open({ path: '/Users/a/acme-site', onConfirm }))
}

function boxes(): boolean[] {
  return (screen.getAllByRole('checkbox') as HTMLInputElement[]).map((c) => c.checked)
}

afterEach(() => {
  cleanup()
  act(() => useAddWorkspaceDialogStore.getState().close())
})

describe('Add Workspace dialog', () => {
  it('renders nothing until opened', () => {
    const { container } = render(<AddWorkspaceDialog />)
    expect(container.innerHTML).toBe('')
  })

  it('shows the folder; Add sends the defaults (wiki and AGENTS.md on, fan-out off) and closes', async () => {
    const onConfirm = vi.fn(async (_o: Opts) => {})
    render(<AddWorkspaceDialog />)
    open(onConfirm)
    expect(screen.getByText('/Users/a/acme-site')).toBeTruthy()
    expect(boxes()).toEqual([true, true, false])
    fireEvent.click(screen.getByRole('button', { name: 'Add workspace' }))
    await waitFor(() => expect(useAddWorkspaceDialogStore.getState().isOpen).toBe(false))
    expect(onConfirm).toHaveBeenCalledWith({ seedWiki: true, seedAgentsMd: true, fanout: false })
  })

  it('sends what the boxes say', async () => {
    const onConfirm = vi.fn(async (_o: Opts) => {})
    render(<AddWorkspaceDialog />)
    open(onConfirm)
    for (const box of screen.getAllByRole('checkbox')) fireEvent.click(box)
    expect(boxes()).toEqual([false, false, true])
    fireEvent.click(screen.getByRole('button', { name: 'Add workspace' }))
    await waitFor(() => expect(onConfirm).toHaveBeenCalledWith({ seedWiki: false, seedAgentsMd: false, fanout: true }))
  })

  it('a refused add shows its message and stays open, ready to try again', async () => {
    const onConfirm = vi.fn(async (_o: Opts) => {
      throw new Error('That folder is already a workspace: Acme Site')
    })
    render(<AddWorkspaceDialog />)
    open(onConfirm)
    fireEvent.click(screen.getByRole('button', { name: 'Add workspace' }))
    await screen.findByText('That folder is already a workspace: Acme Site')
    expect(useAddWorkspaceDialogStore.getState().isOpen).toBe(true)
    const add = screen.getByRole('button', { name: 'Add workspace' }) as HTMLButtonElement
    expect(add.disabled).toBe(false)
  })

  it('Escape and Cancel close it without adding; reopening starts from the defaults', () => {
    const onConfirm = vi.fn(async (_o: Opts) => {})
    render(<AddWorkspaceDialog />)
    open(onConfirm)
    fireEvent.click(screen.getAllByRole('checkbox')[2])
    fireEvent.keyDown(window, { key: 'Escape' })
    expect(useAddWorkspaceDialogStore.getState().isOpen).toBe(false)
    open(onConfirm)
    expect(boxes()).toEqual([true, true, false])
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))
    expect(useAddWorkspaceDialogStore.getState().isOpen).toBe(false)
    expect(onConfirm).not.toHaveBeenCalled()
  })
})
