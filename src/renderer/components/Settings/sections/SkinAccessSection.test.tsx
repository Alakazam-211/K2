// @vitest-environment jsdom
//
// Settings → Skin Access. Mock daemonCli*; never hit Caddy/Hydra.
// Fail loud: errors surface, mint without secret throws, routes are exact.

import { describe, it, expect, beforeEach, vi } from 'vitest'
import { render, screen, waitFor, fireEvent, cleanup } from '@testing-library/react'

const h = vi.hoisted(() => ({
  daemonCliGet: vi.fn(),
  daemonCliPost: vi.fn(),
}))

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: h.daemonCliGet,
  daemonCliPost: h.daemonCliPost,
}))

import {
  SkinAccessSection,
  SKIN_ACCESS_MANIFEST,
  DEFAULT_FRONT_DOOR,
  DEFAULT_SKIN_CAPS,
  SKIN_CAP_CHOICES,
  parseFrontDoor,
  parseSkinUsers,
  parseSkinTokens,
  parseHydra,
  mintSecretFrom,
  prefixLabel,
} from './SkinAccessSection'

const USER_ALICE = { username: 'alice', createdAt: '2026-08-01T00:00:00Z' }
const USER_BOB = { username: 'bob' }
const KEY_ROW = {
  id: 'tok-1',
  prefix: 'k2skn_deadbeefab12',
  name: 'vercel',
  caps: ['thread:read', 'thread:post'],
  rooms: ['proj-sales'],
  roomHandles: ['sales'],
}
const WS_SALES = { id: 'proj-sales', handle: 'sales', name: 'Sales' }
const WS_SUPPORT = { id: 'proj-support', handle: 'support', name: 'Support' }

const HYDRA_UNSUPPORTED = {
  supported: false,
  enabled: false,
  running: false,
  publicUrl: 'http://127.0.0.1:4444/',
  adminUrl: 'http://127.0.0.1:4445/',
  hint: 'THIS FEATURE ONLY WORKS ON LINUX DEPLOYMENTS, THIS PAGE IS JUST HERE FOR EXAMPLE PURPOSES.',
}

const HYDRA_SUPPORTED = {
  supported: true,
  enabled: false,
  running: false,
  publicUrl: 'http://127.0.0.1:4444/',
  adminUrl: 'http://127.0.0.1:4445/',
  hint: 'Off. Enabling skins does not start Hydra. Subject = skin principal id; no users in Hydra.',
}

function mockOk(): void {
  h.daemonCliGet.mockImplementation(async (route: string) => {
    if (route === 'skin/front-door') {
      return { mode: 'connect', connectUrl: 'https://skin.acme.k2.dev', subdomain: 'acme' }
    }
    if (route === 'skin/users') return { users: [USER_ALICE, USER_BOB] }
    if (route === 'skin/roles') return { roles: [] }
    if (route === 'skin-tokens') return { tokens: [KEY_ROW] }
    if (route === 'skin/hydra') return HYDRA_UNSUPPORTED
    if (route === 'projects/list') return [WS_SALES, WS_SUPPORT]
    throw new Error(`unexpected GET ${route}`)
  })
  h.daemonCliPost.mockResolvedValue({ ok: true })
}

async function loaded(): Promise<void> {
  await waitFor(() => {
    expect(screen.getByText('bob')).not.toBeNull()
    expect(screen.getByText('k2skn_…ab12')).not.toBeNull()
  })
}

beforeEach(() => {
  cleanup()
  h.daemonCliGet.mockReset()
  h.daemonCliPost.mockReset()
  mockOk()
})

function lastFrontDoorPost(): Record<string, unknown> {
  const call = [...h.daemonCliPost.mock.calls].reverse().find((c) => c[0] === 'skin/front-door')
  expect(call).toBeTruthy()
  return (call?.[1] ?? {}) as Record<string, unknown>
}

const CADDY_MISSING_FIXTURE = {
  mode: 'direct' as const,
  listen: 'Caddy :443 → 127.0.0.1:18789',
  connectUrl: 'https://skin.acme.k2.dev',
  caddy: { missing: true, running: false, pid: null, binary: null },
}

const NESTED_REGISTERED_FIXTURE = {
  mode: 'connect' as const,
  connectUrl: 'https://skin.acme.k2.dev',
  nested: {
    label: 'skin',
    host: 'skin.acme.k2.dev',
    target: '127.0.0.1:18789',
    registered: true,
  },
}

