// 0.45.7 Settings → K2 Connect → URLs: add / repoint / remove published
// subdomains. Covers the label + target rules, the preview host, the exact
// control-plane request bodies (same as cli/k2 `_connect_curl`), the local
// daemon attribution + refresh calls around them, and the CLI's error text.

import { describe, it, expect, beforeEach, vi } from 'vitest'

const { postMock, postQueryMock } = vi.hoisted(() => ({
  postMock: vi.fn(),
  postQueryMock: vi.fn(),
}))

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliPost: (...a: unknown[]) => postMock(...a),
  daemonCliPostQuery: (...a: unknown[]) => postQueryMock(...a),
}))

import {
  CONNECT_PROXY_ROUTE,
  PROXY_MISSING_MESSAGE,
  PublishedSubdomainError,
  canManagePublishedSubdomains,
  controlPlaneErrorMessage,
  createRequest,
  previewHost,
  proxyFailureMessage,
  publishedSubdomains,
  removeRequest,
  updateTargetRequest,
  validateLabel,
  validateTarget,
} from './published-subdomains'
import type { ServerScope } from '@/kessel/server-scope'

const scope = { connectionKey: 'local', isRemote: false } as unknown as ServerScope

function okProxy(body: unknown = {}): { status: number; body: unknown } {
  return { status: 200, body }
}

beforeEach(() => {
  postMock.mockReset()
  postQueryMock.mockReset()
})

describe('validateLabel', () => {
  it('accepts clean DNS labels', () => {
    for (const l of ['staging', 'my-app', 'app2', 'a1b', 'x'.repeat(63)]) {
      expect(validateLabel(l)).toEqual({ ok: true, value: l })
    }
  })

  it('trims surrounding space', () => {
    expect(validateLabel('  staging ')).toEqual({ ok: true, value: 'staging' })
  })

  it('refuses bad shapes with a reason', () => {
    const bad: Array<[string, RegExp]> = [
      ['', /Enter a label/],
      ['Staging', /lowercase/],
      ['my_app', /letters, digits and dashes/],
      ['my app', /letters, digits and dashes/],
      ['my.app', /letters, digits and dashes/],
      ['-app', /start or end with a dash/],
      ['app-', /start or end with a dash/],
      ['my--app', /two dashes/],
      ['ab', /at least 3/],
      ['x'.repeat(64), /at most 63/],
      ['12345', /only digits/],
    ]
    for (const [label, why] of bad) {
      const r = validateLabel(label)
      expect(r.ok, label).toBe(false)
      if (r.ok) throw new Error(`expected ${label} to be refused`)
      expect(r.error, label).toMatch(why)
    }
  })

  it('refuses the reserved front doors and control-plane names', () => {
    for (const l of ['mail', 'meet', 'chat', 'skin', 'cert', 'connect']) {
      const r = validateLabel(l)
      if (r.ok) throw new Error(`expected ${l} to be reserved`)
      expect(r.error).toMatch(/reserved/)
    }
  })
})

