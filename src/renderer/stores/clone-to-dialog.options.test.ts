// clone-to-dialog store, 0.46.0 options phase: preview defaults (Thread
// ticked, per-secret defaults), Member read-only, and the person confirms
// a clone needs before it starts (D4 `.env` files, Copy rows).

import { describe, it, expect, beforeEach, vi } from 'vitest'

vi.mock('@/stores/projects', () => ({
  useProjectsStore: { getState: () => ({ projects: [], fetchProjects: vi.fn(), setActiveProject: vi.fn() }) },
}))

import { useCloneToDialogStore, type CloneOptionDeps } from './clone-to-dialog'
import { normalizePreview, CARRY_NOT_READY, type ClonePreview } from '@/lib/clone-options'

const flush = (): Promise<void> => new Promise((r) => setTimeout(r, 0))

function preview(callerCanCopySecrets = true): ClonePreview {
  return normalizePreview({
    threads: { available: true, conversations: 1, docs: 5 },
    secrets: {
      available: true,
      rows: [
        { name: 'STRIPE_KEY', kind: 'api_key', group: 'workspace', default: 'copy', copyable: true },
        { name: 'GMAIL_TOKEN', kind: 'oauth_token', group: 'workspace', default: 'skip', copyable: true },
      ],
      notCarried: [],
    },
    envFiles: ['.env'],
    callerCanCopySecrets,
  })
}

type Spied = { [K in keyof CloneOptionDeps]: CloneOptionDeps[K] & ReturnType<typeof vi.fn> }

function deps(p: ClonePreview, over: Partial<CloneOptionDeps> = {}): Spied {
  return {
    fetchPreview: vi.fn(async () => p),
    envConfirm: vi.fn(async () => 'env-tok'),
    carryConfirm: vi.fn(async () => 'carry-tok'),
    ...over,
  } as unknown as Spied
}

async function open(p: ClonePreview, over: Partial<CloneOptionDeps> = {}) {
  const d = deps(p, over)
  const onConfirm = vi.fn()
  useCloneToDialogStore.getState().start({ projectPath: '/w', projectName: 'w', host: null, pull: true, onConfirm, optionDeps: d })
  await flush()
  return { d, onConfirm }
}

beforeEach(() => useCloneToDialogStore.getState().close())

describe('clone dialog options defaults', () => {
  it('Thread ticked, .env files off, per-secret defaults from the preview', async () => {
    await open(preview())
    const s = useCloneToDialogStore.getState()
    expect(s.bringThread).toBe(true)
    expect(s.sinceDays).toBeNull()
    expect(s.carrySecrets).toBe(false)
    expect(s.previewLoading).toBe(false)
    expect(s.secretChoices).toEqual({ STRIPE_KEY: 'copy', GMAIL_TOKEN: 'skip' })
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
  it('no Copy and no .env files: starts at once with the Thread + secret choices', async () => {
    const { d, onConfirm } = await open(preview())
    useCloneToDialogStore.getState().setSecretChoice('STRIPE_KEY', 'blank')
    useCloneToDialogStore.getState().setSinceDays(30)
    await useCloneToDialogStore.getState().confirm()
    expect(d.envConfirm).not.toHaveBeenCalled()
    expect(d.carryConfirm).not.toHaveBeenCalled()
    expect(onConfirm).toHaveBeenCalledWith(false, true, {
      threads: { sinceDays: 30 },
      secrets: { STRIPE_KEY: 'blank', GMAIL_TOKEN: 'skip' },
    })
    expect(useCloneToDialogStore.getState().phase).toBe('running')
  })

  it('.env files as files need the clone-env confirm first (D4)', async () => {
    const { d, onConfirm } = await open(preview())
    useCloneToDialogStore.getState().setSecretChoice('STRIPE_KEY', 'skip')
    useCloneToDialogStore.getState().setCarrySecrets(true)
    useCloneToDialogStore.getState().setBringThread(false)
    await useCloneToDialogStore.getState().confirm()
    expect(d.envConfirm).toHaveBeenCalledTimes(1)
    expect(onConfirm).toHaveBeenCalledWith(true, true, {
      threads: 'none',
      secrets: { STRIPE_KEY: 'skip', GMAIL_TOKEN: 'skip' },
      env_confirm_token: 'env-tok',
    })
  })

  it('a Copy row needs the clone-carry confirm; until the vault update it stays on options with the reason', async () => {
    const { d, onConfirm } = await open(preview(), { carryConfirm: vi.fn(async () => Promise.reject(new Error(CARRY_NOT_READY))) })
    await useCloneToDialogStore.getState().confirm()
    expect(d.carryConfirm).toHaveBeenCalledWith({ STRIPE_KEY: 'copy', GMAIL_TOKEN: 'skip' }, expect.any(Function))
    expect(onConfirm).not.toHaveBeenCalled()
    const s = useCloneToDialogStore.getState()
    expect(s.phase).toBe('options')
    expect(s.confirming).toBe(false)
    expect(s.optionsError).toBe(CARRY_NOT_READY)
  })

  it('the carry token rides confirm_token when the confirm succeeds', async () => {
    const { onConfirm } = await open(preview())
    await useCloneToDialogStore.getState().confirm()
    expect(onConfirm.mock.calls[0][2]).toMatchObject({ confirm_token: 'carry-tok', secrets: { STRIPE_KEY: 'copy' } })
  })

  it('a password confirm waits on the dialog field; cancel keeps the options open with no error', async () => {
    let asked: Promise<string | null> | null = null
    const { onConfirm } = await open(preview(), {
      envConfirm: vi.fn(async (ask) => {
        asked = ask('vault-password', '.env files of w')
        const pw = await asked
        if (pw === null) {
          const { CloneConfirmCancelled } = await import('@/lib/clone-options')
          throw new CloneConfirmCancelled()
        }
        return 'env-tok'
      }),
    })
    useCloneToDialogStore.getState().setSecretChoice('STRIPE_KEY', 'blank')
    useCloneToDialogStore.getState().setCarrySecrets(true)
    const running = useCloneToDialogStore.getState().confirm()
    await flush()
    expect(useCloneToDialogStore.getState().passwordPrompt).toEqual({ method: 'vault-password', label: '.env files of w' })
    useCloneToDialogStore.getState().answerPassword(null)
    await running
    expect(onConfirm).not.toHaveBeenCalled()
    expect(useCloneToDialogStore.getState().optionsError).toBeNull()
    expect(useCloneToDialogStore.getState().passwordPrompt).toBeNull()
  })
})