describe('parsers', () => {
  it('parseFrontDoor defaults to connect stub URLs', () => {
    expect(parseFrontDoor({})).toEqual(DEFAULT_FRONT_DOOR)
    expect(parseFrontDoor({ mode: 'direct', subdomain: 'acme' }).connectUrl).toBe(
      'https://skin.acme.k2.dev',
    )
    expect(parseFrontDoor({ mode: 'direct' }).mode).toBe('direct')
  })

  it('parseFrontDoor accepts old daemon {mode,url,hint} and ignores extra keys', () => {
    expect(
      parseFrontDoor({
        mode: 'connect',
        url: 'https://skin.acme.k2.dev',
        hint: 'Nested hostname',
      }),
    ).toMatchObject({
      mode: 'connect',
      url: 'https://skin.acme.k2.dev',
      hint: 'Nested hostname',
      connectUrl: 'https://skin.acme.k2.dev',
      uiPort: null,
      error: null,
    })
    expect(parseFrontDoor({ mode: 'direct', url: 'https://skin.app.com' }).connectUrl).toBe(
      'https://skin.<sub>.k2.dev',
    )
    const parsed = parseFrontDoor({
      mode: 'direct',
      listen: 'Caddy :443 → 127.0.0.1:9',
      uiPort: 5173,
      applied: true,
      caddy: {
        running: false,
        missing: true,
        pid: null,
        binary: null,
        configPath: '/tmp/Caddyfile',
      },
      nested: {
        label: 'skin',
        host: 'skin.acme.k2.dev',
        target: '127.0.0.1:9',
        registered: true,
      },
      error: 'caddy: not installed',
      extraFuture: { foo: 1 },
    })
    expect(parsed.listen).toBe('Caddy :443 → 127.0.0.1:9')
    expect(parsed.uiPort).toBe(5173)
    expect(parsed.applied).toBe(true)
    expect(parsed.caddy).toEqual({
      running: false,
      missing: true,
      pid: null,
      binary: null,
      configPath: '/tmp/Caddyfile',
    })
    expect(parsed.nested?.host).toBe('skin.acme.k2.dev')
    expect(parsed.nested?.registered).toBe(true)
    expect(parsed.error).toBe('caddy: not installed')
  })

  it('parseSkinUsers reads roster, never invents connect-users fields', () => {
    expect(parseSkinUsers({ users: [USER_ALICE] })).toEqual([
      {
        username: 'alice',
        createdAt: '2026-08-01T00:00:00Z',
        defaultRooms: [],
        defaultRoomHandles: [],
        hasPassword: false,
        roleId: null,
        roleName: null,
      },
    ])
    expect(parseSkinUsers([USER_BOB])).toEqual([
      {
        username: 'bob',
        createdAt: null,
        defaultRooms: [],
        defaultRoomHandles: [],
        hasPassword: false,
        roleId: null,
        roleName: null,
      },
    ])
    expect(
      parseSkinUsers({ users: [{ username: 'cara', hasPassword: true }] })[0].hasPassword,
    ).toBe(true)
  })

  it('parseHydra reads supported/enabled/running and URLs', () => {
    expect(parseHydra(HYDRA_UNSUPPORTED)).toEqual({
      supported: false,
      enabled: false,
      running: false,
      publicUrl: 'http://127.0.0.1:4444/',
      adminUrl: 'http://127.0.0.1:4445/',
      hint: HYDRA_UNSUPPORTED.hint,
    })
    expect(parseHydra({ supported: true, enabled: true, running: false }).supported).toBe(true)
  })

  it('parseSkinTokens keeps prefix + caps; mintSecretFrom is once-only', () => {
    expect(parseSkinTokens({ tokens: [KEY_ROW] })[0]).toEqual({
      id: 'tok-1',
      prefix: 'k2skn_deadbeefab12',
      name: 'vercel',
      caps: ['thread:read', 'thread:post'],
      rooms: ['proj-sales'],
      roomHandles: ['sales'],
    })
    expect(mintSecretFrom({ secret: 'k2skn_once' })).toBe('k2skn_once')
    expect(mintSecretFrom({ id: 'tok-1' })).toBeNull()
    expect(prefixLabel('k2skn_deadbeefab12')).toBe('k2skn_…ab12')
  })
})

