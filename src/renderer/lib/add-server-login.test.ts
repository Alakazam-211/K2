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

import { verifyHostCredentials, type AddServerLoginDeps } from './add-server-login'
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

function deps(login: LoginResult): AddServerLoginDeps & {
  addHost: ReturnType<typeof vi.fn>
  removeHost: ReturnType<typeof vi.fn>
  loginToHost: ReturnType<typeof vi.fn>
} {
  return {
    addHost: vi.fn(),
    removeHost: vi.fn(),
    loginToHost: vi.fn(async () => login),
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

  it('failed login on an EDIT keeps the existing tile', async () => {
    const d = deps({ ok: false, kind: 'not-found', reason: '404' })
    const out = await verifyHostCredentials(HOST, 'bad', false, d)
    expect(out.kind).toBe('error')
    expect(d.removeHost).not.toHaveBeenCalled()
  })
})
