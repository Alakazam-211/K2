// @vitest-environment jsdom

// 0.45.7 Settings → K2 Connect → URLs: Add published subdomain + per-row
// Change target / Remove, shown only to the local server's owner/admin.

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { render, screen, fireEvent, cleanup, waitFor, within } from '@testing-library/react'

const h = vi.hoisted(() => ({
  activeHost: 'local' as unknown,
  confirmResult: true,
  confirm: vi.fn(),
  api: {
    create: vi.fn(),
    updateTarget: vi.fn(),
    remove: vi.fn(),
  },
  tunnel: {
    status: {
      running: true,
      public_url: 'https://rosson.k2.dev',
      subdomain: 'rosson',
      local_port: 5000,
      server_addr: 'relay',
      frpc_installed: true,
    },
    subs: {
      primary: 'rosson',
      targets: {
        staging: { target: 'localhost:3000', projectId: 'p1' },
        docs: { target: 'localhost:4000', projectId: null },
      },
    },
  } as { status: unknown; subs: unknown },
}))

vi.mock('@/kessel/server-scope', () => ({
  primaryScope: () => ({ connectionKey: 'local', isRemote: false }),
}))
vi.mock('@/hooks/useTunnelUrls', () => ({ useTunnelUrls: () => h.tunnel }))
vi.mock('@/stores/projects', () => ({
  useProjectsStore: (sel: (s: unknown) => unknown) =>
    sel({ projects: [{ id: 'p1', name: 'Sales' }, { id: 'p2', name: 'Docs' }] }),
}))
vi.mock('@/stores/connect-host', () => ({
  useConnectHostStore: (sel: (s: unknown) => unknown) => sel({ activeHost: h.activeHost }),
}))
vi.mock('@/stores/confirm-dialog', () => ({
  useConfirmDialogStore: (sel: (s: unknown) => unknown) => sel({ confirm: h.confirm }),
}))
vi.mock('@/lib/published-subdomains', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@/lib/published-subdomains')>()
  return { ...actual, publishedSubdomains: () => h.api }
})

import { TunnelUrlsPanel } from './TunnelUrlsPanel'

beforeEach(() => {
  h.activeHost = 'local'
  h.confirm.mockReset()
  h.confirm.mockImplementation(async () => h.confirmResult)
  h.confirmResult = true
  h.api.create.mockReset()
  h.api.updateTarget.mockReset()
  h.api.remove.mockReset()
})

afterEach(() => cleanup())

function openAddForm(): HTMLElement {
  fireEvent.click(screen.getByRole('button', { name: 'Add published subdomain' }))
  return screen.getByRole('form', { name: 'Add published subdomain' })
}

describe('TunnelUrlsPanel — who sees the controls', () => {
  it('a remote host shows no add / change / remove, even with canManage', () => {
    h.activeHost = { id: 'r1', label: 'iascm', hostname: 'iascm.k2.dev' }
    render(<TunnelUrlsPanel canManage />)
    expect(screen.getByText('localhost:3000')).toBeTruthy()
    expect(screen.queryByRole('button', { name: 'Add published subdomain' })).toBeNull()
    expect(screen.queryByRole('button', { name: 'Change target' })).toBeNull()
    expect(screen.queryByRole('button', { name: 'Remove' })).toBeNull()
  })

  it('without canManage (member) the panel is read-only', () => {
    render(<TunnelUrlsPanel />)
    expect(screen.queryByRole('button', { name: 'Add published subdomain' })).toBeNull()
    expect(screen.queryByRole('button', { name: 'Remove' })).toBeNull()
  })

  it('the local owner/admin gets add plus per-row actions', () => {
    render(<TunnelUrlsPanel canManage />)
    expect(screen.getByRole('button', { name: 'Add published subdomain' })).toBeTruthy()
    expect(screen.getAllByRole('button', { name: 'Change target' })).toHaveLength(2)
    expect(screen.getAllByRole('button', { name: 'Remove' })).toHaveLength(2)
  })
})

