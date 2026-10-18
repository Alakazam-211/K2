// 0.46.1: the Clone dialog opened from an agent's clone proposal
// (`k2 clone propose`). Its choices prefill the options; Clone runs ONE
// person confirm that approves the proposal and covers any secret copy
// (the daemon mints both tokens from one check), then approves, runs, and
// reports the run's end to the proposal.

import { describe, it, expect, beforeEach, vi } from 'vitest'

vi.mock('@/stores/projects', () => ({
  useProjectsStore: { getState: () => ({ projects: [], fetchProjects: vi.fn(), setActiveProject: vi.fn() }) },
}))

import { useCloneToDialogStore, type CloneOptionDeps } from './clone-to-dialog'
import { normalizePreview, CloneConfirmCancelled, type ClonePreview } from '@/lib/clone-options'
import {
  applyProposalSecrets,
  cloneProposalConfirm,
  parseProposal,
  proposerName,
  type CloneProposal,
} from '@/lib/clone-proposals'

const flush = (): Promise<void> => new Promise((r) => setTimeout(r, 0))
const TARGET = { recipient: 'age1dest', vault_id: 'vault-d', label: 'luzz.k2.dev' }
const BINDING = { action: 'clone-carry', secretId: 'clone:abc', reason: 'Copy 1 secret', methods: ['vault-password'], allowed: true }

function preview(): ClonePreview {
  return normalizePreview({
    threads: { available: true, conversations: 1, docs: 5 },
    secrets: {
      available: true,
      rows: [
        { name: 'STRIPE_KEY', kind: 'api_key', group: 'workspace', default: 'copy', choice: 'copy', copyable: true },
        { name: 'GMAIL_TOKEN', kind: 'oauth_token', group: 'workspace', default: 'skip', choice: 'skip', copyable: true },
        { name: 'TLS_KEY', kind: 'tls_key', group: 'workspace', default: 'blank', choice: 'blank', copyable: false },
      ],
      notCarried: [],
      confirm: BINDING,
    },
    envFiles: [],
    callerCanCopySecrets: true,
  })
}

function proposal(secrets: Record<string, string>, over: Record<string, unknown> = {}): CloneProposal {
  const p = parseProposal({
    id: 'cp_1',
    projectId: 'p1',
    proposedBy: 'agent:sales/s1',
    target: 'luzz.k2.dev',
    options: { threads: false, sinceDays: 30, secrets, allHistory: false },
    why: 'move to the new box',
    status: 'open',
    expiresAt: 9e9,
    workspaceHandle: 'sales',
    workspacePath: '/w/sales',
    summary: 'clone sales to luzz.k2.dev, …',
    ...over,
  })
  if (!p) throw new Error('proposal did not parse')
  return p
}

type Spied = { [K in keyof CloneOptionDeps]-?: NonNullable<CloneOptionDeps[K]> & ReturnType<typeof vi.fn> }

function deps(over: Partial<CloneOptionDeps> = {}): Spied {
  return {
    lookupTarget: vi.fn(async () => ({ target: TARGET })),
    fetchPreview: vi.fn(async () => preview()),
    carryConfirm: vi.fn(async () => 'k2vc_carry_only'),
    proposalConfirm: vi.fn(async () => ({ token: 'k2vc_carry', approveToken: 'k2vc_approve' })),
    approveProposal: vi.fn(async () => undefined),
    finishProposal: vi.fn(async () => undefined),
    ...over,
  } as unknown as Spied
}

async function open(p: CloneProposal, d: Spied) {
  const onConfirm = vi.fn()
  useCloneToDialogStore.getState().start({ projectPath: '/w/sales', projectName: 'sales', host: null, onConfirm, optionDeps: d, proposal: p })
  await flush()
  await flush()
  return { onConfirm }
}

beforeEach(() => useCloneToDialogStore.getState().close())

