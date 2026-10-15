// @vitest-environment jsdom
// Settings → K2 API Tokens: the hire scope (0.45.6, prd-agent-hire-templates-v1
// HT3/HT5/HT8): DB door + "Allow hiring agents" with limits, the owner confirm
// before the mint, the /v1-off hint, and the hire summary on a key row.

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { render, screen, waitFor, fireEvent, cleanup } from '@testing-library/react'

const daemonCliGet = vi.hoisted(() => vi.fn())
const daemonCliPost = vi.hoisted(() => vi.fn())
const settingsState = vi.hoisted(() => ({ apiEnabled: true, setSection: vi.fn() }))
vi.mock('@/lib/daemon-cli', () => ({ daemonCliGet, daemonCliPost }))
vi.mock('@/kessel/server-scope', () => ({ primaryScope: () => 'primary' }))
vi.mock('@/stores/projects', () => ({
  useProjectsStore: (sel: (s: unknown) => unknown) => sel({ projects: [] }),
}))
vi.mock('@/stores/settings', () => {
  const useSettingsStore = (sel: (s: unknown) => unknown) => sel(settingsState)
  ;(useSettingsStore as unknown as { getState: () => unknown }).getState = () => settingsState
  return { useSettingsStore }
})

import { ApiTokensSection, publicApiToggleLocation } from './ApiTokensSection'
import { escOwnedByDialog } from '../settings-esc'
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'

const HIRE_KEY = {
  id: 'key-1',
  label: 'jobs-app',
  scope: 'owner',
  createdAt: 1,
  revokedAt: null,
  disabledAt: null,
  keySet: true,
  anthropicKeySet: false,
  allowedWorkspaces: null,
  provider: null,
  baseUrl: null,
  capabilities: { hostSessions: false, canonicalMessage: false, sandboxes: false, db: false, hire: true },
  hireLimits: {
    maxMembersPerDay: 40,
    maxLiveMembers: 200,
    maxMembersPerTeam: 12,
    db: 'off',
    link: true,
    linkTargets: 'created',
    retire: true,
    reach: 'none',
    folder: 'acme',
  },
}

beforeEach(() => {
  settingsState.apiEnabled = true
  daemonCliGet.mockReset()
  daemonCliPost.mockReset()
  daemonCliGet.mockResolvedValue({ keys: [HIRE_KEY] })
})

afterEach(() => cleanup())

