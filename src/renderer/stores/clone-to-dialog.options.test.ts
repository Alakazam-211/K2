// clone-to-dialog store, 0.46.0 options phase: the destination vault is
// asked first, the preview carries slice A's ask (vault_target + clone id),
// per-secret choices come from the plan, a Member stays read-only, and the
// ONE `clone-carry` confirm is minted for the FINAL choices (Copy rows and
// the `.env` tick, D4) before the run starts.

import { describe, it, expect, beforeEach, vi } from 'vitest'

vi.mock('@/stores/projects', () => ({
  useProjectsStore: { getState: () => ({ projects: [], fetchProjects: vi.fn(), setActiveProject: vi.fn() }) },
}))

import { useCloneToDialogStore, type CloneOptionDeps } from './clone-to-dialog'
import { normalizePreview, MEMBER_NO_COPY_LINE, CloneConfirmCancelled, type ClonePreview } from '@/lib/clone-options'
import type { VaultRecipientLookup } from '@/lib/clone-vault'

const flush = (): Promise<void> => new Promise((r) => setTimeout(r, 0))
const TARGET = { recipient: 'age1dest', vault_id: 'vault-d', label: 'This computer' }
const BINDING = { action: 'clone-carry', secretId: 'clone:abc', reason: 'Copy 1 secret', methods: ['vault-password'], allowed: true }

function preview(callerCanCopySecrets = true, confirm: unknown = BINDING): ClonePreview {
  return normalizePreview({
    threads: { available: true, conversations: 1, docs: 5 },
    secrets: {
      available: true,
      rows: [
        { name: 'STRIPE_KEY', kind: 'api_key', group: 'workspace', default: 'copy', choice: 'copy', copyable: callerCanCopySecrets },
        { name: 'GMAIL_TOKEN', kind: 'oauth_token', group: 'workspace', default: 'skip', choice: 'skip', copyable: callerCanCopySecrets },
      ],
      notCarried: [],
      confirm,
    },
    envFiles: ['.env'],
    callerCanCopySecrets,
  })
}

type Spied = { [K in keyof CloneOptionDeps]: CloneOptionDeps[K] & ReturnType<typeof vi.fn> }

function deps(p: ClonePreview, over: Partial<CloneOptionDeps> = {}, lookup: VaultRecipientLookup = { target: TARGET }): Spied {
  return {
    lookupTarget: vi.fn(async () => lookup),
    fetchPreview: vi.fn(async () => p),
    carryConfirm: vi.fn(async () => 'k2vc_final'),
    ...over,
  } as unknown as Spied
}

async function open(p: ClonePreview, over: Partial<CloneOptionDeps> = {}, lookup?: VaultRecipientLookup) {
  const d = deps(p, over, lookup)
  const onConfirm = vi.fn()
  useCloneToDialogStore.getState().start({ projectPath: '/w', projectName: 'w', host: null, pull: true, onConfirm, optionDeps: d })
  await flush()
  await flush()
  return { d, onConfirm, cloneId: useCloneToDialogStore.getState().cloneId as string }
}

beforeEach(() => useCloneToDialogStore.getState().close())

describe('clone dialog options defaults', () => {
  it('asks the destination vault first, then previews with vault_target + a fresh clone id', async () => {
    const { d, cloneId } = await open(preview())
    expect(cloneId).toMatch(/\S{8,}/)
    expect(d.lookupTarget).toHaveBeenCalledTimes(1)
    expect(d.fetchPreview).toHaveBeenCalledWith({ clone_id: cloneId, vault_target: TARGET }, null)
    const s = useCloneToDialogStore.getState()
    expect(s.bringThread).toBe(true)
    expect(s.carrySecrets).toBe(false)
    expect(s.ownBoxMove).toBe(false)
    expect(s.previewLoading).toBe(false)
    expect(s.secretChoices).toEqual({ STRIPE_KEY: 'copy', GMAIL_TOKEN: 'skip' })
  })

  it('an older destination (a note) clones without secrets: no choices, preview without a target', async () => {
    const { d, cloneId } = await open(preview(), {}, { note: 'older K2' })
    expect(d.fetchPreview).toHaveBeenCalledWith({ clone_id: cloneId }, null)
    expect(useCloneToDialogStore.getState().secretChoices).toEqual({})
    expect(useCloneToDialogStore.getState().vaultLookup).toEqual({ note: 'older K2' })
  })

  it('a Member never gets Copy, even by picking it', async () => {
    await open(preview(false))
    expect(useCloneToDialogStore.getState().secretChoices.STRIPE_KEY).toBe('blank')
    useCloneToDialogStore.getState().setSecretChoice('STRIPE_KEY', 'copy')
    expect(useCloneToDialogStore.getState().secretChoices.STRIPE_KEY).toBe('blank')
    useCloneToDialogStore.getState().setSecretChoice('STRIPE_KEY', 'skip')
    expect(useCloneToDialogStore.getState().secretChoices.STRIPE_KEY).toBe('skip')
  })

  it('a preview refusal (clone gate) is kept verbatim', async () => {
    const hint = 'cloning isn\'t allowed for "mo" here'
    await open(preview(), { fetchPreview: vi.fn(async () => Promise.reject(new Error(hint))) })
    expect(useCloneToDialogStore.getState().previewError).toBe(hint)
  })
})