describe('TunnelUrlsPanel — add', () => {
  it('previews the full host and validates the label as you type', () => {
    render(<TunnelUrlsPanel canManage />)
    const form = openAddForm()
    expect(within(form).getByTestId('subdomain-preview').textContent).toBe('<label>.rosson.k2.dev')
    fireEvent.change(within(form).getByPlaceholderText('staging'), { target: { value: 'my-app' } })
    expect(within(form).getByTestId('subdomain-preview').textContent).toBe('my-app.rosson.k2.dev')
    fireEvent.change(within(form).getByPlaceholderText('staging'), { target: { value: 'mail' } })
    expect(within(form).getByText(/'mail' is reserved/)).toBeTruthy()
    const add = within(form).getByRole('button', { name: 'Add' }) as HTMLButtonElement
    expect(add.disabled).toBe(true)
  })

  it('the target field starts empty', () => {
    render(<TunnelUrlsPanel canManage />)
    const form = openAddForm()
    const target = within(form).getByPlaceholderText('localhost:<port>') as HTMLInputElement
    expect(target.value).toBe('')
  })

  it('submits label + target with no workspace by default', async () => {
    h.api.create.mockResolvedValueOnce({ refreshed: true, warning: null })
    render(<TunnelUrlsPanel canManage />)
    const form = openAddForm()
    fireEvent.change(within(form).getByPlaceholderText('staging'), { target: { value: 'preview' } })
    fireEvent.change(within(form).getByPlaceholderText('localhost:<port>'), {
      target: { value: 'localhost:5173' },
    })
    fireEvent.click(within(form).getByRole('button', { name: 'Add' }))
    await waitFor(() => expect(h.api.create).toHaveBeenCalledTimes(1))
    expect(h.api.create).toHaveBeenCalledWith('preview', 'localhost:5173', { projectId: null })
    await waitFor(() => expect(screen.getByText('Added preview.rosson.k2.dev.')).toBeTruthy())
  })

  it('shows the mapped control-plane error and keeps the form open', async () => {
    h.api.create.mockRejectedValueOnce(new Error('That label is already taken.'))
    render(<TunnelUrlsPanel canManage />)
    const form = openAddForm()
    fireEvent.change(within(form).getByPlaceholderText('staging'), { target: { value: 'preview' } })
    fireEvent.change(within(form).getByPlaceholderText('localhost:<port>'), {
      target: { value: 'localhost:5173' },
    })
    fireEvent.click(within(form).getByRole('button', { name: 'Add' }))
    await waitFor(() => expect(screen.getByRole('alert').textContent).toBe('That label is already taken.'))
    expect(screen.getByRole('form', { name: 'Add published subdomain' })).toBeTruthy()
  })
})

describe('TunnelUrlsPanel — per row', () => {
  it('Change target sends the new target with the row owner', async () => {
    h.api.updateTarget.mockResolvedValueOnce({ refreshed: true, warning: null })
    render(<TunnelUrlsPanel canManage />)
    // Rows are sorted by label: docs, staging.
    fireEvent.click(screen.getAllByRole('button', { name: 'Change target' })[1])
    const form = screen.getByRole('form', { name: 'Change target for staging' })
    fireEvent.change(within(form).getByLabelText('New target'), { target: { value: 'localhost:3001' } })
    fireEvent.click(within(form).getByRole('button', { name: 'Save' }))
    await waitFor(() => expect(h.api.updateTarget).toHaveBeenCalledTimes(1))
    expect(h.api.updateTarget).toHaveBeenCalledWith('staging', 'localhost:3001', { projectId: 'p1' })
    await waitFor(() => expect(screen.getByText('Now points to localhost:3001.')).toBeTruthy())
  })

  it('Remove asks first, then removes with the row owner', async () => {
    h.api.remove.mockResolvedValueOnce({ refreshed: false, warning: null })
    render(<TunnelUrlsPanel canManage />)
    fireEvent.click(screen.getAllByRole('button', { name: 'Remove' })[1])
    await waitFor(() => expect(h.api.remove).toHaveBeenCalledTimes(1))
    expect(h.confirm).toHaveBeenCalledTimes(1)
    const opts = h.confirm.mock.calls[0][0] as { message: string; destructive: boolean }
    expect(opts.message).toContain('staging.rosson.k2.dev')
    expect(opts.destructive).toBe(true)
    expect(h.api.remove).toHaveBeenCalledWith('staging', { projectId: 'p1' })
    await waitFor(() => expect(screen.getByText('Removed. The URL list updates within a minute.')).toBeTruthy())
  })

  it('cancelling the confirm removes nothing', async () => {
    h.confirmResult = false
    render(<TunnelUrlsPanel canManage />)
    fireEvent.click(screen.getAllByRole('button', { name: 'Remove' })[0])
    await waitFor(() => expect(h.confirm).toHaveBeenCalledTimes(1))
    expect(h.api.remove).not.toHaveBeenCalled()
  })
})