describe('clone proposals lib', () => {
  it('parses a daemon row; keeps only real choices; names the proposer', () => {
    const p = proposal({ STRIPE_KEY: 'copy', BAD: 'steal' })
    expect(p.options).toEqual({ threads: false, sinceDays: 30, secrets: { STRIPE_KEY: 'copy' }, allHistory: false, dest: null })
    expect(proposerName(p)).toBe('sales')
    expect(parseProposal({ id: '', target: 'x' })).toBeNull()
    expect(parseProposal(null)).toBeNull()
  })

  it('applies the proposal over the defaults: only rows the plan has, Copy only where allowed', () => {
    expect(
      applyProposalSecrets(
        { STRIPE_KEY: 'copy', GMAIL_TOKEN: 'skip', TLS_KEY: 'blank' },
        { STRIPE_KEY: 'blank', GMAIL_TOKEN: 'copy', TLS_KEY: 'copy', GHOST: 'copy' },
        (n) => n !== 'TLS_KEY',
      ),
    ).toEqual({ STRIPE_KEY: 'blank', GMAIL_TOKEN: 'copy', TLS_KEY: 'blank' })
  })

  it('cloneProposalConfirm: methods from vault/confirm/start, ONE clone/vault/confirm with the proposal', async () => {
    const calls: Array<[string, unknown]> = []
    const post = vi.fn(async (_s: unknown, route: string, body?: unknown) => {
      calls.push([route, body])
      if (route === 'vault/confirm/start') return { methods: ['vault-password'] }
      if (route === 'clone/vault/confirm') return { token: 'k2vc_c', approveToken: 'k2vc_a' }
      throw new Error(`unexpected ${route}`)
    })
    const ask = { clone_id: 'c1', vault_target: TARGET, secrets: { STRIPE_KEY: 'copy' as const } }
    const r = await cloneProposalConfirm(post as never, 'scope' as never, '/w/sales', ask, 'cp_1', 'approve and clone sales', async () => 'pw')
    expect(r).toEqual({ token: 'k2vc_c', approveToken: 'k2vc_a' })
    expect(calls).toEqual([
      ['vault/confirm/start', { action: 'clone-approve' }],
      ['clone/vault/confirm', { project_path: '/w/sales', ...ask, proposal: 'cp_1', method: 'vault-password', password: 'pw' }],
    ])
    // Touch ID when offered (no password asked); cancel is a cancel.
    const ask2 = vi.fn(async () => null)
    const post2 = vi.fn(async (_s: unknown, route: string) =>
      route === 'vault/confirm/start' ? { methods: ['local-auth'] } : { token: null, approveToken: 'k2vc_a2' },
    )
    expect(await cloneProposalConfirm(post2 as never, 's' as never, '/w', { clone_id: 'c' }, 'cp_1', 'x', ask2)).toEqual({ token: null, approveToken: 'k2vc_a2' })
    expect(ask2).not.toHaveBeenCalled()
    const post3 = vi.fn(async () => ({ methods: ['vault-password'] }))
    await expect(cloneProposalConfirm(post3 as never, 's' as never, '/w', { clone_id: 'c' }, 'cp_1', 'x', async () => null)).rejects.toBeInstanceOf(CloneConfirmCancelled)
  })
})

