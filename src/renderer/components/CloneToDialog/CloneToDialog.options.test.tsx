// @vitest-environment jsdom
// The Clone dialog's 0.46.0 options blocks: "Bring the Thread" ticked by
// default, per-secret radios with the daemon defaults, Copy disabled for a
// Member with the D1 line, the relabelled `.env` option and the button
// that turns into "Confirm and clone".

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { render, screen, cleanup, fireEvent } from '@testing-library/react'

vi.mock('@/stores/projects', () => ({
  useProjectsStore: { getState: () => ({ projects: [], fetchProjects: vi.fn(), setActiveProject: vi.fn() }) },
}))

import CloneToDialog from './CloneToDialog'
import { useCloneToDialogStore } from '../../stores/clone-to-dialog'
import { doneCountsLine } from './CloneOptionsBlocks'
import { MEMBER_NO_COPY_LINE, normalizePreview, type ClonePreview } from '../../lib/clone-options'
import type { VaultRecipientLookup } from '../../lib/clone-vault'

const flush = (): Promise<void> => new Promise((r) => setTimeout(r, 0))

function preview(callerCanCopySecrets: boolean): ClonePreview {
  return normalizePreview({
    threads: { available: true, conversations: 3, docs: 412, participants: ['you', 'alice'] },
    secrets: {
      available: true,
      rows: [
        { name: 'STRIPE_KEY', kind: 'api_key', group: 'workspace', default: 'copy', choice: 'copy', copyable: callerCanCopySecrets },
        { name: 'GMAIL_TOKEN', kind: 'oauth_token', group: 'workspace', default: 'skip', choice: 'skip', copyable: callerCanCopySecrets },
      ],
      notCarried: ['STRIPE_APP (app)'],
    },
    envFiles: ['.env', 'app/.env.local'],
    callerCanCopySecrets,
  })
}

const TARGET = { recipient: 'age1dest', vault_id: 'vault-d', label: 'This computer' }

async function open(p: ClonePreview, lookup: VaultRecipientLookup = { target: TARGET }): Promise<void> {
  useCloneToDialogStore.getState().start({
    projectPath: '/w',
    projectName: 'sales',
    host: null,
    pull: true,
    onConfirm: vi.fn(),
    optionDeps: { lookupTarget: async () => lookup, fetchPreview: async () => p, carryConfirm: async () => 'c' },
  })
  await flush()
  await flush()
}

beforeEach(() => useCloneToDialogStore.getState().close())
afterEach(() => cleanup())

describe('CloneToDialog options (0.46.0)', () => {
  it('shows Bring the Thread ticked with counts and participants, Chatter never travels', async () => {
    await open(preview(true))
    render(<CloneToDialog />)
    const tick = screen.getByTestId('clone-bring-thread') as HTMLInputElement
    expect(tick.checked).toBe(true)
    expect(screen.getByText(/Bring the Thread \(3 conversations, 412 messages\)/)).toBeTruthy()
    expect(screen.getByText(/Includes messages from: you, alice\. Chatter never travels\./)).toBeTruthy()
    fireEvent.click(tick)
    expect(useCloneToDialogStore.getState().bringThread).toBe(false)
  })

  it('per-secret radios start at the defaults; a Copy row makes it "Confirm and clone"', async () => {
    await open(preview(true))
    render(<CloneToDialog />)
    expect((screen.getByTestId('clone-secret-STRIPE_KEY-copy') as HTMLInputElement).checked).toBe(true)
    expect((screen.getByTestId('clone-secret-GMAIL_TOKEN-skip') as HTMLInputElement).checked).toBe(true)
    expect(screen.getByTestId('clone-confirm-button').textContent).toBe('Confirm and clone')
    fireEvent.click(screen.getByTestId('clone-secret-STRIPE_KEY-blank'))
    expect(screen.getByTestId('clone-confirm-button').textContent).toBe('Clone')
    expect(screen.getByText(/Not carried: STRIPE_APP \(app\)/)).toBeTruthy()
    const own = screen.getByTestId('clone-own-box') as HTMLInputElement
    expect(own.checked).toBe(false)
    fireEvent.click(own)
    expect(useCloneToDialogStore.getState().ownBoxMove).toBe(true)
  })

  it('an older destination hides the Secrets block and says why', async () => {
    await open(preview(true), { note: 'This computer runs an older K2 that can\'t receive vault secrets yet.' })
    render(<CloneToDialog />)
    expect(screen.queryByTestId('clone-secrets-block')).toBeNull()
    expect(screen.getByTestId('clone-vault-note').textContent).toMatch(/older K2/)
    expect(screen.getByTestId('clone-confirm-button').textContent).toBe('Clone')
  })

  it('a Member sees Copy disabled and the Owner/Admin line', async () => {
    await open(preview(false))
    render(<CloneToDialog />)
    expect(screen.getByText(MEMBER_NO_COPY_LINE)).toBeTruthy()
    expect((screen.getByTestId('clone-secret-STRIPE_KEY-copy') as HTMLInputElement).disabled).toBe(true)
    expect((screen.getByTestId('clone-secret-STRIPE_KEY-blank') as HTMLInputElement).checked).toBe(true)
    expect(screen.getByTestId('clone-confirm-button').textContent).toBe('Clone')
  })

  it('the .env option is relabelled, lists the files, offers the vault, and needs a confirm', async () => {
    await open(preview(false))
    render(<CloneToDialog />)
    expect(screen.getByText(/files as files \(2\)/)).toBeTruthy()
    expect(screen.getByText('.env, app/.env.local')).toBeTruthy()
    expect(screen.getByText('k2 vault import .env')).toBeTruthy()
    const env = screen.getByTestId('clone-env-files') as HTMLInputElement
    expect(env.checked).toBe(false)
    fireEvent.click(env)
    expect(screen.getByTestId('clone-confirm-button').textContent).toBe('Confirm and clone')
  })

  it('the done screen: Thread import report and the vault outcome, names only', () => {
    expect(
      doneCountsLine(
        {
          threads_imported: 412,
          threads: { conversations: [{ localConversationId: 'a', role: 'canonical', imported: 400, skipped: 0 }, { localConversationId: 'import:b', role: 'detached', imported: 12, skipped: 0 }, { localConversationId: 'c', role: 'sidecar', imported: 0, skipped: 0 }], detached: 1 },
        },
        { imported: ['STRIPE_KEY', 'DB_URL'], blank: ['OPENAI_KEY'], skipped: ['GMAIL_TOKEN'], note: null },
      ),
    ).toEqual([
      'Thread: imported 412 messages into 3 Threads (1 detached)',
      'Secrets: copied STRIPE_KEY, DB_URL · left blank OPENAI_KEY (card posted) · skipped GMAIL_TOKEN',
    ])
    expect(doneCountsLine({}, { imported: [], blank: [], skipped: [], note: 'older K2' })).toEqual(['Secrets: older K2'])
    expect(doneCountsLine({})).toEqual([])
  })
})
