// @vitest-environment jsdom

// Settings → Database: the page reads the daemon's db/status and db/list
// (checked replies), shows sample data under the Linux note when the daemon
// says it can't run here, creates a database through a dialog, and confirms
// every destructive action in the shared dialog frame, keeping a failure
// in the dialog.

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { render, screen, waitFor, fireEvent, cleanup, within } from '@testing-library/react'

vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}), emit: vi.fn(async () => {}) }))
vi.mock('@/lib/daemon-cli', () => ({ daemonCliGet: vi.fn(), daemonCliPost: vi.fn() }))

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { DataSection } from './DataSection'
import { useProjectsStore } from '@/stores/projects'

const get = vi.mocked(daemonCliGet)
const post = vi.mocked(daemonCliPost)

const STATUS = { ok: true, supported: true, state: 'running', installedMajor: 16, listen: 'localhost', port: 5432, publishHint: 'x', lastError: null, clusters: [], databases: [{ id: 'db1', name: 'ws_atlas', major: 16 }], upgradeAvailable: [] }
const DB = {
  id: 'db1', name: 'ws_atlas', status: 'active', createdAt: 1762000000, type: 'sql', documents: true, ownerProjectId: 'p1', ownerWorkspace: 'atlas', bindRole: 'ws_atlas_agent', cap: { used: 1, cap: 1 },
  owner: { projectId: 'p1', workspace: 'atlas', level: 'write', canManage: true },
  grants: [{ projectId: 'p2', workspace: 'billing', level: 'read', canManage: false }], yourLevel: 'write', dbAgentAccess: 'off',
}

function routes(over: Record<string, unknown> = {}): void {
  get.mockImplementation(async (_scope, path: string) => {
    if (path in over) {
      const v = over[path]
      if (v instanceof Error) throw v
      return v as never
    }
    if (path === 'db/status') return STATUS as never
    if (path === 'db/list') return { ok: true, databases: [DB] } as never
    if (path === 'db/upgrade') return { live: [], walLevels: [], liveHelper: true } as never
    throw new Error(`unexpected GET ${path}`)
  })
}

beforeEach(() => {
  vi.clearAllMocks()
  useProjectsStore.setState({ projects: [{ id: 'p1', name: 'atlas', path: '/w/atlas' }, { id: 'p2', name: 'billing', path: '/w/billing' }, { id: 'p3', name: 'notes', path: '/w/notes' }] } as never)
})
afterEach(() => cleanup())