describe('Clone dialog from a proposal', () => {
  it('prefills Thread, history and per-secret choices from the proposal', async () => {
    const d = deps()
    await open(proposal({ STRIPE_KEY: 'blank', GMAIL_TOKEN: 'copy', TLS_KEY: 'copy' }), d)
    const s = useCloneToDialogStore.getState()
    expect(s.proposalId).toBe('cp_1')
    expect(s.bringThread).toBe(false)
    expect(s.sinceDays).toBe(30)
    expect(s.includeAllHistory).toBe(false)
    // TLS_KEY can't be copied: it keeps its default.
    expect(s.secretChoices).toEqual({ STRIPE_KEY: 'blank', GMAIL_TOKEN: 'copy', TLS_KEY: 'blank' })
  })

  it('Clone = ONE confirm for approve + copy, then approve, then the run with the carry token', async () => {
    const d = deps()
    const { onConfirm } = await open(proposal({ STRIPE_KEY: 'copy' }), d)
    await useCloneToDialogStore.getState().confirm()
    expect(d.proposalConfirm).toHaveBeenCalledTimes(1)
    expect(d.carryConfirm).not.toHaveBeenCalled()
    const [ask, , pid] = d.proposalConfirm.mock.calls[0]
    expect(pid).toBe('cp_1')
    expect(ask).toMatchObject({ secrets: { STRIPE_KEY: 'copy' }, vault_target: TARGET })
    expect(d.approveProposal).toHaveBeenCalledWith('cp_1', 'k2vc_approve')
    expect(onConfirm).toHaveBeenCalledTimes(1)
    const [, includeAllHistory, extras, vault] = onConfirm.mock.calls[0]
    expect(includeAllHistory).toBe(false)
    expect(extras).toMatchObject({ threads: 'none', proposal_id: 'cp_1' })
    expect(vault.vaultOptions.confirmToken).toBe('k2vc_carry')
    expect(useCloneToDialogStore.getState().phase).toBe('running')
    // The run's end is reported to the proposal, once.
    useCloneToDialogStore.getState().setDone({ dest_path: '/x' } as never)
    expect(d.finishProposal).toHaveBeenCalledWith('cp_1', true, undefined)
    useCloneToDialogStore.getState().setError('late')
    expect(d.finishProposal).toHaveBeenCalledTimes(1)
  })

  it('nothing to copy: still one confirm (approve only), no carry token on the run', async () => {
    const d = deps({ proposalConfirm: vi.fn(async () => ({ token: null, approveToken: 'k2vc_a' })) as never })
    const { onConfirm } = await open(proposal({ STRIPE_KEY: 'skip' }), d)
    await useCloneToDialogStore.getState().confirm()
    expect(d.proposalConfirm).toHaveBeenCalledTimes(1)
    expect(d.approveProposal).toHaveBeenCalledWith('cp_1', 'k2vc_a')
    expect(onConfirm.mock.calls[0][3].vaultOptions.confirmToken).toBeUndefined()
  })

  it('a failed run is reported as failed', async () => {
    const d = deps()
    await open(proposal({ STRIPE_KEY: 'copy' }), d)
    await useCloneToDialogStore.getState().confirm()
    useCloneToDialogStore.getState().setError('upload failed')
    expect(d.finishProposal).toHaveBeenCalledWith('cp_1', false, 'upload failed')
  })

  it('a refused approve (the clone gate) stays on the options with the reason; nothing runs', async () => {
    const d = deps({ approveProposal: vi.fn(async () => { throw new Error('cloning to personal computers is off') }) as never })
    const { onConfirm } = await open(proposal({ STRIPE_KEY: 'copy' }), d)
    await useCloneToDialogStore.getState().confirm()
    const s = useCloneToDialogStore.getState()
    expect(s.phase).toBe('options')
    expect(s.optionsError).toMatch(/personal computers is off/)
    expect(onConfirm).not.toHaveBeenCalled()
    expect(d.finishProposal).not.toHaveBeenCalled()
  })

  it('a confirm that did not cover the copy is refused before approving', async () => {
    const d = deps({ proposalConfirm: vi.fn(async () => ({ token: null, approveToken: 'k2vc_a' })) as never })
    const { onConfirm } = await open(proposal({ STRIPE_KEY: 'copy' }), d)
    await useCloneToDialogStore.getState().confirm()
    expect(useCloneToDialogStore.getState().optionsError).toMatch(/didn't cover copying secrets/)
    expect(d.approveProposal).not.toHaveBeenCalled()
    expect(onConfirm).not.toHaveBeenCalled()
  })
})