describe('ApiTokensSection hire scope', () => {
  it('shows a hire key with its limits and the hire cap', async () => {
    render(<ApiTokensSection />)
    const line = await screen.findByTestId('hire-limits-key-1')
    expect(line.textContent).toBe(
      'Hires agents: 40/day · 200 live · 12 per team · db off · link · retire · folder acme',
    )
    expect(screen.getByText(/Caps: hire/)).toBeTruthy()
    expect(screen.queryByTestId('api-surface-off')).toBeNull()
  })

  it('warns when the /v1 surface is off', async () => {
    settingsState.apiEnabled = false
    render(<ApiTokensSection />)
    const hint = await screen.findByTestId('api-surface-off')
    // The toggle moved: there is no "K2 Connect" item in Settings now.
    expect(hint.textContent).not.toContain('K2 Connect')
    expect(hint.textContent).toContain('Settings → K2 Server → Tunnel → Policies → Enable public API (/v1)')
    expect(hint.textContent).toContain('k2 api on')
  })

  it('names Connected Servers for the toggle under air-gap (Tunnel is hidden)', () => {
    expect(publicApiToggleLocation(true)).toBe('Settings → K2 Server → Connected Servers → Policies')
    expect(publicApiToggleLocation(false)).toBe('Settings → K2 Server → Tunnel → Policies')
  })

  it('confirms on the Mac, then mints a hire-only key with the limits', async () => {
    daemonCliPost.mockImplementation(async (_scope: string, route: string) => {
      if (route === 'vault/confirm/start') return { methods: ['local-auth'] }
      if (route === 'vault/confirm/finish') return { token: 'tok-1' }
      if (route === 'api-keys/create') return { id: 'k2', key: 'k2sk_secret' }
      throw new Error(`unexpected ${route}`)
    })
    render(<ApiTokensSection />)
    fireEvent.click(await screen.findByText('Create API key'))
    fireEvent.click(screen.getByText('host-sessions'))
    fireEvent.click(screen.getByLabelText('Allow hiring agents'))
    fireEvent.change(screen.getByLabelText('Agents per day'), { target: { value: '5' } })
    fireEvent.click(screen.getByLabelText('May link teams'))
    fireEvent.change(screen.getByLabelText('Folder under the hire root'), { target: { value: 'acme' } })
    fireEvent.click(screen.getByText('Create'))
    await waitFor(() => expect(screen.getByText('k2sk_secret')).toBeTruthy())
    const routes = daemonCliPost.mock.calls.map((c) => c[1])
    expect(routes).toEqual(['vault/confirm/start', 'vault/confirm/finish', 'api-keys/create'])
    expect(daemonCliPost.mock.calls[0][2]).toEqual({ action: 'hire-key-mint' })
    expect(daemonCliPost.mock.calls[1][2]).toMatchObject({ action: 'hire-key-mint', method: 'local-auth' })
    const body = daemonCliPost.mock.calls[2][2] as Record<string, unknown>
    expect(body.confirmToken).toBe('tok-1')
    expect(body.capabilities).toEqual({ hostSessions: false, canonicalMessage: false, sandboxes: false, db: false })
    expect(body.hire).toEqual({
      maxMembersPerDay: 5,
      maxLiveMembers: 100,
      maxMembersPerTeam: 12,
      db: 'off',
      link: true,
      retire: false,
      reach: 'none',
      folder: 'acme',
    })
  })

  it('asks for the vault password when there is no Touch ID', async () => {
    daemonCliPost.mockImplementation(async (_scope: string, route: string, body: Record<string, unknown>) => {
      if (route === 'vault/confirm/start') return { methods: ['vault-password'] }
      if (route === 'vault/confirm/finish') {
        expect(body.password).toBe('correct horse battery')
        return { token: 'tok-2' }
      }
      if (route === 'api-keys/create') return { id: 'k3', key: 'k2sk_two' }
      throw new Error(`unexpected ${route}`)
    })
    render(<ApiTokensSection />)
    fireEvent.click(await screen.findByText('Create API key'))
    fireEvent.click(screen.getByLabelText('Allow hiring agents'))
    fireEvent.click(screen.getByText('Create'))
    const pw = await screen.findByLabelText('Confirm password')
    fireEvent.change(pw, { target: { value: 'correct horse battery' } })
    fireEvent.click(screen.getByText('Confirm and create'))
    await waitFor(() => expect(screen.getByText('k2sk_two')).toBeTruthy())
    expect((daemonCliPost.mock.calls[2][2] as Record<string, unknown>).confirmToken).toBe('tok-2')
  })

  it('refuses database access for hired agents without the key db door, before any confirm', async () => {
    render(<ApiTokensSection />)
    fireEvent.click(await screen.findByText('Create API key'))
    fireEvent.click(screen.getByLabelText('Allow hiring agents'))
    const sel = screen.getByLabelText('Database access for hired agents') as HTMLSelectElement
    expect((sel.querySelector('option[value="write"]') as HTMLOptionElement).disabled).toBe(true)
    fireEvent.change(screen.getByLabelText('Agents per day'), { target: { value: '0' } })
    fireEvent.click(screen.getByText('Create'))
    expect(await screen.findByText(/Agents per day must be a whole number/)).toBeTruthy()
    expect(daemonCliPost).not.toHaveBeenCalled()
  })

  it('a plain key mints without a confirm and sends the db door', async () => {
    daemonCliPost.mockResolvedValue({ id: 'k4', key: 'k2sk_plain' })
    render(<ApiTokensSection />)
    fireEvent.click(await screen.findByText('Create API key'))
    fireEvent.click(screen.getByText(/^db \(create/))
    fireEvent.click(screen.getByText('Create'))
    await waitFor(() => expect(screen.getByText('k2sk_plain')).toBeTruthy())
    expect(daemonCliPost).toHaveBeenCalledTimes(1)
    const body = daemonCliPost.mock.calls[0][2] as Record<string, unknown>
    expect(body.capabilities).toEqual({ hostSessions: true, canonicalMessage: false, sandboxes: false, db: true })
    expect(body.hire).toBeUndefined()
  })
})

// ── App access limit (0.45.6, Rosson 2026-10-14) ─────────────────────────

const OPTIONS = {
  apps: [
    {
      id: 'svc-jobs',
      name: 'jobs',
      kind: 'skin',
      workspaceId: 'ws-portal',
      workspace: 'portal',
      roomIds: ['ws-portal', 'ws-a'],
      roles: [
        { id: 'role-member', name: 'Member', appId: 'svc-jobs', rooms: ['ws-a'] },
        { id: 'role-viewer', name: 'Viewer', appId: null, rooms: ['ws-a'] },
      ],
    },
  ],
  rooms: [
    { id: 'self', handle: 'self', name: "The hired agent's own workspace" },
    { id: 'ws-portal', handle: 'portal', name: 'Portal' },
    { id: 'ws-a', handle: 'job-a', name: 'Job A' },
    { id: 'ws-b', handle: 'job-b', name: 'Job B' },
  ],
}

const CEILING_KEY = {
  ...HIRE_KEY,
  id: 'key-c',
  label: 'jobs-ceiling',
  hireLimits: {
    ...HIRE_KEY.hireLimits,
    apps: [{ app: 'svc-jobs', rooms: ['self', 'ws-a', 'ws-b'], roles: ['role-member'] }],
  },
}

function routeGets(keys: unknown[]) {
  daemonCliGet.mockImplementation(async (_scope: string, route: string) => {
    if (route === 'api-keys/list') return { keys }
    if (route === 'api-keys/hire-ceiling-options') return OPTIONS
    throw new Error(`unexpected GET ${route}`)
  })
}

const confirmRequired = () =>
  new Error(JSON.stringify({ ok: false, error: { code: 'vault_confirm_required', hint: 'confirm first' } }))

describe('ApiTokensSection App access limit', () => {
  it('shows the limit in one line on the key row', async () => {
    routeGets([CEILING_KEY])
    render(<ApiTokensSection />)
    await waitFor(() =>
      expect(screen.getByTestId('hire-ceiling-key-c').textContent).toBe('App access: jobs · 3 rooms · up to Member'),
    )
  })

  it('says "No App access" for a hire key with an empty limit', async () => {
    routeGets([HIRE_KEY])
    render(<ApiTokensSection />)
    const line = await screen.findByTestId('hire-ceiling-key-1')
    expect(line.textContent).toBe('App access: No App access')
  })

  it('the create form starts empty and says agents get no App login', async () => {
    routeGets([])
    render(<ApiTokensSection />)
    fireEvent.click(await screen.findByText('Create API key'))
    fireEvent.click(screen.getByLabelText('Allow hiring agents'))
    const none = screen.getByTestId('app-access-none')
    expect(none.textContent).toContain('No App access: agents hired with this key get no App login.')
    expect((screen.getByLabelText('App') as HTMLSelectElement).value).toBe('')
    expect(screen.queryByLabelText('Highest role')).toBeNull()
  })

  it('mints a hire key with the App, rooms and highest role picked in the form', async () => {
    routeGets([])
    daemonCliPost.mockImplementation(async (_scope: string, route: string) => {
      if (route === 'vault/confirm/start') return { methods: ['local-auth'] }
      if (route === 'vault/confirm/finish') return { token: 'tok-c' }
      if (route === 'api-keys/create') return { id: 'k9', key: 'k2sk_ceiling' }
      throw new Error(`unexpected ${route}`)
    })
    render(<ApiTokensSection />)
    fireEvent.click(await screen.findByText('Create API key'))
    fireEvent.click(screen.getByLabelText('Allow hiring agents'))
    await waitFor(() => expect(screen.getByLabelText('App').querySelectorAll('option').length).toBe(2))
    fireEvent.change(screen.getByLabelText('App'), { target: { value: 'svc-jobs' } })
    expect((screen.getByLabelText('Room self') as HTMLInputElement).checked).toBe(true)
    fireEvent.click(screen.getByLabelText('Room job-a'))
    const role = screen.getByLabelText('Highest role') as HTMLSelectElement
    expect([...role.options].map((o) => o.textContent)).toEqual([
      'No role (Thread only in these rooms)',
      'Member',
      'Viewer (all Apps)',
    ])
    fireEvent.change(role, { target: { value: 'role-member' } })
    fireEvent.click(screen.getByText('Create'))
    await waitFor(() => expect(screen.getByText('k2sk_ceiling')).toBeTruthy())
    const body = daemonCliPost.mock.calls[2][2] as { hire: Record<string, unknown>; confirmToken: string }
    expect(body.confirmToken).toBe('tok-c')
    expect(body.hire.apps).toEqual([{ app: 'svc-jobs', rooms: ['self', 'ws-a'], roles: ['role-member'] }])
  })

  it('refuses an App with no rooms before any confirm', async () => {
    routeGets([])
    render(<ApiTokensSection />)
    fireEvent.click(await screen.findByText('Create API key'))
    fireEvent.click(screen.getByLabelText('Allow hiring agents'))
    await waitFor(() => expect(screen.getByLabelText('App').querySelectorAll('option').length).toBe(2))
    fireEvent.change(screen.getByLabelText('App'), { target: { value: 'svc-jobs' } })
    fireEvent.click(screen.getByLabelText('Room self'))
    fireEvent.click(screen.getByText('Create'))
    expect(await screen.findByText(/Pick at least one room on jobs/)).toBeTruthy()
    expect(daemonCliPost).not.toHaveBeenCalled()
  })

  it('lowering to "No App access" saves without a confirm', async () => {
    routeGets([CEILING_KEY])
    daemonCliPost.mockImplementation(async (_scope: string, route: string) => {
      if (route === 'api-keys/hire-ceiling') return { id: 'key-c', change: 'lower', confirmed: false, hireLimits: {} }
      throw new Error(`unexpected ${route}`)
    })
    render(<ApiTokensSection />)
    fireEvent.click(await screen.findByLabelText('Change App access for jobs-ceiling'))
    const dialog = screen.getByTestId('edit-app-access')
    await waitFor(() => expect(dialog.textContent).toContain('Now: jobs · 3 rooms · up to Member'))
    expect((screen.getByLabelText('App') as HTMLSelectElement).value).toBe('svc-jobs')
    expect((screen.getByLabelText('Room job-b') as HTMLInputElement).checked).toBe(true)
    expect((screen.getByLabelText('Highest role') as HTMLSelectElement).value).toBe('role-member')
    fireEvent.change(screen.getByLabelText('App'), { target: { value: '' } })
    expect(screen.getByTestId('app-access-none').textContent).toContain('get no App login')
    fireEvent.click(screen.getByText('Save'))
    await waitFor(() => expect(screen.queryByTestId('edit-app-access')).toBeNull())
    expect(daemonCliPost.mock.calls.map((c) => c[1])).toEqual(['api-keys/hire-ceiling'])
    expect(daemonCliPost.mock.calls[0][2]).toEqual({ id: 'key-c', apps: [] })
  })

  it('raising asks for the owner confirm on the Mac, then saves with the token', async () => {
    routeGets([CEILING_KEY])
    let saves = 0
    daemonCliPost.mockImplementation(async (_scope: string, route: string) => {
      if (route === 'api-keys/hire-ceiling') {
        saves += 1
        if (saves === 1) throw confirmRequired()
        return { id: 'key-c', change: 'raise', confirmed: true, hireLimits: {} }
      }
      if (route === 'vault/confirm/start') return { methods: ['local-auth'] }
      if (route === 'vault/confirm/finish') return { token: 'tok-raise' }
      throw new Error(`unexpected ${route}`)
    })
    render(<ApiTokensSection />)
    fireEvent.click(await screen.findByLabelText('Change App access for jobs-ceiling'))
    fireEvent.click(await screen.findByLabelText('Room portal'))
    fireEvent.click(screen.getByText('Save'))
    await waitFor(() => expect(screen.queryByTestId('edit-app-access')).toBeNull())
    expect(daemonCliPost.mock.calls.map((c) => c[1])).toEqual([
      'api-keys/hire-ceiling',
      'vault/confirm/start',
      'vault/confirm/finish',
      'api-keys/hire-ceiling',
    ])
    expect(daemonCliPost.mock.calls[0][2]).toEqual({
      id: 'key-c',
      apps: [{ app: 'svc-jobs', rooms: ['self', 'ws-a', 'ws-b', 'ws-portal'], roles: ['role-member'] }],
    })
    expect(daemonCliPost.mock.calls[1][2]).toEqual({ action: 'hire-key-mint' })
    expect(daemonCliPost.mock.calls[2][2]).toMatchObject({ action: 'hire-key-mint', method: 'local-auth' })
    expect(daemonCliPost.mock.calls[3][2]).toMatchObject({ id: 'key-c', confirmToken: 'tok-raise' })
  })

  it('raising without Touch ID asks for the password; a wrong one saves nothing and keeps the dialog', async () => {
    routeGets([CEILING_KEY])
    let saves = 0
    daemonCliPost.mockImplementation(async (_scope: string, route: string, body: Record<string, unknown>) => {
      if (route === 'api-keys/hire-ceiling') {
        saves += 1
        if (saves === 1) throw confirmRequired()
        expect(body.confirmToken).toBe('tok-pw')
        return { id: 'key-c', change: 'raise', confirmed: true, hireLimits: {} }
      }
      if (route === 'vault/confirm/start') return { methods: ['vault-password'] }
      if (route === 'vault/confirm/finish') {
        if (body.password !== 'right one') {
          throw new Error(JSON.stringify({ error: { code: 'bad_password', hint: 'That password is not right.' } }))
        }
        return { token: 'tok-pw' }
      }
      throw new Error(`unexpected ${route}`)
    })
    render(<ApiTokensSection />)
    fireEvent.click(await screen.findByLabelText('Change App access for jobs-ceiling'))
    fireEvent.change(await screen.findByLabelText('Highest role'), { target: { value: 'role-viewer' } })
    fireEvent.click(screen.getByLabelText('Room portal'))
    fireEvent.click(screen.getByText('Save'))
    const pw = await screen.findByLabelText('Confirm password')
    fireEvent.change(pw, { target: { value: 'wrong' } })
    fireEvent.click(screen.getByText('Confirm and save'))
    expect(await screen.findByText('That password is not right.')).toBeTruthy()
    expect(saves).toBe(1)
    expect(screen.getByTestId('edit-app-access')).toBeTruthy()
    fireEvent.change(screen.getByLabelText('Confirm password'), { target: { value: 'right one' } })
    fireEvent.click(screen.getByText('Confirm and save'))
    await waitFor(() => expect(screen.queryByTestId('edit-app-access')).toBeNull())
    expect(saves).toBe(2)
  })
})

describe('Esc in the Create API key dialog (0.45.6 Linux check)', () => {
  // Settings closes on Escape from a window listener registered before any
  // dialog inside it opens. Same order here, same guard as Settings.tsx.
  let settingsClose: ReturnType<typeof vi.fn>
  let settingsEsc: (e: KeyboardEvent) => void
  beforeEach(() => {
    settingsClose = vi.fn()
    settingsEsc = (e) => {
      if (e.key !== 'Escape' || escOwnedByDialog(e)) return
      settingsClose()
    }
    window.addEventListener('keydown', settingsEsc)
  })
  afterEach(() => window.removeEventListener('keydown', settingsEsc))

  const esc = () => fireEvent.keyDown(window, { key: 'Escape' })

  it('one Esc closes the dialog only; the next closes Settings', async () => {
    render(<ApiTokensSection />)
    fireEvent.click(await screen.findByText('Create API key'))
    expect(screen.getByRole('dialog', { name: 'Create API key' })).toBeTruthy()
    esc()
    await waitFor(() => expect(screen.queryByRole('dialog', { name: 'Create API key' })).toBeNull())
    expect(settingsClose).not.toHaveBeenCalled()
    esc()
    expect(settingsClose).toHaveBeenCalledTimes(1)
  })

  it('Settings stands down even when the dialog listener ran first', () => {
    const ev = new KeyboardEvent('keydown', { key: 'Escape', cancelable: true })
    ev.preventDefault()
    expect(escOwnedByDialog(ev)).toBe(true)
    const plain = new KeyboardEvent('keydown', { key: 'Escape', cancelable: true })
    expect(escOwnedByDialog(plain)).toBe(false)
  })

  it('Settings.tsx checks the guard before closing', () => {
    const src = readFileSync(resolve(__dirname, '../Settings.tsx'), 'utf8')
    const at = src.indexOf("if (e.key === 'Escape' && !searchOpen) {")
    expect(at).toBeGreaterThan(-1)
    const guard = src.indexOf('if (escOwnedByDialog(e)) return', at)
    const close = src.indexOf('closeSettings()', at)
    expect(guard).toBeGreaterThan(at)
    expect(guard).toBeLessThan(close)
  })
})
