// @vitest-environment jsdom
//
// Agents sidebar right-click → "Add to Home": the row sits directly under
// Pin/Unpin; one Home adds straight away with a toast; several Homes open
// the pick-a-Home dialog (Esc closes, Enter adds); an agent already on a
// Home is never added twice; and a remote server's agent keeps that
// server's host key in the row address.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({ label: 'main' }) }))
const av = vi.hoisted(() => ({ putHomeAvatarOnAdd: vi.fn() }))
vi.mock('@/lib/home-avatars', () => ({ putHomeAvatarOnAdd: av.putHomeAvatarOnAdd }))

import AddToHomeDialog from './AddToHomeDialog'
import { workspacePlacementMenuItems } from '@/components/Sidebar/workspace-menu-items'
import {
  ADD_TO_HOME_MENU_ID,
  ADD_TO_HOME_MENU_LABEL,
  useAddToHomeDialogStore,
} from '@/stores/add-to-home-dialog'
import { addAgentToHomeFromMenu, type ListedWorkspace } from '@/lib/add-to-home'
import { useHomesStore, type Home } from '@/stores/homes'
import { useToastStore } from '@/stores/toast'
import { activeHomeHostKey } from '@/lib/home-address'
import type { ConnectHost } from '@/stores/connect-host'

const dir = dirname(fileURLToPath(import.meta.url))
const sidebarSrc = readFileSync(join(dir, '../Sidebar/Sidebar.tsx'), 'utf8')
const panelsSrc = readFileSync(join(dir, '../Home/HomeAddPanels.tsx'), 'utf8')

const atlas: ListedWorkspace = { id: 'w1', name: 'Atlas', handle: 'atlas', path: '/srv/atlas', iconUrl: null }

function setHomes(homes: Home[], selectedId = homes[0].id): void {
  act(() => {
    useHomesStore.setState({ homes, selectedId })
  })
}

function rowsOf(id: string): string[] {
  const h = useHomesStore.getState().homes.find((x) => x.id === id)
  if (!h) throw new Error(`no Home ${id}`)
  return h.rows.map((r) => r.address)
}

function toasts(): Array<{ message: string; type: string }> {
  return useToastStore.getState().toasts.map((t) => ({ message: t.message, type: t.type }))
}

beforeEach(() => {
  av.putHomeAvatarOnAdd.mockReset()
  useToastStore.setState({ toasts: [] })
  useAddToHomeDialogStore.getState().close()
})

afterEach(() => {
  cleanup()
})

