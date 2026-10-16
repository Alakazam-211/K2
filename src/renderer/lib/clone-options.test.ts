// Clone dialog options (prd-clone-carries-threads-and-vault-v1 §4.1):
// per-secret choices from slice A's plan (D1 Member rule, copyable), the
// non-vault bundle/pack extras, slice A's ask, and the one `clone-carry`
// confirm. Loud: every branch asserts exact values.

import { describe, it, expect, vi } from 'vitest'

import {
  anyCopy,
  buildCarryAsk,
  buildCloneExtras,
  CloneConfirmCancelled,
  cloneVaultConfirm,
  fetchClonePreview,
  initialSecretChoices,
  needsCarryConfirm,
  normalizePreview,
  threadsValue,
  threadSummaryLine,
  type ClonePreview,
} from './clone-options'
import type { ServerScope } from '@/kessel/server-scope'

const SCOPE = 'source' as unknown as ServerScope
const TARGET = { recipient: 'age1dest', vault_id: 'vault-d', label: 'luzz.k2.dev' }

function preview(callerCanCopySecrets: boolean): ClonePreview {
  return normalizePreview({
    threads: { available: true, conversations: 3, docs: 412, participants: ['you', 'alice'], roles: { canonical: 1, sidecar: 2 } },
    secrets: {
      available: true,
      rows: [
        { name: 'STRIPE_KEY', kind: 'api_key', group: 'workspace', default: 'copy', choice: 'copy', copyable: true },
        { name: 'GMAIL_TOKEN', kind: 'oauth_token', group: 'workspace', default: 'skip', choice: 'skip', copyable: true },
        { name: 'OPENAI_KEY', kind: 'api_key', group: 'server', default: 'blank', choice: 'blank', copyable: true },
        { name: 'TLS_KEY', kind: 'tls_key', group: 'workspace', default: 'copy', copyable: false },
      ],
      notCarried: ['APP_PASS (login)'],
      confirm: null,
    },
    envFiles: ['.env'],
    callerCanCopySecrets,
  })
}

describe('initialSecretChoices', () => {
  it("takes the plan's choice per row, never Copy on a row that cannot be copied", () => {
    expect(initialSecretChoices(preview(true))).toEqual({
      STRIPE_KEY: 'copy',
      GMAIL_TOKEN: 'skip',
      OPENAI_KEY: 'blank',
      TLS_KEY: 'blank',
    })
  })

  it('a Member (no Owner/Admin confirm) gets Leave blank where the plan said Copy (D1)', () => {
    const c = initialSecretChoices(preview(false))
    expect(c).toEqual({ STRIPE_KEY: 'blank', GMAIL_TOKEN: 'skip', OPENAI_KEY: 'blank', TLS_KEY: 'blank' })
    expect(anyCopy(c)).toBe(false)
  })
})

describe('extras, ask and the confirm rule', () => {
  it('threads: ticked = all, a since choice, unticked = none', () => {
    expect(threadsValue(true, null)).toBe('all')
    expect(threadsValue(true, 30)).toEqual({ sinceDays: 30 })
    expect(threadsValue(false, 30)).toBe('none')
  })

  it('extras carry only threads, the clone id and a proposal id', () => {
    expect(buildCloneExtras({ bringThread: true, sinceDays: 90, cloneId: 'c1', proposalId: 'cp_9' })).toEqual({
      threads: { sinceDays: 90 },
      clone_id: 'c1',
      proposal_id: 'cp_9',
    })
    expect(buildCloneExtras({ bringThread: false, sinceDays: null })).toEqual({ threads: 'none' })
  })

  it("slice A's ask holds the target, clone id, final choices, own-box and .env flags", () => {
    expect(
      buildCarryAsk({ target: TARGET, cloneId: 'c1', secretChoices: { A: 'copy' }, ownBoxMove: true, sendEnvFiles: true }),
    ).toEqual({ vault_target: TARGET, clone_id: 'c1', secrets: { A: 'copy' }, own_box_move: true, send_env_files: true })
    expect(buildCarryAsk({ target: null, cloneId: 'c1', secretChoices: {}, ownBoxMove: false, sendEnvFiles: false })).toEqual({
      clone_id: 'c1',
    })
  })

  it('a Copy row or the .env tick needs the confirm; blank/skip alone does not', () => {
    expect(needsCarryConfirm({ A: 'blank', B: 'skip' }, false)).toBe(false)
    expect(needsCarryConfirm({ A: 'copy' }, false)).toBe(true)
    expect(needsCarryConfirm({}, true)).toBe(true)
  })

  it('the Thread line names the main Chat and sidecars', () => {
    expect(threadSummaryLine(preview(true).threads)).toBe('1 main Chat, 2 sidecars, 412 messages')
    expect(threadSummaryLine({ available: false })).toBeNull()
  })
})

describe('fetchClonePreview', () => {
  it('posts the source path, the ask and sinceDays to clone/preview', async () => {
    const post = vi.fn(async () => ({ threads: { available: false }, secrets: { available: false } }))
    const p = await fetchClonePreview(post as never, SCOPE, '/w/sales', { clone_id: 'c1', vault_target: TARGET }, 30)
    expect(post).toHaveBeenCalledWith(SCOPE, 'clone/preview', {
      project_path: '/w/sales',
      sinceDays: 30,
      clone_id: 'c1',
      vault_target: TARGET,
    })
    expect(p.secrets).toEqual({ available: false, rows: [], notCarried: [], confirm: null })
    expect(p.envFiles).toEqual([])
  })
})

describe('cloneVaultConfirm (clone-carry for the final ask)', () => {
  const ask = { clone_id: 'c1', vault_target: TARGET, secrets: { STRIPE_KEY: 'copy' as const } }

  it('Touch ID when the source offers local-auth (no password asked)', async () => {
    const post = vi.fn(async () => ({ token: 'k2vc_tid' }))
    const askPw = vi.fn()
    const tok = await cloneVaultConfirm(post as never, SCOPE, '/w/sales', ask, ['local-auth', 'vault-password'], 'clone sales', askPw)
    expect(tok).toBe('k2vc_tid')
    expect(askPw).not.toHaveBeenCalled()
    expect(post).toHaveBeenCalledWith(SCOPE, 'clone/vault/confirm', {
      project_path: '/w/sales',
      ...ask,
      method: 'local-auth',
    })
  })

  it('a password method asks the person and sends the password once', async () => {
    const post = vi.fn(async () => ({ token: 'k2vc_pw' }))
    const tok = await cloneVaultConfirm(post as never, SCOPE, '/w/sales', ask, ['connect-password'], 'clone sales', async () => 'hunter2')
    expect(tok).toBe('k2vc_pw')
    expect(post).toHaveBeenCalledWith(SCOPE, 'clone/vault/confirm', {
      project_path: '/w/sales',
      ...ask,
      method: 'connect-password',
      password: 'hunter2',
    })
  })

  it('cancel is a CloneConfirmCancelled; no method is a teaching error', async () => {
    const post = vi.fn()
    await expect(
      cloneVaultConfirm(post as never, SCOPE, '/w/sales', ask, ['vault-password'], 'x', async () => null),
    ).rejects.toBeInstanceOf(CloneConfirmCancelled)
    await expect(cloneVaultConfirm(post as never, SCOPE, '/w/sales', ask, [], 'x', async () => 'p')).rejects.toThrow(
      /set a vault password first/,
    )
    expect(post).not.toHaveBeenCalled()
  })
})
