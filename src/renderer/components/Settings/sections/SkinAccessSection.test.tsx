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

vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly((...a: unknown[]) => h.daemonCliGet(...a)),
    daemonCliPost: primaryOnly((...a: unknown[]) => h.daemonCliPost(...a)),
  }
})

import {
  SkinAccessSection,
  SKIN_ACCESS_MANIFEST,
  DEFAULT_FRONT_DOOR,
  DEFAULT_SKIN_CAPS,
  SKIN_ACTIVITY_CAPS,
  SKIN_CAP_CHOICES,
  SKIN_CAP_LABELS,
  SKIN_HEARTBEAT_CAPS,
  skinCapChoices,
  SKIN_FILE_CAPS,
  SKIN_STORE_CAPS,
  parseFrontDoor,
  parseSkinUsers,
  parseSkinTokens,
  parseHydra,
  mintSecretFrom,
  prefixLabel,
} from './SkinAccessSection'
import { searchManifest, SECTION_LABELS } from '../searchManifest'

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
    if (route === 'skin/grants') return { grants: [] }
    if (route === 'publish/list') return { services: [] }
    throw new Error(`unexpected GET ${route}`)
  })
  h.daemonCliPost.mockResolvedValue({ ok: true })
}

async function loaded(): Promise<void> {
  await waitFor(() => {
    expect(screen.queryByText('Loading…')).toBeNull()
    expect(screen.getByRole('heading', { level: 2, name: 'Apps' })).not.toBeNull()
  })
}

async function openHost(): Promise<void> {
  await loaded()
  fireEvent.click(screen.getByRole('button', { name: 'Host' }))
  await waitFor(() => {
    expect(screen.getByText('k2skn_…ab12')).not.toBeNull()
  })
}

beforeEach(() => {
  cleanup()
  h.daemonCliGet.mockReset()
  h.daemonCliPost.mockReset()
  mockOk()
})

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
        id: null,
        username: 'alice',
        createdAt: '2026-08-01T00:00:00Z',
        defaultRooms: [],
        defaultRoomHandles: [],
        hasPassword: false,
        roleId: null,
        roleName: null,
        email: null,
      },
    ])
    expect(parseSkinUsers([USER_BOB])).toEqual([
      {
        id: null,
        username: 'bob',
        createdAt: null,
        defaultRooms: [],
        defaultRoomHandles: [],
        hasPassword: false,
        roleId: null,
        roleName: null,
        email: null,
      },
    ])
    expect(parseSkinUsers({ users: [{ id: 'p-ada', username: 'ada' }] })[0].id).toBe('p-ada')
    expect(
      parseSkinUsers({ users: [{ username: 'cara', email: 'cara@clinic.com' }] })[0].email,
    ).toBe('cara@clinic.com')
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
  it('is the Apps section (route id skin-access), not Server Access', () => {
    expect(SKIN_ACCESS_MANIFEST.every((e) => e.section === 'skin-access')).toBe(true)
    expect(SKIN_ACCESS_MANIFEST.map((e) => e.id)).toEqual([
      'skin-access.roles',
      'skin-access.keys',
      'skin-access.hydra',
    ])
    expect(SKIN_ACCESS_MANIFEST.some((e) => /caddy/i.test(e.label + (e.group ?? '')))).toBe(false)
    expect(SKIN_ACCESS_MANIFEST.map((e) => e.id).join(' ')).not.toContain(
      'agents-can-manage-skin',
    )
    for (const e of SKIN_ACCESS_MANIFEST) {
      expect(e.keywords ?? []).toEqual(expect.arrayContaining(['skin', 'app']))
    }
  })
})