describe('sidebar menu row', () => {
  it('"Add to Home" sits directly beneath Pin/Unpin', () => {
    for (const pinned of [0, 1]) {
      const items = workspacePlacementMenuItems({ pinned, manuallyActive: 0 })
      const ids = items.map((i) => i.id)
      const pin = ids.indexOf('toggle-pin')
      expect(items[pin].label).toBe(pinned ? 'Unpin' : 'Pin to Top')
      expect(ids[pin + 1]).toBe(ADD_TO_HOME_MENU_ID)
      expect(items[pin + 1].label).toBe('Add to Home')
      expect(ids[pin + 2]).toBe('toggle-active')
    }
    expect(ADD_TO_HOME_MENU_LABEL).toBe('Add to Home')
  })

  it('the Agents sidebar uses that block and adds with the connected server key', () => {
    expect(sidebarSrc).toContain('...workspacePlacementMenuItems(project)')
    expect(sidebarSrc).not.toMatch(/\{ id: 'toggle-pin', label: project\.pinned/)
    expect(sidebarSrc).toContain('clickedId === ADD_TO_HOME_MENU_ID')
    expect(sidebarSrc).toContain('addAgentToHomeFromMenu(activeHomeHostKey(activeHost), project)')
  })

  it("Home's own picker adds through the same shared function (one way to add)", () => {
    expect(panelsSrc).toContain('const addTo = addWorkspaceToHome')
    expect(panelsSrc).not.toContain('useHomesStore.getState().addRow(')
  })
})

describe('one Home', () => {
  it('adds the agent straight away and toasts', () => {
    setHomes([{ id: 'h1', name: 'My Home', rows: [] }])
    const out = addAgentToHomeFromMenu('local', atlas)
    expect(out).toEqual({ kind: 'added', homeId: 'h1' })
    expect(rowsOf('h1')).toEqual(['atlas::local'])
    expect(useHomesStore.getState().homes[0].rows[0]).toEqual({
      address: 'atlas::local',
      workspaceId: 'w1',
      label: 'Atlas',
    })
    expect(av.putHomeAvatarOnAdd).toHaveBeenCalledWith('local', 'atlas::local', atlas)
    expect(useAddToHomeDialogStore.getState().isOpen).toBe(false)
    expect(toasts()).toEqual([{ message: 'Added to My Home', type: 'success' }])
  })

  it('never adds a duplicate: toasts "Already on" instead', () => {
    setHomes([{ id: 'h1', name: 'My Home', rows: [{ address: 'atlas::local', workspaceId: 'w1', label: 'Atlas' }] }])
    const out = addAgentToHomeFromMenu('local', atlas)
    expect(out).toEqual({ kind: 'already', homeId: 'h1' })
    expect(rowsOf('h1')).toEqual(['atlas::local'])
    expect(toasts()).toEqual([{ message: 'Already on My Home', type: 'info' }])
  })
})

describe('several Homes', () => {
  const two = (): Home[] => [
    { id: 'h1', name: 'My Home', rows: [] },
    { id: 'h2', name: 'Work', rows: [] },
  ]

  it('opens the dialog instead of adding', () => {
    setHomes(two(), 'h2')
    render(<AddToHomeDialog />)
    let out: ReturnType<typeof addAgentToHomeFromMenu> | null = null
    act(() => {
      out = addAgentToHomeFromMenu('local', atlas)
    })
    expect(out).toEqual({ kind: 'pick' })
    expect(rowsOf('h1')).toEqual([])
    expect(rowsOf('h2')).toEqual([])
    expect(screen.getByRole('dialog', { name: 'Add to Home' })).toBeTruthy()
    // This window's selected Home starts ticked.
    expect((screen.getByLabelText(/Work/) as HTMLInputElement).checked).toBe(true)
    expect((screen.getByLabelText(/My Home/) as HTMLInputElement).checked).toBe(false)
  })

  it('Enter adds to the ticked Homes and toasts', () => {
    setHomes(two(), 'h2')
    render(<AddToHomeDialog />)
    act(() => {
      addAgentToHomeFromMenu('local', atlas)
    })
    fireEvent.click(screen.getByLabelText(/My Home/))
    fireEvent.keyDown(window, { key: 'Enter' })
    expect(rowsOf('h1')).toEqual(['atlas::local'])
    expect(rowsOf('h2')).toEqual(['atlas::local'])
    expect(useAddToHomeDialogStore.getState().isOpen).toBe(false)
    expect(screen.queryByRole('dialog')).toBeNull()
    expect(toasts()).toEqual([{ message: 'Added to My Home and Work', type: 'success' }])
  })

  it('Esc closes without adding', () => {
    setHomes(two())
    render(<AddToHomeDialog />)
    act(() => {
      addAgentToHomeFromMenu('local', atlas)
    })
    fireEvent.keyDown(window, { key: 'Escape' })
    expect(useAddToHomeDialogStore.getState().isOpen).toBe(false)
    expect(rowsOf('h1')).toEqual([])
    expect(rowsOf('h2')).toEqual([])
    expect(toasts()).toEqual([])
  })

  it('shows a Home that already has the agent as checked and disabled, and never adds it twice', () => {
    setHomes(
      [
        { id: 'h1', name: 'My Home', rows: [{ address: 'atlas::local', workspaceId: 'w1', label: 'Atlas' }] },
        { id: 'h2', name: 'Work', rows: [] },
      ],
      'h1',
    )
    render(<AddToHomeDialog />)
    act(() => {
      addAgentToHomeFromMenu('local', atlas)
    })
    const mine = screen.getByLabelText(/My Home/) as HTMLInputElement
    expect(mine.checked).toBe(true)
    expect(mine.disabled).toBe(true)
    expect(screen.getByText('Already added')).toBeTruthy()
    // The selected Home already has it, so nothing starts ticked: Add is off.
    const add = screen.getByRole('button', { name: 'Add' }) as HTMLButtonElement
    expect(add.disabled).toBe(true)
    fireEvent.keyDown(window, { key: 'Enter' })
    expect(useAddToHomeDialogStore.getState().isOpen).toBe(true)
    fireEvent.click(screen.getByLabelText(/Work/))
    fireEvent.click(add)
    expect(rowsOf('h1')).toEqual(['atlas::local'])
    expect(rowsOf('h2')).toEqual(['atlas::local'])
    expect(toasts()).toEqual([{ message: 'Added to Work', type: 'success' }])
  })
})

describe('remote servers', () => {
  const remote: ConnectHost = {
    id: 'x',
    label: 'DTL',
    hostname: 'DTL.k2.dev',
    username: 'rosson',
    port: 443,
    secure: true,
    token: 't',
    remember: true,
    lastConnectedAt: null,
  }
  const lan: ConnectHost = { ...remote, id: 'y', hostname: '192.168.1.20', port: 38471, secure: false }

  it('one Home: the row keeps the remote host key', () => {
    setHomes([{ id: 'h1', name: 'Home', rows: [] }])
    addAgentToHomeFromMenu(activeHomeHostKey(remote), atlas)
    expect(rowsOf('h1')).toEqual(['atlas::dtl.k2.dev'])
    expect(av.putHomeAvatarOnAdd).toHaveBeenCalledWith('dtl.k2.dev', 'atlas::dtl.k2.dev', atlas)
  })

  it('the same handle on this computer and on a remote are two rows, not a duplicate', () => {
    setHomes([{ id: 'h1', name: 'Home', rows: [{ address: 'atlas::local', workspaceId: 'w1', label: 'Atlas' }] }])
    expect(addAgentToHomeFromMenu(activeHomeHostKey(lan), atlas)).toEqual({ kind: 'added', homeId: 'h1' })
    expect(rowsOf('h1')).toEqual(['atlas::local', 'atlas::192.168.1.20:38471'])
  })

  it('several Homes: the dialog adds with the key captured at the click', () => {
    setHomes([
      { id: 'h1', name: 'Home', rows: [] },
      { id: 'h2', name: 'Work', rows: [] },
    ])
    render(<AddToHomeDialog />)
    act(() => {
      addAgentToHomeFromMenu(activeHomeHostKey(lan), atlas)
    })
    expect(useAddToHomeDialogStore.getState().hostKey).toBe('192.168.1.20:38471')
    fireEvent.click(screen.getByRole('button', { name: 'Add' }))
    expect(rowsOf('h1')).toEqual(['atlas::192.168.1.20:38471'])
    expect(rowsOf('h2')).toEqual([])
  })
})
