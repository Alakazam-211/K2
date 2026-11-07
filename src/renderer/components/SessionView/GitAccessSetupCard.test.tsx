// @vitest-environment jsdom
// K2's own form for this server's GitHub credential on a card (0.46.10,
// `compute.git-access-setup`): "Set up GitHub access" opens it, the App key
// or the token goes straight to the setup route as this person, and only
// then is Allow sent, with no key or token in it. A refused save leaves the
// card open and unanswered.
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { ServerScope } from '@/kessel/server-scope'

const post = vi.fn()
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(),
  daemonCliPost: (...a: unknown[]) => post(...a),
}))

import { ChoiceCard } from './ChoiceCard'
import { GIT_ACCESS_GUIDANCE, gitAccessProblem } from './GitAccessSetupForm'
import { threadItemsFromSnapshot, type OverlayChoice } from './overlayThread'
import { permissionDoc } from './permissionCard.fixtures'
import type { PermissionCardActions } from './permissionCardView'

const scope = { id: 'local', hostKey: 'local', isPrimary: true } as unknown as ServerScope

const PEM = '-----BEGIN PRIVATE KEY-----\nMIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC7k2testFAKEkey\n-----END PRIVATE KEY-----\n'
const PAT = 'github_pat_11K2TESTfakeCARDtoken0123456789'

const SETUP_VIEW = {
  requestId: 'pr_gas1',
  action: 'compute.git-access-setup',
  setting: 'GitHub access for compute jobs',
  whatChanges: "Set this server's GitHub credential in K2's own form: a GitHub App (its App ID and private key) or a fine-grained token.",
  grant: 'standing',
  scope: 'this server',
  duration: 'until someone replaces or clears it (k2 compute git-access setup)',
  consequences: 'It goes straight into this server’s vault; agent-x never sees it.',
  values: [{ key: 'workspace', value: 'agent-x' }],
  why: { by: 'agent-x', text: 'Jobs need to clone acme/app.' },
  notes: [],
  askedFor: null,
  approvers: { floor: 'admin', label: 'owner or admin', names: ['rosson'] },
  settingsPage: 'compute',
}

function choiceOf(doc: Record<string, unknown>): OverlayChoice {
  const snap = threadItemsFromSnapshot({ conversation_id: 'c', items: [{ collection: 'thread', seq: 9, id: doc.id, doc }] })
  const c = snap.items[0]?.doc.choice
  if (!c) throw new Error('no choice on the doc')
  return c
}

function setupCard(actions: PermissionCardActions): void {
  const c = choiceOf(permissionDoc({ view: SETUP_VIEW }))
  render(
    <ChoiceCard
      options={c.options}
      allowCustom={c.allow_custom}
      status={c.status}
      answer={c.answer}
      expiresAt={c.expiresAt}
      nowSec={1761062000}
      permission={c.permission}
      permissionActions={actions}
      from="agent-x"
      onPick={() => {
        throw new Error('a permission card never answers through onPick')
      }}
    />,
  )
}

beforeEach(() => post.mockReset())
afterEach(() => cleanup())