describe('clone dialog confirm', () => {
  it('no Copy and no .env files: starts at once; extras hold only threads + clone id', async () => {
    const { d, onConfirm, cloneId } = await open(preview())
    useCloneToDialogStore.getState().setSecretChoice('STRIPE_KEY', 'blank')
    useCloneToDialogStore.getState().setSinceDays(30)
    await useCloneToDialogStore.getState().confirm()
    expect(d.carryConfirm).not.toHaveBeenCalled()
    expect(onConfirm).toHaveBeenCalledWith(
      false,
      true,
      { threads: { sinceDays: 30 }, clone_id: cloneId },
      { carryVault: true, vaultOptions: { target: TARGET, cloneId, secrets: { STRIPE_KEY: 'blank', GMAIL_TOKEN: 'skip' } } },
    )
    expect(useCloneToDialogStore.getState().phase).toBe('running')
  })

  it('a Copy row: the plan is re-read for the FINAL ask and the clone-carry token rides the vault options', async () => {
    const { d, onConfirm, cloneId } = await open(preview())
    useCloneToDialogStore.getState().setOwnBoxMove(true)
    await useCloneToDialogStore.getState().confirm()
    const finalAsk = {
      clone_id: cloneId,
      vault_target: TARGET,
      secrets: { STRIPE_KEY: 'copy', GMAIL_TOKEN: 'skip' },
      own_box_move: true,
    }
    expect(d.fetchPreview).toHaveBeenLastCalledWith(finalAsk, null)
    expect(d.carryConfirm).toHaveBeenCalledWith(finalAsk, ['vault-password'], expect.any(Function))
    expect(onConfirm.mock.calls[0][2]).toEqual({ threads: 'all', clone_id: cloneId })
    expect(onConfirm.mock.calls[0][3]).toEqual({
      carryVault: true,
      vaultOptions: {
        target: TARGET,
        cloneId,
        secrets: { STRIPE_KEY: 'copy', GMAIL_TOKEN: 'skip' },
        confirmToken: 'k2vc_final',
        ownBoxMove: true,
      },
    })
  })

  it('.env files need the same clone-carry confirm (D4); with no destination vault the token rides extras', async () => {
    const { d, onConfirm, cloneId } = await open(preview(), {}, { note: 'older K2' })
    useCloneToDialogStore.getState().setCarrySecrets(true)
    useCloneToDialogStore.getState().setBringThread(false)
    await useCloneToDialogStore.getState().confirm()
    expect(d.carryConfirm).toHaveBeenCalledWith({ clone_id: cloneId, send_env_files: true }, ['vault-password'], expect.any(Function))
    expect(onConfirm).toHaveBeenCalledWith(
      true,
      true,
      { threads: 'none', clone_id: cloneId, confirm_token: 'k2vc_final' },
      { carryVault: false, vaultOptions: {} },
    )
  })

  it('a caller the source says may not confirm (a Member) stays on options with the reason', async () => {
    const { d, onConfirm } = await open(preview(true, { ...BINDING, allowed: false }))
    await useCloneToDialogStore.getState().confirm()
    expect(d.carryConfirm).not.toHaveBeenCalled()
    expect(onConfirm).not.toHaveBeenCalled()
    const s = useCloneToDialogStore.getState()
    expect(s.phase).toBe('options')
    expect(s.optionsError).toBe(MEMBER_NO_COPY_LINE)
  })

  it('a password confirm waits on the dialog field; cancel keeps the options open with no error', async () => {
    const { onConfirm } = await open(preview(), {
      carryConfirm: vi.fn(async (_ask, _methods, askPassword) => {
        const pw = await askPassword('vault-password', 'clone w')
        if (pw === null) throw new CloneConfirmCancelled()
        return 'k2vc_final'
      }),
    })
    const running = useCloneToDialogStore.getState().confirm()
    await flush()
    await flush()
    expect(useCloneToDialogStore.getState().passwordPrompt).toEqual({ method: 'vault-password', label: 'clone w' })
    useCloneToDialogStore.getState().answerPassword(null)
    await running
    expect(onConfirm).not.toHaveBeenCalled()
    expect(useCloneToDialogStore.getState().optionsError).toBeNull()
    expect(useCloneToDialogStore.getState().passwordPrompt).toBeNull()
  })
})