describe('SKIN_ACCESS_MANIFEST', () => {
  it('is the Skin Access section, not Server Access', () => {
    expect(SKIN_ACCESS_MANIFEST.every((e) => e.section === 'skin-access')).toBe(true)
    expect(SKIN_ACCESS_MANIFEST.map((e) => e.id)).toEqual([
      'skin-access.front-door',
      'skin-access.users',
      'skin-access.roles',
      'skin-access.keys',
      'skin-access.hydra',
    ])
    expect(SKIN_ACCESS_MANIFEST.map((e) => e.id).join(' ')).not.toContain(
      'agents-can-manage-skin',
    )
  })
})

describe('SkinAccessSection', () => {
  it('loads front-door, skin users, and tokens — never /cli/users', async () => {
    render(<SkinAccessSection />)
    await loaded()
    const gets = h.daemonCliGet.mock.calls.map((c) => c[0])
    expect(gets).toContain('skin/front-door')
    expect(gets).toContain('skin/users')
    expect(gets).toContain('skin/roles')
    expect(gets).toContain('skin-tokens')
    expect(gets.some((r: string) => r === 'users' || r.startsWith('users/'))).toBe(false)
    expect(screen.getByText('https://skin.acme.k2.dev')).not.toBeNull()
    expect(screen.getAllByText('alice').length).toBeGreaterThan(0)
    expect(screen.getAllByText('thread:read').length).toBeGreaterThan(0)
    expect(screen.queryByText(/Stub URLs only/i)).toBeNull()
  })

  it("Connect radio POSTs {mode:'connect'} (apply may be true)", async () => {
    h.daemonCliGet.mockImplementation(async (route: string) => {
      if (route === 'skin/front-door') return { mode: 'direct', listen: ':443' }
      if (route === 'skin/users') return { users: [USER_ALICE, USER_BOB] }
      if (route === 'skin/roles') return { roles: [] }
      if (route === 'skin-tokens') return { tokens: [KEY_ROW] }
      if (route === 'skin/hydra') return HYDRA_UNSUPPORTED
      if (route === 'projects/list') return [WS_SALES, WS_SUPPORT]
      throw new Error(`unexpected GET ${route}`)
    })
    render(<SkinAccessSection />)
    await loaded()
    const doorGets = () => h.daemonCliGet.mock.calls.filter((c) => c[0] === 'skin/front-door').length
    const getsBefore = doorGets()
    fireEvent.click(screen.getByRole('radio', { name: /Use K2 Connect/i }))
    await waitFor(() => {
      expect(h.daemonCliPost).toHaveBeenCalled()
    })
    const body = lastFrontDoorPost()
    expect(body.mode).toBe('connect')
    if (body.apply !== undefined) expect(body.apply).toBe(true)
    await waitFor(() => {
      expect(doorGets()).toBeGreaterThan(getsBefore)
    })
  })

  it("Direct radio POSTs {mode:'direct'}", async () => {
    render(<SkinAccessSection />)
    await loaded()
    const doorGets = () => h.daemonCliGet.mock.calls.filter((c) => c[0] === 'skin/front-door').length
    const getsBefore = doorGets()
    fireEvent.click(screen.getByRole('radio', { name: /Direct \/ this box/i }))
    await waitFor(() => {
      expect(h.daemonCliPost).toHaveBeenCalled()
    })
    const body = lastFrontDoorPost()
    expect(body.mode).toBe('direct')
    if (body.apply !== undefined) expect(body.apply).toBe(true)
    await waitFor(() => {
      expect(doorGets()).toBeGreaterThan(getsBefore)
    })
    const posts = h.daemonCliPost.mock.calls.map((c) => String(c[0]))
    expect(posts.some((r) => /caddy/i.test(r))).toBe(false)
  })

  it('renders listen + caddy missing hint from fixture', async () => {
    h.daemonCliGet.mockImplementation(async (route: string) => {
      if (route === 'skin/front-door') return CADDY_MISSING_FIXTURE
      if (route === 'skin/users') return { users: [USER_ALICE, USER_BOB] }
      if (route === 'skin/roles') return { roles: [] }
      if (route === 'skin-tokens') return { tokens: [KEY_ROW] }
      if (route === 'skin/hydra') return HYDRA_UNSUPPORTED
      if (route === 'projects/list') return [WS_SALES, WS_SUPPORT]
      throw new Error(`unexpected GET ${route}`)
    })
    render(<SkinAccessSection />)
    await loaded()
    expect(screen.getByText('Caddy :443 → 127.0.0.1:18789')).not.toBeNull()
    expect(screen.getByText(/brew install caddy/)).not.toBeNull()
    expect(screen.getByText(/distro package/)).not.toBeNull()
    expect(screen.queryByRole('switch', { name: /caddy/i })).toBeNull()
  })

  it('renders nested host when registered', async () => {
    h.daemonCliGet.mockImplementation(async (route: string) => {
      if (route === 'skin/front-door') return NESTED_REGISTERED_FIXTURE
      if (route === 'skin/users') return { users: [USER_ALICE, USER_BOB] }
      if (route === 'skin/roles') return { roles: [] }
      if (route === 'skin-tokens') return { tokens: [KEY_ROW] }
      if (route === 'skin/hydra') return HYDRA_UNSUPPORTED
      if (route === 'projects/list') return [WS_SALES, WS_SUPPORT]
      throw new Error(`unexpected GET ${route}`)
    })
    render(<SkinAccessSection />)
    await loaded()
    expect(screen.getByText('skin.acme.k2.dev')).not.toBeNull()
    expect(screen.getByText(/registered/i)).not.toBeNull()
  })

  it('surfaces GET error on the existing alert', async () => {
    h.daemonCliGet.mockImplementation(async (route: string) => {
      if (route === 'skin/front-door') {
        return { mode: 'connect', error: 'caddy: binary missing' }
      }
      if (route === 'skin/users') return { users: [USER_ALICE, USER_BOB] }
      if (route === 'skin/roles') return { roles: [] }
      if (route === 'skin-tokens') return { tokens: [KEY_ROW] }
      if (route === 'skin/hydra') return HYDRA_UNSUPPORTED
      if (route === 'projects/list') return [WS_SALES, WS_SUPPORT]
      throw new Error(`unexpected GET ${route}`)
    })
    render(<SkinAccessSection />)
    await loaded()
    expect(screen.getByRole('alert').textContent).toMatch(/caddy: binary missing/)
  })

  it('search filters the skin roster', async () => {
    render(<SkinAccessSection />)
    await loaded()
    const roster = () => document.querySelector('[data-settings-id="skin-access.users"]')
    fireEvent.change(screen.getByLabelText('Search skin users'), { target: { value: 'ali' } })
    expect(roster()?.textContent).toMatch(/alice/)
    expect(roster()?.textContent).not.toMatch(/bob/)
    fireEvent.change(screen.getByLabelText('Search skin users'), { target: { value: 'zzz' } })
    expect(screen.getByText('No users match.')).not.toBeNull()
  })

  it('adds and removes skin users via skin/users — not users/add', async () => {
    render(<SkinAccessSection />)
    await loaded()
    fireEvent.change(screen.getByLabelText('New skin username'), { target: { value: 'carol' } })
    fireEvent.click(screen.getByRole('button', { name: 'Add user' }))
    await waitFor(() => {
      expect(h.daemonCliPost).toHaveBeenCalledWith('skin/users', { username: 'carol' })
    })
    fireEvent.click(screen.getAllByRole('button', { name: 'Remove' })[0])
    fireEvent.click(screen.getByRole('button', { name: 'Confirm remove' }))
    await waitFor(() => {
      expect(h.daemonCliPost).toHaveBeenCalledWith('skin/users/remove', { username: 'alice' })
    })
    const posts = h.daemonCliPost.mock.calls.map((c) => String(c[0]))
    expect(posts).not.toContain('users/add')
    expect(posts).not.toContain('users/remove')
  })

  it('mints a secret once and lists prefix + caps without the secret', async () => {
    h.daemonCliPost.mockImplementation(async (route: string) => {
      if (route === 'skin-tokens') {
        return { id: 'tok-new', prefix: 'k2skn_ffff', name: 'vercel', secret: 'k2skn_ONCESECRET' }
      }
      return { ok: true }
    })
    render(<SkinAccessSection />)
    await loaded()
    fireEvent.change(screen.getByLabelText('Platform token name'), { target: { value: 'vercel' } })
    fireEvent.click(screen.getByLabelText('Mint agent sales'))
    fireEvent.click(screen.getByRole('button', { name: 'Mint key' }))
    await waitFor(() => {
      expect(screen.getByText('k2skn_ONCESECRET')).not.toBeNull()
    })
    expect(h.daemonCliPost).toHaveBeenCalledWith('skin-tokens', {
      name: 'vercel',
      caps: ['thread:read', 'thread:post'],
      rooms: ['sales'],
    })
    expect(DEFAULT_SKIN_CAPS).toEqual(['thread:read', 'thread:post'])
    expect(SKIN_CAP_CHOICES).toEqual([
      'thread:read',
      'thread:post',
      'files:read',
      'files:write',
      'tickets:read',
      'tickets:post',
      'wiki:read',
    ])
    expect(screen.getByText('Store this key now — it cannot be retrieved again')).not.toBeNull()
    expect(screen.getByText('k2skn_…ab12')).not.toBeNull()
  })

  it('offers files read/write checkboxes next to Thread and mints them when checked', async () => {
    h.daemonCliPost.mockImplementation(async (route: string) => {
      if (route === 'skin-tokens') {
        return { id: 'tok-files', prefix: 'k2skn_ffff', name: 'vercel', secret: 'k2skn_FILESECRET' }
      }
      return { ok: true }
    })
    render(<SkinAccessSection />)
    await loaded()
    expect(screen.getByLabelText('Mint cap files:read')).not.toBeNull()
    expect(screen.getByLabelText('Mint cap files:write')).not.toBeNull()
    fireEvent.change(screen.getByLabelText('Platform token name'), { target: { value: 'vercel' } })
    fireEvent.click(screen.getByLabelText('Mint agent sales'))
    fireEvent.click(screen.getByLabelText('Mint cap files:read'))
    fireEvent.click(screen.getByLabelText('Mint cap files:write'))
    fireEvent.click(screen.getByRole('button', { name: 'Mint key' }))
    await waitFor(() => {
      expect(screen.getByText('k2skn_FILESECRET')).not.toBeNull()
    })
    const mintCall = h.daemonCliPost.mock.calls.find((c) => c[0] === 'skin-tokens')
    expect(mintCall).toBeTruthy()
    const body = mintCall![1] as { caps: string[] }
    expect(body.caps).toContain('thread:read')
    expect(body.caps).toContain('thread:post')
    expect(body.caps).toContain('files:read')
    expect(body.caps).toContain('files:write')
  })

  it('fails loud when mint returns no secret', async () => {
    h.daemonCliPost.mockResolvedValue({ id: 'tok-new', prefix: 'k2skn_ffff' })
    render(<SkinAccessSection />)
    await loaded()
    fireEvent.change(screen.getByLabelText('Platform token name'), { target: { value: 'vercel' } })
    fireEvent.click(screen.getByLabelText('Mint agent sales'))
    fireEvent.click(screen.getByRole('button', { name: 'Mint key' }))
    await waitFor(() => {
      expect(screen.getByRole('alert').textContent).toMatch(/mint returned no secret/)
    })
    expect(screen.queryByText('Store this key now — it cannot be retrieved again')).toBeNull()
  })

  it('fails loud when GET routes reject', async () => {
    h.daemonCliGet.mockRejectedValue(new Error('skin routes missing'))
    render(<SkinAccessSection />)
    await waitFor(() => {
      expect(screen.getByRole('alert').textContent).toMatch(/skin routes missing/)
    })
    expect(screen.getByRole('alert').textContent).toMatch(/front-door/)
    expect(screen.getByRole('alert').textContent).toMatch(/users/)
    expect(screen.getByRole('alert').textContent).toMatch(/keys/)
  })

  it('revokes via skin-tokens/revoke', async () => {
    const confirm = vi.spyOn(window, 'confirm').mockReturnValue(true)
    render(<SkinAccessSection />)
    await waitFor(() => expect(screen.getByText('k2skn_…ab12')).not.toBeNull())
    fireEvent.click(screen.getByRole('button', { name: 'Revoke' }))
    await waitFor(() => {
      expect(h.daemonCliPost).toHaveBeenCalledWith('skin-tokens/revoke', { id: 'tok-1' })
    })
    confirm.mockRestore()
  })

  it('unsupported Hydra toggle is disabled and does not POST', async () => {
    render(<SkinAccessSection />)
    await loaded()
    expect(h.daemonCliGet.mock.calls.map((c) => c[0])).toContain('skin/hydra')
    expect(screen.getByText(/THIS FEATURE ONLY WORKS ON LINUX DEPLOYMENTS/i)).not.toBeNull()
    const sw = screen.getByRole('switch', { name: 'Turn on Hydra OIDC issuer' })
    expect(sw.getAttribute('aria-checked')).toBe('false')
    expect((sw as HTMLButtonElement).disabled).toBe(true)
    fireEvent.click(sw)
    expect(h.daemonCliPost.mock.calls.map((c) => String(c[0])).join(' ')).not.toMatch(/hydra/i)
  })

  it('supported Hydra toggle POSTs {enabled, apply:true} when owner', async () => {
    h.daemonCliGet.mockImplementation(async (route: string) => {
      if (route === 'skin/front-door') {
        return { mode: 'connect', connectUrl: 'https://skin.acme.k2.dev', subdomain: 'acme' }
      }
      if (route === 'skin/users') return { users: [USER_ALICE, USER_BOB] }
      if (route === 'skin/roles') return { roles: [] }
      if (route === 'skin-tokens') return { tokens: [KEY_ROW] }
      if (route === 'skin/hydra') return HYDRA_SUPPORTED
      if (route === 'projects/list') return [WS_SALES, WS_SUPPORT]
      throw new Error(`unexpected GET ${route}`)
    })
    h.daemonCliPost.mockImplementation(async (route: string, body?: unknown) => {
      if (route === 'skin/hydra') {
        const rec = (body ?? {}) as { enabled?: boolean }
        return { ...HYDRA_SUPPORTED, enabled: rec.enabled === true, running: rec.enabled === true }
      }
      return { ok: true }
    })
    render(<SkinAccessSection />)
    await loaded()
    const sw = screen.getByRole('switch', { name: 'Turn on Hydra OIDC issuer' })
    expect((sw as HTMLButtonElement).disabled).toBe(false)
    fireEvent.click(sw)
    await waitFor(() => {
      expect(h.daemonCliPost).toHaveBeenCalledWith('skin/hydra', { enabled: true, apply: true })
    })
  })

  it('banners live keys with empty rooms and dismiss does not POST rooms', async () => {
    h.daemonCliGet.mockImplementation(async (route: string) => {
      if (route === 'skin/front-door') {
        return { mode: 'connect', connectUrl: 'https://skin.acme.k2.dev', subdomain: 'acme' }
      }
      if (route === 'skin/users') return { users: [USER_ALICE, USER_BOB] }
      if (route === 'skin/roles') return { roles: [] }
      if (route === 'skin-tokens') {
        return { tokens: [{ ...KEY_ROW, rooms: [], roomHandles: [] }] }
      }
      if (route === 'skin/hydra') return HYDRA_UNSUPPORTED
      if (route === 'projects/list') return [WS_SALES, WS_SUPPORT]
      throw new Error(`unexpected GET ${route}`)
    })
    render(<SkinAccessSection />)
    await loaded()
    expect(screen.getByText(/Assign agents or these platform tokens go dark/i)).not.toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }))
    expect(screen.queryByText(/Assign agents or these platform tokens go dark/i)).toBeNull()
    expect(h.daemonCliPost.mock.calls.map((c) => String(c[0])).join(' ')).not.toMatch(/rooms/)
  })

  it('sets a skin password via skin/users/password, not Connect users', async () => {
    h.daemonCliGet.mockImplementation(async (route: string) => {
      if (route === 'skin/front-door') {
        return { mode: 'connect', connectUrl: 'https://skin.acme.k2.dev', subdomain: 'acme' }
      }
      if (route === 'skin/users') {
        return { users: [{ ...USER_ALICE, hasPassword: false }, USER_BOB] }
      }
      if (route === 'skin/roles') return { roles: [] }
      if (route === 'skin-tokens') return { tokens: [KEY_ROW] }
      if (route === 'skin/hydra') return HYDRA_UNSUPPORTED
      if (route === 'projects/list') return [WS_SALES, WS_SUPPORT]
      throw new Error(`unexpected GET ${route}`)
    })
    render(<SkinAccessSection />)
    await loaded()
    fireEvent.change(screen.getByLabelText('alice password'), { target: { value: 's3cret-horse' } })
    fireEvent.click(screen.getAllByRole('button', { name: 'Set password' })[0])
    await waitFor(() => {
      expect(h.daemonCliPost).toHaveBeenCalledWith('skin/users/password', {
        username: 'alice',
        password: 's3cret-horse',
      })
    })
    expect(h.daemonCliPost.mock.calls.map((c) => String(c[0]))).not.toContain('users/set-password')
  })

  it('mints disabled until an agent is checked', async () => {
    render(<SkinAccessSection />)
    await loaded()
    fireEvent.change(screen.getByLabelText('Platform token name'), { target: { value: 'vercel' } })
    expect((screen.getByRole('button', { name: 'Mint key' }) as HTMLButtonElement).disabled).toBe(
      true,
    )
    fireEvent.click(screen.getByLabelText('Mint agent sales'))
    expect((screen.getByRole('button', { name: 'Mint key' }) as HTMLButtonElement).disabled).toBe(
      false,
    )
  })

  it('loads skin/roles, assigns via POST, and does not offer Connect names', async () => {
    const dentist = {
      id: 'role-1',
      name: 'dentist',
      caps: ['thread:read', 'thread:post', 'files:read'],
      rooms: ['proj-sales'],
      roomHandles: ['sales'],
      roomAccess: [
        { handle: 'sales', caps: ['thread:read', 'thread:post', 'files:read'] },
      ],
    }
    h.daemonCliGet.mockImplementation(async (route: string) => {
      if (route === 'skin/front-door') {
        return { mode: 'connect', connectUrl: 'https://skin.acme.k2.dev', subdomain: 'acme' }
      }
      if (route === 'skin/users') return { users: [USER_ALICE, USER_BOB] }
      if (route === 'skin/roles') return { roles: [dentist] }
      if (route === 'skin-tokens') return { tokens: [KEY_ROW] }
      if (route === 'skin/hydra') return HYDRA_UNSUPPORTED
      if (route === 'projects/list') return [WS_SALES, WS_SUPPORT]
      throw new Error(`unexpected GET ${route}`)
    })
    render(<SkinAccessSection />)
    await loaded()
    expect(h.daemonCliGet.mock.calls.map((c) => c[0])).toContain('skin/roles')
    expect(
      screen.getByText(/Skin roles are not Connect owner\/admin\/member\/viewer/),
    ).not.toBeNull()
    expect(screen.getByText(/They never include the terminal/)).not.toBeNull()
    expect(
      screen.getByText(/Files on Documents does not grant files on Anna/),
    ).not.toBeNull()
    const bobRole = screen.getByLabelText('bob role') as HTMLSelectElement
    const optionNames = [...bobRole.options].map((o) => o.value)
    expect(optionNames).toEqual(['', 'dentist'])
    expect(optionNames).not.toContain('owner')
    expect(optionNames).not.toContain('admin')
    expect(optionNames).not.toContain('member')
    expect(optionNames).not.toContain('viewer')
    fireEvent.change(bobRole, { target: { value: 'dentist' } })
    await waitFor(() => {
      expect(h.daemonCliPost).toHaveBeenCalledWith('skin/roles/assign', {
        username: 'bob',
        role: 'dentist',
      })
    })
    expect(
      h.daemonCliGet.mock.calls
        .map((c) => String(c[0]))
        .some((r) => r === 'users' || r.startsWith('users/')),
    ).toBe(false)
  })

  it('creates a role with per-room roomAccess, not caps+rooms', async () => {
    render(<SkinAccessSection />)
    await loaded()
    fireEvent.change(screen.getByLabelText('New skin role name'), { target: { value: 'dentist' } })
    fireEvent.click(screen.getByLabelText('Role agent sales'))
    fireEvent.click(screen.getByLabelText('Role sales files:read'))
    fireEvent.click(screen.getByRole('button', { name: 'Create role' }))
    await waitFor(() => {
      expect(h.daemonCliPost).toHaveBeenCalledWith('skin/roles', {
        name: 'dentist',
        roomAccess: [
          { handle: 'sales', caps: ['thread:read', 'thread:post', 'files:read'] },
        ],
      })
    })
    const posts = h.daemonCliPost.mock.calls.filter((c) => c[0] === 'skin/roles')
    const body = posts[0][1] as { caps?: unknown; rooms?: unknown }
    expect(body.caps).toBeUndefined()
    expect(body.rooms).toBeUndefined()
  })
})
