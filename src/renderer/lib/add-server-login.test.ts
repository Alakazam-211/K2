// verifyHostCredentials — the Settings → Connections add/edit-server entry
// point (PRD connect-login-edge-only D4/W4). Deps are injected so the
// decision is tested without rendering Settings; the URL rule itself is
// loginToHost's (covered in connect-host.rotation.test.ts) — this asserts
// the add path CALLS that same loginToHost and never treats a
// `mustChangePassword` answer as signed in.

import { describe, it, expect, vi } from 'vitest'

const mem = new Map<string, string>()
vi.stubGlobal('localStorage', {
  getItem: (k: string) => (mem.has(k) ? mem.get(k)! : null),
  setItem: (k: string, v: string) => void mem.set(k, v),
  removeItem: (k: string) => void mem.delete(k),
  clear: () => mem.clear(),
})
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))

import { abandonRotation, verifyHostCredentials, type AddServerLoginDeps } from './add-server-login'
import type { ConnectHost, LoginResult } from '@/stores/connect-host'

const HOST: ConnectHost = {
  id: 'new-1',
  label: 'Box',
  hostname: 'box.k2.dev',
  username: 'alice',
  port: 443,
  secure: true,
  token: '',
  remember: true,
  lastConnectedAt: null,
}

function deps(login: LoginResult, role: string | null = 'member'): AddServerLoginDeps & {
  addHost: ReturnType<typeof vi.fn>
  removeHost: ReturnType<typeof vi.fn>
  loginToHost: ReturnType<typeof vi.fn>
  whoamiRole: ReturnType<typeof vi.fn>
  setHostAccess: ReturnType<typeof vi.fn>
} {
  return {
    addHost: vi.fn(() => true),
    removeHost: vi.fn(),
    loginToHost: vi.fn(async () => login),
    whoamiRole: vi.fn(async () => role),
    setHostAccess: vi.fn(),
  }
}

describe('verifyHostCredentials (add-server path)', () => {
  it('adds the host FIRST, then logs in through loginToHost with the typed password', async () => {
    const d = deps({ ok: true, token: 'tok', mustChangePassword: false })
    const out = await verifyHostCredentials(HOST, 'pw', true, d)
    expect(out).toEqual({ kind: 'signed-in', token: 'tok' })
    expect(d.addHost).toHaveBeenCalledWith(HOST)
    expect(d.loginToHost).toHaveBeenCalledWith(HOST, 'pw')
    expect(d.addHost.mock.invocationCallOrder[0]).toBeLessThan(d.loginToHost.mock.invocationCallOrder[0])
    expect(d.removeHost).not.toHaveBeenCalled()
  })

  it('mustChangePassword → rotate with the RESTRICTED token; NOT signed-in; provisional tile kept', async () => {
    const d = deps({ ok: true, token: 'restricted', mustChangePassword: true })
    const out = await verifyHostCredentials(HOST, 'temp', true, d)
    expect(out).toEqual({ kind: 'rotate', host: { ...HOST, token: 'restricted' } })
    expect(d.removeHost).not.toHaveBeenCalled()
  })

  it('failed login on a NEW host removes the provisional tile and surfaces the reason', async () => {
    const d = deps({ ok: false, kind: 'auth', reason: 'Invalid username or password.' })
    const out = await verifyHostCredentials(HOST, 'bad', true, d)
    expect(out).toEqual({ kind: 'error', reason: 'Invalid username or password.' })
    expect(d.removeHost).toHaveBeenCalledWith('new-1')
  })

  // prd-voice-chat-only-members-v1 §8.4: the role is read before the host
  // counts as open; a Voice & Chat only answer is saved with the hint and
  // never reported as signed in (the caller never switches to it).
  it('a voice_chat whoami → voice-chat-only with the hint, never signed-in', async () => {
    const d = deps({ ok: true, token: 'tok', mustChangePassword: false }, 'voice_chat')
    const out = await verifyHostCredentials(HOST, 'pw', true, d)
    expect(out).toEqual({ kind: 'voice-chat-only', token: 'tok' })
    expect(d.whoamiRole).toHaveBeenCalledWith(HOST, 'tok')
    expect(d.setHostAccess).toHaveBeenCalledWith('new-1', 'voice_chat')
    expect(d.removeHost).not.toHaveBeenCalled()
  })

  it('a member whoami clears any old hint and is signed-in', async () => {
    const d = deps({ ok: true, token: 'tok', mustChangePassword: false }, 'member')
    const out = await verifyHostCredentials(HOST, 'pw', false, d)
    expect(out).toEqual({ kind: 'signed-in', token: 'tok' })
    expect(d.setHostAccess).toHaveBeenCalledWith('new-1', 'member')
  })

  it('failed login on an EDIT keeps the existing tile', async () => {
    const d = deps({ ok: false, kind: 'not-found', reason: '404' })
    const out = await verifyHostCredentials(HOST, 'bad', false, d)
    expect(out.kind).toBe('error')
    expect(d.removeHost).not.toHaveBeenCalled()
  })
})

// 0.46.8: logins someone else makes have a temporary password, so the
// rotation step shows on every first sign-in. Cancelling it must not leave
// the RESTRICTED token loginToHost cached in the keychain.
describe('abandonRotation (rotation step cancelled)', () => {
  function abandonDeps(): {
    removeHost: ReturnType<typeof vi.fn>
    clearHostToken: ReturnType<typeof vi.fn>
    forgetToken: ReturnType<typeof vi.fn>
  } {
    return { removeHost: vi.fn(), clearHostToken: vi.fn(), forgetToken: vi.fn(async () => undefined) }
  }

  it('an EDITED host drops the restricted token in memory AND forgets it in the keychain', async () => {
    const d = abandonDeps()
    await abandonRotation('h-1', false, d)
    expect(d.clearHostToken).toHaveBeenCalledWith('h-1')
    expect(d.forgetToken).toHaveBeenCalledWith('h-1')
    expect(d.removeHost).not.toHaveBeenCalled()
  })

  it('a NEW host removes the provisional tile (removeHost forgets its token itself)', async () => {
    const d = abandonDeps()
    await abandonRotation('new-1', true, d)
    expect(d.removeHost).toHaveBeenCalledWith('new-1')
    expect(d.clearHostToken).not.toHaveBeenCalled()
    expect(d.forgetToken).not.toHaveBeenCalled()
  })

  it('the real store: an edited host already tokenless in memory still has its keychain entry deleted', async () => {
    const { invoke } = await import('@tauri-apps/api/core')
    const inv = vi.mocked(invoke)
    const { useConnectHostStore } = await import('@/stores/connect-host')
    // In memory the token is already gone (e.g. an expiry); the keychain
    // may still hold the restricted one.
    useConnectHostStore.getState().addHost({ ...HOST, id: 'edit-1', token: '' })
    inv.mockClear()
    await abandonRotation('edit-1', false)
    const h = useConnectHostStore.getState().hosts.find((x) => x.id === 'edit-1')
    if (!h) throw new Error('host vanished')
    expect(h.token).toBe('')
    const deletes = inv.mock.calls.filter(
      (c) => c[0] === 'k2_secret_delete' && (c[1] as { account?: string }).account === 'edit-1',
    )
    expect(deletes.length).toBeGreaterThan(0)
  })
})