describe('compute.git-access-setup', () => {
  it('opens K2 form with the CLI guidance; the App key goes to the route and never into the answer', async () => {
    post.mockResolvedValue({ credential: { kind: 'app', appId: '4242', app: 'acme-compute' }, note: 'x' })
    const answer = vi.fn(async () => ({ ok: true }))
    setupCard({ answer, confirmScope: scope })
    expect(screen.getByTestId('permission-git-note').textContent).toContain('never sees it')
    expect(screen.queryByTestId('git-access-form')).toBeNull()
    fireEvent.click(screen.getByTestId('permission-allow'))
    expect(answer).not.toHaveBeenCalled()
    expect(screen.getByTestId('git-access-guidance').textContent).toBe(GIT_ACCESS_GUIDANCE)
    expect(GIT_ACCESS_GUIDANCE).toContain('Contents read-only')
    fireEvent.change(screen.getByTestId('git-access-app-id'), { target: { value: '4242' } })
    fireEvent.change(screen.getByTestId('git-access-key'), { target: { value: PEM } })
    fireEvent.click(screen.getByTestId('git-access-save'))
    await waitFor(() => expect(answer).toHaveBeenCalledTimes(1))
    expect(post).toHaveBeenCalledWith(scope, 'compute/git-access/setup', { app_id: '4242', key_pem: PEM })
    expect(answer).toHaveBeenCalledWith({ answer: 'Allow' })
    expect(JSON.stringify(answer.mock.calls)).not.toContain('PRIVATE KEY')
  })

  it('reads a chosen .pem file into the key field', async () => {
    post.mockResolvedValue({ credential: { kind: 'app', appId: '7' } })
    const answer = vi.fn(async () => ({ ok: true }))
    setupCard({ answer, confirmScope: scope })
    fireEvent.click(screen.getByTestId('permission-allow'))
    fireEvent.change(screen.getByTestId('git-access-app-id'), { target: { value: '7' } })
    fireEvent.change(screen.getByTestId('git-access-key-file'), { target: { files: [new File([PEM], 'app.pem')] } })
    await waitFor(() => expect((screen.getByTestId('git-access-key') as HTMLTextAreaElement).value).toBe(PEM))
    fireEvent.click(screen.getByTestId('git-access-save'))
    await waitFor(() => expect(answer).toHaveBeenCalledTimes(1))
    expect(post).toHaveBeenCalledWith(scope, 'compute/git-access/setup', { app_id: '7', key_pem: PEM })
  })

  it('the token choice posts a fine-grained token, and the field is emptied', async () => {
    post.mockResolvedValue({ credential: { kind: 'token' } })
    const answer = vi.fn(async () => ({ ok: true }))
    setupCard({ answer, confirmScope: scope })
    fireEvent.click(screen.getByTestId('permission-allow'))
    fireEvent.click(screen.getByTestId('git-access-kind-token'))
    const field = screen.getByTestId('git-access-token') as HTMLInputElement
    expect(field.type).toBe('password')
    fireEvent.change(field, { target: { value: PAT } })
    fireEvent.click(screen.getByTestId('git-access-save'))
    await waitFor(() => expect(answer).toHaveBeenCalledWith({ answer: 'Allow' }))
    expect(post).toHaveBeenCalledWith(scope, 'compute/git-access/setup', { token: PAT })
    expect(field.value).toBe('')
    expect(JSON.stringify(answer.mock.calls)).not.toContain(PAT)
  })

  it('a refused save leaves the card open and unanswered', async () => {
    post.mockImplementationOnce(async () => {
      throw new Error("GitHub didn't accept the App's key: Bad credentials")
    })
    const answer = vi.fn(async () => ({ ok: true }))
    setupCard({ answer, confirmScope: scope })
    fireEvent.click(screen.getByTestId('permission-allow'))
    fireEvent.change(screen.getByTestId('git-access-app-id'), { target: { value: '4242' } })
    fireEvent.change(screen.getByTestId('git-access-key'), { target: { value: PEM } })
    fireEvent.click(screen.getByTestId('git-access-save'))
    await waitFor(() => expect(screen.getByTestId('git-access-error').textContent).toContain("didn't accept"))
    expect(answer).not.toHaveBeenCalled()
  })

  it('checks the values before sending anything', () => {
    const answer = vi.fn(async () => ({ ok: true }))
    setupCard({ answer, confirmScope: scope })
    fireEvent.click(screen.getByTestId('permission-allow'))
    fireEvent.change(screen.getByTestId('git-access-app-id'), { target: { value: 'my-app' } })
    fireEvent.click(screen.getByTestId('git-access-save'))
    expect(screen.getByTestId('git-access-error').textContent).toContain('App ID')
    expect(post).not.toHaveBeenCalled()
    expect(gitAccessProblem('app', '42', 'not a key', '')).toContain('private key')
    expect(gitAccessProblem('token', '', '', '  ')).toContain('github_pat_')
    expect(gitAccessProblem('token', '', '', PAT)).toBeNull()
  })

  it('a client with no server to post to keeps the plain Allow (the daemon refuses it until set)', () => {
    const answer = vi.fn(async () => ({ ok: true }))
    setupCard({ answer })
    expect(screen.queryByTestId('permission-git-note')).toBeNull()
    expect(screen.getByTestId('permission-allow').textContent).toBe('Allow')
  })
})
