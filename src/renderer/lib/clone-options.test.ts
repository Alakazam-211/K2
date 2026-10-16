// Clone dialog options (prd-clone-carries-threads-and-vault-v1 §4.1):
// per-secret defaults (D1 Member rule, copyable), the bundle/pack extras,
// and the vault confirm flow. Loud: every branch asserts exact values.

import { describe, it, expect, vi } from 'vitest'

import {
  anyCopy,
  buildCloneExtras,
  CARRY_NOT_READY,
  cloneCarryConfirm,
  CloneConfirmCancelled,
  envConfirmSecretId,
  getVaultConfirmToken,
  initialSecretChoices,
  normalizePreview,
  threadsValue,
  threadSummaryLine,
  type ClonePreview,
} from './clone-options'
import type { ServerScope } from '@/kessel/server-scope'

const SCOPE = 'source' as unknown as ServerScope

function preview(callerCanCopySecrets: boolean): ClonePreview {
  return normalizePreview({
    threads: { available: true, conversations: 3, docs: 412, participants: ['you', 'alice'] },
    secrets: {
      available: true,
      rows: [
        { name: 'STRIPE_KEY', kind: 'api_key', group: 'workspace', default: 'copy', copyable: true },
        { name: 'GMAIL_TOKEN', kind: 'oauth_token', group: 'workspace', default: 'skip', copyable: true },
        { name: 'OPENAI_KEY', kind: 'api_key', group: 'server', default: 'blank', copyable: true },
        { name: 'TLS_KEY', kind: 'tls_key', group: 'workspace', default: 'copy', copyable: false },
      ],
      notCarried: ['1 App secret'],
    },
    envFiles: ['.env'],
    callerCanCopySecrets,
  })
}

describe('initialSecretChoices', () => {
  it('takes the daemon default per row, never Copy on a row that cannot be copied', () => {
    expect(initialSecretChoices(preview(true))).toEqual({
      STRIPE_KEY: 'copy',
      GMAIL_TOKEN: 'skip',
      OPENAI_KEY: 'blank',
      TLS_KEY: 'blank',
    })
  })

  it('a Member (no Owner/Admin confirm) gets Leave blank where the default was Copy (D1)', () => {
    const c = initialSecretChoices(preview(false))
    expect(c).toEqual({ STRIPE_KEY: 'blank', GMAIL_TOKEN: 'skip', OPENAI_KEY: 'blank', TLS_KEY: 'blank' })
    expect(anyCopy(c)).toBe(false)
  })
})

describe('buildCloneExtras / threadsValue', () => {
  it('Thread ticked = all; since = {sinceDays}; unticked = none', () => {
    expect(threadsValue(true, null)).toBe('all')
    expect(threadsValue(true, 30)).toEqual({ sinceDays: 30 })
    expect(threadsValue(false, 90)).toBe('none')
  })

  it('carries names and choices only, plus the proposal id', () => {
    expect(
      buildCloneExtras({ bringThread: true, sinceDays: null, secretChoices: { A: 'copy' }, proposalId: 'cp_9' }),
    ).toEqual({ threads: 'all', secrets: { A: 'copy' }, proposal_id: 'cp_9' })
    expect(buildCloneExtras({ bringThread: false, sinceDays: null, secretChoices: {} })).toEqual({ threads: 'none' })
  })

  it('binds the .env confirm to the exact project path', () => {
    expect(envConfirmSecretId('/Users/rosson/ws')).toBe('env-files:/Users/rosson/ws')
  })
})

describe('normalizePreview', () => {
  it('reads an older daemon (no preview fields) as nothing available', () => {
    const p = normalizePreview({})
    expect(p.threads.available).toBe(false)
    expect(p.secrets).toEqual({ available: false, rows: [], notCarried: [] })
    expect(p.envFiles).toEqual([])
    expect(p.callerCanCopySecrets).toBe(false)
    expect(threadSummaryLine(p.threads)).toBeNull()
  })

  it('summarizes Thread counts', () => {
    expect(threadSummaryLine(preview(true).threads)).toBe('3 conversations, 412 messages')
  })
})

describe('getVaultConfirmToken', () => {
  it('uses Touch ID when the caller has local-auth', async () => {
    const post = vi.fn(async (_s: ServerScope, route: string, _body?: unknown) => {
      if (route === 'vault/confirm/start') return { methods: ['local-auth', 'vault-password'] }
      return { token: 'tok-1' }
    })
    const ask = vi.fn()
    const t = await getVaultConfirmToken(post as never, SCOPE, 'clone-env', 'env-files:/w', '.env files of w', ask)
    expect(t).toBe('tok-1')
    expect(ask).not.toHaveBeenCalled()
    expect(post).toHaveBeenNthCalledWith(1, SCOPE, 'vault/confirm/start', { action: 'clone-env', secretId: 'env-files:/w' })
    expect(post).toHaveBeenNthCalledWith(2, SCOPE, 'vault/confirm/finish', {
      action: 'clone-env',
      secretId: 'env-files:/w',
      method: 'local-auth',
      label: '.env files of w',
    })
  })

  it('asks for the password otherwise, and a cancel never finishes', async () => {
    const post = vi.fn(async (_s: ServerScope, route: string, _body?: unknown) =>
      route === 'vault/confirm/start' ? { methods: ['connect-password'] } : { token: 'tok-2' },
    )
    const t = await getVaultConfirmToken(post as never, SCOPE, 'clone-env', 'id', 'l', async () => 'pw')
    expect(t).toBe('tok-2')
    expect(post.mock.calls[1][2]).toEqual({ action: 'clone-env', secretId: 'id', method: 'connect-password', label: 'l', password: 'pw' })

    post.mockClear()
    await expect(getVaultConfirmToken(post as never, SCOPE, 'clone-env', 'id', 'l', async () => null)).rejects.toBeInstanceOf(
      CloneConfirmCancelled,
    )
    expect(post).toHaveBeenCalledTimes(1)
  })

  it('fails loudly when no confirm method exists', async () => {
    const post = vi.fn(async () => ({ methods: [] }))
    await expect(getVaultConfirmToken(post as never, SCOPE, 'clone-env', 'id', 'l', async () => 'x')).rejects.toThrow(
      /set a vault password/,
    )
  })
})

describe('cloneCarryConfirm (vault slice seam)', () => {
  it('refuses until the vault update wires the plan-bound confirm', async () => {
    await expect(
      cloneCarryConfirm({ post: vi.fn() as never, scope: SCOPE, projectPath: '/w', secrets: { A: 'copy' }, askPassword: async () => null }),
    ).rejects.toThrow(CARRY_NOT_READY)
  })
})