describe('DataSection', () => {
  it('lists databases under a header and tabs, with no Linux note on a Linux server', async () => {
    routes()
    render(<DataSection />)
    const row = await screen.findByTestId('db-row')
    expect(row.textContent).toContain('ws_atlas')
    expect(row.textContent).toContain('Owned by atlas · Postgres 16 · shared with 1 workspace')
    expect(screen.getByRole('tab', { name: 'Databases' }).getAttribute('aria-selected')).toBe('true')
    expect(screen.queryByTestId('linux-only-note')).toBeNull()
  })

  it('a reply without a state is a failed read with Try again, never an empty page', async () => {
    routes({ 'db/status': {} })
    render(<DataSection />)
    const card = await screen.findByTestId('db-status-error')
    expect(card.textContent).toContain('Couldn’t read the database server’s status')
    expect(card.textContent).toContain('The server said: the reply had no server state.')
    routes()
    fireEvent.click(within(card).getByRole('button', { name: 'Try again' }))
    expect(await screen.findByTestId('db-row')).toBeTruthy()
  })

  it('unsupported: the Linux note, sample data, every change off, no list read', async () => {
    routes({ 'db/status': { ...STATUS, supported: false, state: 'not-installed' } })
    render(<DataSection />)
    expect((await screen.findByTestId('linux-only-note')).textContent).toContain('Database hosting runs on Linux servers.')
    expect((await screen.findAllByTestId('db-row')).map((r) => r.getAttribute('data-db'))).toEqual(['ws_sales', 'ws_ops'])
    expect((screen.getByTestId('db-create-open') as HTMLButtonElement).disabled).toBe(true)
    expect(get.mock.calls.some((c) => c[1] === 'db/list')).toBe(false)
    expect(document.body.textContent).not.toMatch(/THIS FEATURE ONLY WORKS/)
  })

  it('creates a database from the dialog and says so', async () => {
    routes()
    post.mockResolvedValueOnce({ ok: true, name: 'ws_notes' } as never)
    render(<DataSection />)
    fireEvent.click(await screen.findByTestId('db-create-open'))
    const dialog = await screen.findByTestId('db-create-dialog')
    fireEvent.click(within(dialog).getByRole('button', { name: 'Workspace' }))
    fireEvent.click(await screen.findByText('notes'))
    fireEvent.click(screen.getByTestId('db-create-submit'))
    await waitFor(() => expect(post).toHaveBeenCalledWith(expect.anything(), 'db/create', { project: 'p3' }))
    expect((await screen.findByTestId('db-notice')).textContent).toContain('Created ws_notes for notes.')
    expect(screen.queryByTestId('db-create-dialog')).toBeNull()
  })

  it('a failed create stays in the dialog with the server’s reason', async () => {
    routes()
    post.mockRejectedValueOnce(new Error(JSON.stringify({ error: { code: 'cap', hint: 'atlas already has its one database' } })))
    render(<DataSection />)
    fireEvent.click(await screen.findByTestId('db-create-open'))
    const dialog = await screen.findByTestId('db-create-dialog')
    fireEvent.click(within(dialog).getByRole('button', { name: 'Workspace' }))
    fireEvent.click(await screen.findByText('atlas'))
    fireEvent.click(screen.getByTestId('db-create-submit'))
    const err = await screen.findByTestId('dialog-error')
    expect(err.textContent).toContain('Couldn’t create the database.')
    expect(err.textContent).toContain('The server said: atlas already has its one database.')
  })

  it('removing access confirms first, then drops the row', async () => {
    routes()
    post.mockResolvedValueOnce({ ok: true } as never)
    render(<DataSection />)
    fireEvent.click(await screen.findByTestId('db-row'))
    fireEvent.click(await screen.findByRole('button', { name: 'Remove billing’s access' }))
    expect(post).not.toHaveBeenCalled()
    const confirm = await screen.findByTestId('db-remove-confirm')
    fireEvent.click(within(confirm).getByRole('button', { name: 'Remove access' }))
    await waitFor(() => expect(post).toHaveBeenCalledWith(expect.anything(), 'db/revoke', { project: 'p2', db: 'db1' }))
    await waitFor(() => expect(screen.queryAllByTestId('db-grant-row')).toHaveLength(0))
    expect(screen.getByTestId('db-notice').textContent).toContain('Removed billing’s access to ws_atlas.')
  })

  it('a permission refusal keeps the dialog open, names who can, and offers no retry', async () => {
    routes()
    post.mockRejectedValueOnce(new Error(JSON.stringify({ ok: false, error: { code: 'forbidden', hint: 'only the owning workspace may grant on ws_atlas' } })))
    render(<DataSection />)
    fireEvent.click(await screen.findByTestId('db-row'))
    fireEvent.click(await screen.findByTestId('db-grant-open'))
    const dialog = await screen.findByTestId('db-grant-dialog')
    fireEvent.click(within(dialog).getByRole('button', { name: 'Workspace' }))
    fireEvent.click(await screen.findByText('notes'))
    fireEvent.click(screen.getByTestId('db-grant-submit'))
    const err = await screen.findByTestId('dialog-error')
    expect(err.textContent).toContain('Only the workspace’s owner can change access to ws_atlas.')
    expect(err.textContent).not.toMatch(/try again/i)
    expect(screen.getByTestId('db-grant-dialog')).toBeTruthy()
    expect(screen.queryByTestId('db-grant-submit')).toBeNull()
    expect(within(screen.getByTestId('db-grant-dialog')).getByRole('button', { name: 'Close' })).toBeTruthy()
  })

  it('a refused access change on a row says who can and keeps the old value', async () => {
    routes()
    post.mockRejectedValueOnce(new Error(JSON.stringify({ ok: false, error: { code: 'forbidden', hint: 'not yours' } })))
    render(<DataSection />)
    fireEvent.click(await screen.findByTestId('db-row'))
    const sw = await screen.findByRole('switch', { name: 'billing can manage access' })
    fireEvent.click(sw)
    expect((await screen.findByTestId('db-grant-refused')).textContent).toBe('Only the workspace’s owner can change access to ws_atlas. Nothing changed.')
    expect(screen.getByRole('switch', { name: 'billing can manage access' }).getAttribute('aria-checked')).toBe('false')
  })

  it('not set up: Set up, and Postgres missing from the server is said in words', async () => {
    routes({ 'db/status': { ...STATUS, state: 'not-installed', installedMajor: null, listen: null } })
    post.mockRejectedValueOnce(new Error(JSON.stringify({ ok: false, error: { code: 'not_baked', hint: 'Postgres sidecar is not baked on this box. Re-run provision-k2-server.sh --bake --with-db' } })))
    render(<DataSection />)
    fireEvent.click(await screen.findByRole('tab', { name: 'Server' }))
    const btn = await screen.findByTestId('db-turn-on')
    expect(btn.textContent).toBe('Set up')
    fireEvent.click(btn)
    const note = await screen.findByTestId('db-notice')
    expect(note.textContent).toContain('Postgres isn’t installed on this server')
    expect(note.textContent).not.toContain('provision-k2-server')
  })

  it('turning the server off confirms that data is kept', async () => {
    routes()
    post.mockResolvedValueOnce({ ok: true, state: 'disabled' } as never)
    render(<DataSection />)
    fireEvent.click(await screen.findByRole('tab', { name: 'Server' }))
    expect((await screen.findByTestId('db-listen')).textContent).toBe('localhost:5432')
    fireEvent.click(screen.getByTestId('db-turn-off'))
    const confirm = await screen.findByTestId('db-turn-off-confirm')
    expect(confirm.textContent).toContain('Every database and its data is kept.')
    fireEvent.click(within(confirm).getByRole('button', { name: 'Turn off' }))
    await waitFor(() => expect(post).toHaveBeenCalledWith(expect.anything(), 'db/server/disable', {}))
    expect((await screen.findByTestId('db-notice')).textContent).toContain('The database server is off.')
  })
})