describe('SkinAccessSection', () => {
  it('nav/h2 is Apps, not Skin Access or Server Access', async () => {
    expect(SECTION_LABELS['skin-access']).toBe('Apps')
    expect(SECTION_LABELS['skin-access']).not.toBe('Skin Access')
    expect(SECTION_LABELS['k2-access']).toBe('Admin Access')
    render(<SkinAccessSection />)
    await loaded()
    expect(screen.getByRole('heading', { level: 2, name: 'Apps' })).not.toBeNull()
    expect(screen.queryByRole('heading', { name: 'Skin Access' })).toBeNull()
    expect(screen.queryByRole('heading', { name: 'Server Access' })).toBeNull()
    expect(screen.queryByRole('heading', { name: 'Admin Access' })).toBeNull()
    expect(screen.queryByRole('heading', { name: 'Roles' })).toBeNull()
  })

  it('search skin and app both hit the Apps page', () => {
    const skinHits = searchManifest(SKIN_ACCESS_MANIFEST, 'skin')
    const appHits = searchManifest(SKIN_ACCESS_MANIFEST, 'app')
    expect(skinHits.length).toBeGreaterThan(0)
    expect(appHits.length).toBeGreaterThan(0)
    expect(skinHits.every((e) => e.section === 'skin-access')).toBe(true)
    expect(appHits.every((e) => e.section === 'skin-access')).toBe(true)
  })

  it('loads users, roles, and tokens — never front-door or /cli/users', async () => {
    render(<SkinAccessSection />)
    await openHost()
    const gets = h.daemonCliGet.mock.calls.map((c) => c[0])
    expect(gets).not.toContain('skin/front-door')
    expect(gets).toContain('skin/users')
    expect(gets).toContain('skin/roles')
    expect(gets).toContain('skin-tokens')
    expect(gets.some((r: string) => r === 'users' || r.startsWith('users/'))).toBe(false)
    expect(screen.queryByText('Leftover Caddy (optional)')).toBeNull()
    expect(screen.queryByRole('radio')).toBeNull()
    expect(screen.getByText('k2skn_…ab12')).not.toBeNull()
    expect(screen.getAllByText('thread:read').length).toBeGreaterThan(0)
    expect(screen.queryByText(/Stub URLs only/i)).toBeNull()
  })

  it('does not render Leftover Caddy or the guest create form', async () => {
    render(<SkinAccessSection />)
    await loaded()
    expect(screen.queryByRole('heading', { name: 'Leftover Caddy (optional)' })).toBeNull()
    expect(screen.queryByRole('heading', { name: 'Guests' })).toBeNull()
    expect(screen.queryByLabelText('New skin username')).toBeNull()
    expect(screen.queryByRole('radio', { name: /Use K2 Connect/i })).toBeNull()
    expect(screen.queryByRole('radio', { name: /Direct \/ this box/i })).toBeNull()
    expect(screen.queryByText(/brew install caddy/)).toBeNull()
  })

  it('surfaces a users GET error without a front-door failure', async () => {
    h.daemonCliGet.mockImplementation(async (route: string) => {
      if (route === 'skin/users') throw new Error('users roster down')
      if (route === 'skin/roles') return { roles: [] }
      if (route === 'skin-tokens') return { tokens: [KEY_ROW] }
      if (route === 'skin/hydra') return HYDRA_UNSUPPORTED
      if (route === 'projects/list') return [WS_SALES, WS_SUPPORT]
      if (route === 'skin/grants') return { grants: [] }
      if (route === 'publish/list') return { services: [] }
      throw new Error(`unexpected GET ${route}`)
    })
    render(<SkinAccessSection />)
    await waitFor(() => {
      expect(screen.getByRole('alert').textContent).toMatch(/users roster down/)
    })
    expect(screen.getByRole('alert').textContent).not.toMatch(/front-door/)
    expect(screen.queryByText('Leftover Caddy (optional)')).toBeNull()
  })

  it('mints a secret once and lists prefix + caps without the secret', async () => {
    h.daemonCliPost.mockImplementation(async (route: string) => {
      if (route === 'skin-tokens') {
        return { id: 'tok-new', prefix: 'k2skn_ffff', name: 'vercel', secret: 'k2skn_ONCESECRET' }
      }
      return { ok: true }
    })
    render(<SkinAccessSection />)
    await openHost()
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
    expect(DEFAULT_SKIN_CAPS).not.toContain('activity:read')
    expect(SKIN_FILE_CAPS).toEqual(['files:read', 'files:write'])
    expect(SKIN_FILE_CAPS).not.toContain('activity:read')
    expect(SKIN_ACTIVITY_CAPS).toEqual(['activity:read'])
    expect(SKIN_STORE_CAPS).toEqual(['store:read', 'store:write'])
    expect(SKIN_CAP_CHOICES).toEqual([
      'thread:read',
      'thread:post',
      'files:read',
      'files:write',
      'tickets:read',
      'tickets:post',
      'wiki:read',
      'store:read',
      'store:write',
      'activity:read',
      'heartbeats:read',
      'heartbeats:write',
    ])
    expect(screen.getByText('Store this key now — it cannot be retrieved again')).not.toBeNull()
    expect(screen.getByText('k2skn_…ab12')).not.toBeNull()
  })

  it('T14: heartbeat caps are their own group, labelled, never default, and gated by app-heartbeats', async () => {
    expect(SKIN_HEARTBEAT_CAPS).toEqual(['heartbeats:read', 'heartbeats:write'])
    expect(DEFAULT_SKIN_CAPS).not.toContain('heartbeats:read')
    expect(DEFAULT_SKIN_CAPS).not.toContain('heartbeats:write')
    expect(SKIN_FILE_CAPS).not.toContain('heartbeats:read')
    expect(SKIN_ACTIVITY_CAPS).not.toContain('heartbeats:read')
    for (const cap of SKIN_CAP_CHOICES) {
      expect(SKIN_CAP_LABELS[cap].length).toBeGreaterThan(0)
    }
    expect(skinCapChoices(true)).toEqual(SKIN_CAP_CHOICES)
    const older = skinCapChoices(false)
    expect(older).not.toContain('heartbeats:read')
    expect(older).not.toContain('heartbeats:write')
    expect(older).toContain('activity:read')
    expect(older.length).toBe(SKIN_CAP_CHOICES.length - 2)

    h.daemonCliPost.mockImplementation(async (route: string) => {
      if (route === 'skin-tokens') {
        return { id: 'tok-hb', prefix: 'k2skn_hbhb', name: 'kiosk', secret: 'k2skn_HBSECRET' }
      }
      return { ok: true }
    })
    render(<SkinAccessSection />)
    await openHost()
    const read = screen.getByLabelText('Mint cap heartbeats:read') as HTMLInputElement
    const write = screen.getByLabelText('Mint cap heartbeats:write') as HTMLInputElement
    expect(read.checked).toBe(false)
    expect(write.checked).toBe(false)
    expect(read.closest('label')?.getAttribute('title')).toBe(SKIN_CAP_LABELS['heartbeats:read'])
    fireEvent.change(screen.getByLabelText('Platform token name'), { target: { value: 'kiosk' } })
    fireEvent.click(screen.getByLabelText('Mint agent sales'))
    fireEvent.click(read)
    fireEvent.click(write)
    fireEvent.click(screen.getByRole('button', { name: 'Mint key' }))
    await waitFor(() => {
      expect(screen.getByText('k2skn_HBSECRET')).not.toBeNull()
    })
    const mintCall = h.daemonCliPost.mock.calls.find((c) => c[0] === 'skin-tokens')
    expect(mintCall).toBeTruthy()
    const body = mintCall![1] as { caps: string[] }
    expect(body.caps).toEqual(['thread:read', 'thread:post', 'heartbeats:read', 'heartbeats:write'])
  })

  it('offers files read/write checkboxes next to Thread and mints them when checked', async () => {
    h.daemonCliPost.mockImplementation(async (route: string) => {
      if (route === 'skin-tokens') {
        return { id: 'tok-files', prefix: 'k2skn_ffff', name: 'vercel', secret: 'k2skn_FILESECRET' }
      }
      return { ok: true }
    })
    render(<SkinAccessSection />)
    await openHost()
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
    await openHost()
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
    expect(screen.getByRole('alert').textContent).not.toMatch(/front-door/)
    expect(screen.getByRole('alert').textContent).toMatch(/users/)
    expect(screen.getByRole('alert').textContent).toMatch(/keys/)
  })

  it('revokes via skin-tokens/revoke', async () => {
    const confirm = vi.spyOn(window, 'confirm').mockReturnValue(true)
    render(<SkinAccessSection />)
    await openHost()
    fireEvent.click(screen.getByRole('button', { name: 'Revoke' }))
    await waitFor(() => {
      expect(h.daemonCliPost).toHaveBeenCalledWith('skin-tokens/revoke', { id: 'tok-1' })
    })
    confirm.mockRestore()
  })

  it('unsupported Hydra toggle is disabled and does not POST', async () => {
    render(<SkinAccessSection />)
    await openHost()
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
      if (route === 'skin/grants') return { grants: [] }
      if (route === 'publish/list') return { services: [] }
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
    await openHost()
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
      if (route === 'skin/grants') return { grants: [] }
      if (route === 'publish/list') return { services: [] }
      throw new Error(`unexpected GET ${route}`)
    })
    render(<SkinAccessSection />)
    await openHost()
    expect(screen.getByText(/Assign agents or these platform tokens go dark/i)).not.toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }))
    expect(screen.queryByText(/Assign agents or these platform tokens go dark/i)).toBeNull()
    expect(h.daemonCliPost.mock.calls.map((c) => String(c[0])).join(' ')).not.toMatch(/rooms/)
  })

  it('mints disabled until an agent is checked', async () => {
    render(<SkinAccessSection />)
    await openHost()
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
    const wikiReader = {
      id: 'role-wiki',
      name: 'wiki-reader',
      appId: 'svc-wiki',
      roomAccess: [],
    }
    const bob = {
      id: 'p-bob',
      username: 'bob',
      defaultRoomHandles: ['secret-room'],
    }
    h.daemonCliGet.mockImplementation(async (route: string) => {
      if (route === 'skin/users') return { users: [USER_ALICE, bob] }
      if (route === 'skin/roles') return { roles: [dentist, wikiReader] }
      if (route === 'skin-tokens') return { tokens: [KEY_ROW] }
      if (route === 'skin/hydra') return HYDRA_UNSUPPORTED
      if (route === 'projects/list') return [WS_SALES, WS_SUPPORT]
      if (route === 'skin/grants') {
        return {
          grants: [{
            id: 'gh-bob',
            subjectKind: 'principal',
            subjectId: 'p-bob',
            kind: 'app',
            targetId: 'host',
            roleId: 'role-other',
            enabled: true,
          }],
        }
      }
      if (route === 'publish/list') return { services: [] }
      throw new Error(`unexpected GET ${route}`)
    })
    const view = render(<SkinAccessSection />)
    await loaded()
    expect(h.daemonCliGet.mock.calls.map((c) => c[0])).toContain('skin/roles')
    fireEvent.click(screen.getByRole('button', { name: 'Host' }))
    expect(view.container.querySelector('select')).toBeNull()
    expect(screen.queryByRole('combobox')).toBeNull()
    expect(screen.queryByText('role-other')).toBeNull()
    expect(screen.getByText('default rooms: secret-room')).toBeTruthy()
    expect(
      screen.getByText(/Skin roles are not Connect owner\/admin\/member\/viewer/),
    ).not.toBeNull()
    fireEvent.click(screen.getByLabelText('bob login role'))
    const menu = screen.getByTestId('setting-dropdown-menu')
    const optionNames = [...menu.querySelectorAll('button')].map((b) => b.textContent)
    expect(optionNames).toContain('None')
    expect(optionNames).toContain('dentist')
    expect(optionNames).not.toContain('wiki-reader')
    expect(optionNames).not.toContain('owner')
    expect(optionNames).not.toContain('admin')
    expect(optionNames).not.toContain('member')
    expect(optionNames).not.toContain('viewer')
    fireEvent.click(screen.getByRole('button', { name: 'dentist' }))
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
    fireEvent.click(screen.getByRole('button', { name: 'Host' }))
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
    const body = posts[0][1] as { caps?: unknown; rooms?: unknown; appId?: unknown }
    expect(body.caps).toBeUndefined()
    expect(body.rooms).toBeUndefined()
    expect(body.appId).toBeUndefined()
  })

  it('shows tokens and Hydra only after Host, and never Guests or Leftover Caddy', async () => {
    render(<SkinAccessSection />)
    await loaded()
    expect(screen.queryByRole('heading', { name: 'Guests' })).toBeNull()
    expect(screen.queryByRole('heading', { name: 'Leftover Caddy (optional)' })).toBeNull()
    expect(screen.queryByRole('heading', { name: 'Platform tokens' })).toBeNull()
    expect(screen.queryByRole('heading', { name: 'OIDC issuer (Hydra)' })).toBeNull()
    expect(screen.queryByRole('heading', { name: 'Roles' })).toBeNull()
    expect(screen.queryByLabelText('New skin username')).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Host' }))
    expect(screen.getByRole('heading', { name: 'Roles' })).toBeTruthy()
    expect(screen.getByRole('heading', { name: 'Platform tokens' })).toBeTruthy()
    expect(screen.getByRole('heading', { name: 'OIDC issuer (Hydra)' })).toBeTruthy()
    expect(screen.getByLabelText('New skin role name')).toBeTruthy()
    expect(screen.queryByRole('heading', { name: 'Leftover Caddy (optional)' })).toBeNull()
    expect(screen.queryByRole('heading', { name: 'Guests' })).toBeNull()
    expect(screen.queryByLabelText('New skin username')).toBeNull()
  })

  it('lists cmd and skin from each workspace and resolves workspace grants by project id', async () => {
    h.daemonCliGet.mockImplementation(async (route: string, params?: { project?: string }) => {
      if (route === 'skin/front-door') {
        return { mode: 'connect', connectUrl: 'https://skin.acme.k2.dev', subdomain: 'acme' }
      }
      if (route === 'skin/users') return { users: [USER_ALICE, USER_BOB] }
      if (route === 'skin/roles') return { roles: [] }
      if (route === 'skin-tokens') return { tokens: [KEY_ROW] }
      if (route === 'skin/hydra') return HYDRA_UNSUPPORTED
      if (route === 'projects/list') return [WS_SALES, WS_SUPPORT]
      if (route === 'skin/grants') {
        return {
          grants: [
            {
              id: 'gw',
              subjectKind: 'workspace',
              subjectId: 'proj-sales',
              kind: 'app',
              targetId: 'svc-wiki',
              roleId: null,
              enabled: true,
            },
            {
              id: 'gx',
              subjectKind: 'workspace',
              subjectId: 'not-a-project',
              kind: 'app',
              targetId: 'svc-wiki',
              roleId: null,
              enabled: false,
            },
          ],
        }
      }
      if (route === 'publish/list') {
        if (!params?.project) throw new Error('publish/list without project')
        if (params.project === 'proj-sales') {
          return {
            services: [
              { id: 'svc-wiki', projectId: 'proj-sales', name: 'wiki', kind: 'skin', status: 'running', expose: 'local', url: null },
              { id: 'svc-sh', projectId: 'proj-sales', name: 'shell', kind: 'cmd', status: 'stopped', expose: 'public', url: 'https://sh.example' },
              { id: 'svc-boot', projectId: 'proj-sales', name: 'boot', kind: 'skin', status: 'starting', expose: 'public', url: 'https://boot.example' },
              { id: 'svc-old', projectId: 'proj-sales', name: 'old', kind: 'cmd', status: 'exited', expose: 'public', url: 'https://old.example' },
            ],
          }
        }
        if (params.project === 'proj-support') {
          return {
            services: [
              { id: 'svc-mail', projectId: 'proj-support', name: 'mailer', kind: 'skin', status: 'unhealthy', expose: 'public', url: 'https://m.example' },
            ],
          }
        }
        throw new Error(`unexpected project ${params.project}`)
      }
      throw new Error(`unexpected GET ${route}`)
    })
    h.daemonCliPost.mockRejectedValueOnce(new Error("role 'wiki-editor' already exists"))
    render(<SkinAccessSection />)
    await loaded()
    const publishCalls = h.daemonCliGet.mock.calls.filter((c) => c[0] === 'publish/list')
    expect(publishCalls.map((c) => c[1])).toEqual([
      { project: 'proj-sales' },
      { project: 'proj-support' },
    ])
    expect(screen.getByRole('button', { name: 'Open wiki' }).textContent).toMatch(/skin/)
    expect(screen.getByRole('button', { name: 'Open wiki' }).textContent).toMatch(/running/)
    expect(screen.getByRole('button', { name: 'Open wiki' }).textContent).toMatch(/local-only/)
    expect(screen.getByRole('button', { name: 'Open shell' }).textContent).toMatch(/cmd/)
    expect(screen.getByRole('button', { name: 'Open shell' }).textContent).toMatch(/stopped/)
    expect(screen.getByRole('button', { name: 'Open boot' }).textContent).toMatch(/starting/)
    expect(screen.getByRole('button', { name: 'Open old' }).textContent).toMatch(/exited/)
    expect(screen.getByRole('button', { name: 'Open mailer' }).textContent).toMatch(/unhealthy/)
    fireEvent.click(screen.getByRole('button', { name: 'Open wiki' }))
    expect(screen.getByRole('heading', { name: 'Roles' })).toBeTruthy()
    expect(screen.getByText('Sales · workspace')).toBeTruthy()
    expect(screen.getByText('not-a-project · workspace')).toBeTruthy()
    expect(screen.getByRole('switch', { name: 'Enable grant gw' }).getAttribute('aria-checked')).toBe('true')
    fireEvent.change(screen.getByLabelText('New skin role name'), { target: { value: 'wiki-editor' } })
    fireEvent.click(screen.getByRole('button', { name: 'Create role' }))
    await waitFor(() => {
      expect(screen.getByRole('alert').textContent).toMatch(/already exists/)
    })
    expect(h.daemonCliPost).toHaveBeenCalledWith('skin/roles', {
      name: 'wiki-editor',
      roomAccess: [],
      appId: 'svc-wiki',
    })
    fireEvent.click(screen.getByRole('button', { name: 'Start wiki' }))
    await waitFor(() => {
      expect(h.daemonCliPost).toHaveBeenCalledWith('publish/start', {
        name: 'wiki',
        project: 'proj-sales',
      })
    })
  })

  it('adds an existing person to the selected app as a grants row', async () => {
    const ada = { id: 'p-ada', username: 'ada' }
    const wikiReader = {
      id: 'role-wiki',
      name: 'wiki-reader',
      appId: 'svc-wiki',
      roomAccess: [],
    }
    const dentist = { id: 'role-1', name: 'dentist', roomAccess: [] }
    h.daemonCliGet.mockImplementation(async (route: string, params?: { project?: string }) => {
      if (route === 'skin/users') return { users: [ada] }
      if (route === 'skin/roles') return { roles: [wikiReader, dentist] }
      if (route === 'skin-tokens') return { tokens: [] }
      if (route === 'skin/hydra') return HYDRA_UNSUPPORTED
      if (route === 'projects/list') return [WS_SALES]
      if (route === 'skin/grants') return { grants: [] }
      if (route === 'publish/list') {
        if (params?.project !== 'proj-sales') throw new Error(`unexpected project ${params?.project}`)
        return {
          services: [
            { id: 'svc-wiki', projectId: 'proj-sales', name: 'wiki', kind: 'skin', status: 'running', expose: 'local', url: null },
          ],
        }
      }
      throw new Error(`unexpected GET ${route}`)
    })
    render(<SkinAccessSection />)
    await loaded()
    expect(screen.queryByRole('heading', { name: 'Platform tokens' })).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Open wiki' }))
    expect(screen.queryByRole('heading', { name: 'Platform tokens' })).toBeNull()
    expect(screen.queryByRole('heading', { name: 'OIDC issuer (Hydra)' })).toBeNull()
    fireEvent.click(screen.getByLabelText('App user'))
    fireEvent.click(screen.getByRole('button', { name: 'ada' }))
    fireEvent.click(screen.getByLabelText('App user role'))
    const roleMenu = screen.getByTestId('setting-dropdown-menu')
    const roleNames = [...roleMenu.querySelectorAll('button')].map((b) => b.textContent)
    expect(roleNames).toContain('wiki-reader')
    expect(roleNames).not.toContain('dentist')
    fireEvent.click(screen.getByRole('button', { name: 'wiki-reader' }))
    fireEvent.click(screen.getByRole('button', { name: 'Add user' }))
    await waitFor(() => {
      expect(h.daemonCliPost).toHaveBeenCalledWith('skin/grants', {
        subjectKind: 'principal',
        subjectId: 'p-ada',
        kind: 'app',
        targetId: 'svc-wiki',
        roleId: 'role-wiki',
      })
    })
    expect(h.daemonCliPost.mock.calls.map((c) => c[0])).not.toContain('skin/users')
  })
})

describe('TUWA10: Settings → Apps reads the shared cap table', () => {
  it('SKIN_CAP_LABELS equals today’s twelve labels, word for word', () => {
    expect(SKIN_CAP_LABELS).toEqual({
      'thread:read': 'Read the Thread',
      'thread:post': 'Post to the Thread',
      'files:read': 'Read files (also shows heartbeat instructions)',
      'files:write': 'Write files (not .k2/heartbeats)',
      'tickets:read': 'Read tickets and get live ticket updates',
      'tickets:post': 'Open, answer, assign and set status on tickets',
      'wiki:read': 'Read the wiki',
      'store:read': 'Read the workspace store',
      'store:write': 'Change dump-table rows',
      'activity:read': 'See when the agent is working',
      'heartbeats:read': 'See heartbeats: schedule, next fire, history',
      'heartbeats:write': 'Add, edit, turn on or off, archive and fire heartbeats',
    })
  })
})