describe('validateTarget', () => {
  it('accepts host:port', () => {
    expect(validateTarget('localhost:3000')).toEqual({ ok: true, value: 'localhost:3000' })
    expect(validateTarget('127.0.0.1:8080')).toEqual({ ok: true, value: '127.0.0.1:8080' })
    expect(validateTarget('[::1]:443')).toEqual({ ok: true, value: '[::1]:443' })
  })

  it('refuses bad targets', () => {
    const bad: Array<[string, RegExp]> = [
      ['', /Enter a target/],
      ['localhost', /host:port/],
      ['http://localhost:3000', /http:\/\//],
      ['localhost:abc', /not a port/],
      ['localhost:0', /between 1 and 65535/],
      ['localhost:70000', /between 1 and 65535/],
      [':3000', /host:port/],
      ['local host:3000', /spaces/],
    ]
    for (const [t, why] of bad) {
      const r = validateTarget(t)
      if (r.ok) throw new Error(`expected ${t} to be refused`)
      expect(r.error, t).toMatch(why)
    }
  })
})

describe('previewHost', () => {
  it('uses the primary label under k2.dev', () => {
    expect(previewHost('staging', 'rosson', null)).toBe('staging.rosson.k2.dev')
  })

  it('prefers the running public URL host', () => {
    expect(previewHost('staging', 'rosson', 'https://rosson.k2.dev/')).toBe('staging.rosson.k2.dev')
  })

  it('shows placeholders for what is not known yet', () => {
    expect(previewHost('', 'rosson', null)).toBe('<label>.rosson.k2.dev')
    expect(previewHost('staging', '', null)).toBe('staging.<your subdomain>.k2.dev')
  })

  it('lowercases what was typed', () => {
    expect(previewHost('Staging', 'rosson', null)).toBe('staging.rosson.k2.dev')
  })
})

describe('control-plane request bodies (same as cli/k2)', () => {
  it('create = POST /subdomains {label, target}', () => {
    expect(createRequest('staging', 'localhost:3000')).toEqual({
      method: 'POST',
      endpoint: '/subdomains',
      body: { label: 'staging', target: 'localhost:3000' },
    })
  })

  it('point = PUT /subdomains/<label> {target}', () => {
    expect(updateTargetRequest('staging', 'localhost:4000')).toEqual({
      method: 'PUT',
      endpoint: '/subdomains/staging',
      body: { target: 'localhost:4000' },
    })
  })

  it('rm = DELETE /subdomains/<label> with no body', () => {
    const r = removeRequest('staging')
    expect(r).toEqual({ method: 'DELETE', endpoint: '/subdomains/staging' })
    expect('body' in r).toBe(false)
  })
})

describe('publishedSubdomains(scope)', () => {
  it('create with no workspace: proxy POST then refresh, no claim', async () => {
    postMock.mockResolvedValueOnce(okProxy({ host: 'staging.rosson.k2.dev' }))
    postQueryMock.mockResolvedValueOnce({ success: true })
    const r = await publishedSubdomains(scope).create('staging', 'localhost:3000')
    expect(r).toEqual({ refreshed: true, warning: null })
    expect(postMock).toHaveBeenCalledTimes(1)
    expect(postMock).toHaveBeenCalledWith(scope, CONNECT_PROXY_ROUTE, {
      method: 'POST',
      endpoint: '/subdomains',
      body: { label: 'staging', target: 'localhost:3000' },
    })
    expect(postQueryMock.mock.calls).toEqual([[scope, 'tunnel/subdomains/refresh']])
  })

  it('create for a workspace: check, POST, stamp, refresh — in that order', async () => {
    const order: string[] = []
    postQueryMock.mockImplementation(async (_s: unknown, route: string, params?: Record<string, unknown>) => {
      order.push(`${route}${params?.check ? '?check' : ''}`)
      return { success: true }
    })
    postMock.mockImplementation(async () => {
      order.push('proxy')
      return okProxy()
    })
    await publishedSubdomains(scope).create('staging', 'localhost:3000', { projectId: 'p1' })
    expect(order).toEqual([
      'publish/subdomain/claim?check',
      'proxy',
      'publish/subdomain/claim',
      'tunnel/subdomains/refresh',
    ])
    expect(postQueryMock.mock.calls[0]).toEqual([
      scope,
      'publish/subdomain/claim',
      { label: 'staging', project: 'p1', check: 1 },
    ])
    expect(postQueryMock.mock.calls[1]).toEqual([
      scope,
      'publish/subdomain/claim',
      { label: 'staging', project: 'p1', target: 'localhost:3000' },
    ])
  })

  it('create refused by the workspace check never calls the control plane', async () => {
    postQueryMock.mockRejectedValueOnce(new Error('label_owned_by_other_workspace'))
    await expect(
      publishedSubdomains(scope).create('staging', 'localhost:3000', { projectId: 'p1' }),
    ).rejects.toThrow('label_owned_by_other_workspace')
    expect(postMock).not.toHaveBeenCalled()
  })

  it('create refuses an invalid label before any call', async () => {
    await expect(publishedSubdomains(scope).create('mail', 'localhost:3000')).rejects.toThrow(/reserved/)
    await expect(publishedSubdomains(scope).create('ok-label', 'nope')).rejects.toThrow(/host:port/)
    expect(postMock).not.toHaveBeenCalled()
    expect(postQueryMock).not.toHaveBeenCalled()
  })

  it('a failed stamp after a good create is a warning, not an error', async () => {
    postQueryMock
      .mockResolvedValueOnce({ success: true }) // check
      .mockRejectedValueOnce(new Error('daemon busy')) // stamp
      .mockResolvedValueOnce({ success: true }) // refresh
    postMock.mockResolvedValueOnce(okProxy())
    const r = await publishedSubdomains(scope).create('staging', 'localhost:3000', { projectId: 'p1' })
    expect(r.refreshed).toBe(true)
    expect(r.warning).toMatch(/daemon busy/)
    expect(r.warning).toMatch(/k2 publish subdomain claim staging/)
  })

  it('a failed refresh still succeeds, with refreshed=false', async () => {
    postMock.mockResolvedValueOnce(okProxy())
    postQueryMock.mockRejectedValueOnce(new Error('tunnel not configured'))
    const r = await publishedSubdomains(scope).create('staging', 'localhost:3000')
    expect(r).toEqual({ refreshed: false, warning: null })
  })

  it('updateTarget = PUT with {target}, stamps the new target for an owned label', async () => {
    postMock.mockResolvedValueOnce(okProxy())
    postQueryMock.mockResolvedValue({ success: true })
    await publishedSubdomains(scope).updateTarget('staging', 'localhost:4000', { projectId: 'p1' })
    expect(postMock).toHaveBeenCalledWith(scope, CONNECT_PROXY_ROUTE, {
      method: 'PUT',
      endpoint: '/subdomains/staging',
      body: { target: 'localhost:4000' },
    })
    expect(postQueryMock.mock.calls).toEqual([
      [scope, 'publish/subdomain/claim', { label: 'staging', project: 'p1', check: 1 }],
      [scope, 'publish/subdomain/claim', { label: 'staging', project: 'p1', target: 'localhost:4000' }],
      [scope, 'tunnel/subdomains/refresh'],
    ])
  })

  it('updateTarget for an unowned label skips the claim calls', async () => {
    postMock.mockResolvedValueOnce(okProxy())
    postQueryMock.mockResolvedValueOnce({ success: true })
    await publishedSubdomains(scope).updateTarget('staging', 'localhost:4000', { projectId: null })
    expect(postQueryMock.mock.calls).toEqual([[scope, 'tunnel/subdomains/refresh']])
  })

  it('updateTarget refuses a reserved label (like `k2 publish subdomain point`)', async () => {
    await expect(publishedSubdomains(scope).updateTarget('chat', 'localhost:4000')).rejects.toThrow(/reserved/)
    expect(postMock).not.toHaveBeenCalled()
  })

  it('remove = DELETE with no body, then unclaim, then refresh', async () => {
    postMock.mockResolvedValueOnce(okProxy({ removed: 'staging' }))
    postQueryMock.mockResolvedValue({ success: true })
    const r = await publishedSubdomains(scope).remove('staging', { projectId: 'p1' })
    expect(r).toEqual({ refreshed: true, warning: null })
    expect(postMock).toHaveBeenCalledWith(scope, CONNECT_PROXY_ROUTE, {
      method: 'DELETE',
      endpoint: '/subdomains/staging',
    })
    expect(postQueryMock.mock.calls).toEqual([
      [scope, 'publish/subdomain/unclaim', { label: 'staging', project: 'p1' }],
      [scope, 'tunnel/subdomains/refresh'],
    ])
  })

  it('remove works for a reserved leftover label', async () => {
    postMock.mockResolvedValueOnce(okProxy({ removed: 'meet' }))
    postQueryMock.mockResolvedValueOnce({ success: true })
    await publishedSubdomains(scope).remove('meet')
    expect(postMock).toHaveBeenCalledWith(scope, CONNECT_PROXY_ROUTE, {
      method: 'DELETE',
      endpoint: '/subdomains/meet',
    })
  })

  it('a control-plane error becomes the CLI message and skips the follow-ups', async () => {
    postMock.mockResolvedValueOnce({ status: 409, body: { error: 'label_taken', detail: 'staging' } })
    const err = await publishedSubdomains(scope)
      .create('staging', 'localhost:3000')
      .then(() => null, (e: unknown) => e)
    if (!(err instanceof PublishedSubdomainError)) throw new Error('expected a PublishedSubdomainError')
    expect(err.message).toBe('That label is already taken. (staging)')
    expect(err.code).toBe('label_taken')
    expect(postQueryMock).not.toHaveBeenCalled()
  })

  it('an older daemon without the proxy route says so', async () => {
    postMock.mockRejectedValueOnce(new Error('route not found'))
    await expect(publishedSubdomains(scope).remove('staging')).rejects.toThrow(PROXY_MISSING_MESSAGE)
  })

  it('an unreadable proxy answer is an error, not a success', async () => {
    postMock.mockResolvedValueOnce('not json')
    await expect(publishedSubdomains(scope).remove('staging')).rejects.toThrow(/could not read/)
  })
})

describe('controlPlaneErrorMessage (cli/k2 table)', () => {
  it('maps every code the CLI maps', () => {
    expect(controlPlaneErrorMessage(400, { error: 'bad_label' })).toBe(
      'Invalid label (use lowercase letters, digits, and dashes).',
    )
    expect(controlPlaneErrorMessage(400, { error: 'bad_target' })).toBe(
      'Invalid target endpoint (e.g. localhost:3000).',
    )
    expect(controlPlaneErrorMessage(409, { error: 'label_taken' })).toBe('That label is already taken.')
    expect(controlPlaneErrorMessage(400, { error: 'label_is_primary' })).toBe(
      'That label is your primary subdomain — it cannot be reassigned here.',
    )
    expect(controlPlaneErrorMessage(400, { error: 'primary_undeletable' })).toBe(
      'Your primary subdomain cannot be removed.',
    )
    expect(controlPlaneErrorMessage(404, { error: 'not_found' })).toBe('No such subdomain.')
    expect(controlPlaneErrorMessage(401, { error: 'invalid_token' })).toBe(
      'Tunnel token rejected — re-select your subdomain in Settings -> K2 Connect to refresh it.',
    )
  })

  it('appends the detail to a known code', () => {
    expect(controlPlaneErrorMessage(400, { error: 'bad_label', detail: 'too short' })).toBe(
      'Invalid label (use lowercase letters, digits, and dashes). (too short)',
    )
  })

  it('pro_required is the plan upsell', () => {
    expect(controlPlaneErrorMessage(403, { error: 'pro_required', detail: 'rosson' })).toBe(
      'rosson needs the Pro plan to create nested subdomains. Upgrade your plan on the dashboard: https://k2.dev/dashboard',
    )
    expect(controlPlaneErrorMessage(403, { error: 'pro_required' })).toMatch(/^This subdomain needs the Pro plan/)
  })

  it('falls back like the CLI', () => {
    expect(controlPlaneErrorMessage(401, {})).toBe('Unauthorized — your tunnel token is missing or expired.')
    expect(controlPlaneErrorMessage(400, { error: 'reserved_label', detail: 'skin needs loopback' })).toBe(
      'Control plane error (reserved_label): skin needs loopback',
    )
    expect(controlPlaneErrorMessage(502, 'Bad Gateway')).toBe('Control plane returned HTTP 502: Bad Gateway')
    expect(controlPlaneErrorMessage(500, '')).toBe('Control plane returned HTTP 500')
  })

  it('proxyFailureMessage maps a passed-through code', () => {
    expect(proxyFailureMessage(new Error('label_taken'))).toBe('That label is already taken.')
    expect(proxyFailureMessage(new Error('pro_required'))).toMatch(/needs the Pro plan/)
    expect(proxyFailureMessage(new Error('something else'))).toBe('something else')
  })
})

describe('canManagePublishedSubdomains', () => {
  it('local owner and admin only', () => {
    expect(canManagePublishedSubdomains(false, 'owner')).toBe(true)
    expect(canManagePublishedSubdomains(false, 'admin')).toBe(true)
    expect(canManagePublishedSubdomains(false, 'member')).toBe(false)
    expect(canManagePublishedSubdomains(false, null)).toBe(false)
  })

  it('never on a remote host, even for its owner', () => {
    expect(canManagePublishedSubdomains(true, 'owner')).toBe(false)
    expect(canManagePublishedSubdomains(true, 'admin')).toBe(false)
  })
})
