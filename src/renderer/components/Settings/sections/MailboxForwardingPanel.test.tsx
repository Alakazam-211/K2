// @vitest-environment jsdom
// Settings → Email Hosting → a mailbox → Forwarding (FW4): the panel only
// calls the forward routes. It picks the door the daemon allows (staff route
// first, else the person route), shows destination / keep / state / confirm
// link / 24 h counts / events, sends keep only when the viewer chose it, and
// offers Resume only where the daemon will allow it.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'

type Fwd = Record<string, unknown> | null

const h = vi.hoisted(() => ({
  gets: [] as Array<{ route: string; params: Record<string, unknown> | undefined }>,
  posts: [] as Array<{ route: string; body: Record<string, unknown> }>,
  staffRefuses: false,
  personRefuses: false,
  forward: null as Fwd,
  events: [] as unknown[],
}))

vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}), emit: vi.fn(async () => {}) }))
vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly(async (route: string, params?: Record<string, unknown>) => {
      // Stores pulled in by the ui primitives (themes) read their own routes.
      if (!route.startsWith('mail/')) return { ok: true, themes: [] }
      h.gets.push({ route, params })
      if (route === 'mail/forward' && h.staffRefuses) throw new Error('forbidden')
      if (route === 'mail/self/forward' && h.personRefuses) {
        throw new Error(JSON.stringify({ ok: false, error: { code: 'forbidden', hint: 'not linked to you' } }))
      }
      if (route === 'mail/forward' || route === 'mail/self/forward') {
        return { ok: true, address: params?.address, forward: h.forward, events: h.events }
      }
      throw new Error(`unexpected GET ${route}`)
    }),
    daemonCliPost: primaryOnly(async (route: string, body: Record<string, unknown>) => {
      h.posts.push({ route, body })
      if (route.endsWith('forward')) {
        const outside = String(body.to).endsWith('@gmail.test')
        return {
          ok: true,
          address: body.address,
          to: body.to,
          keep: body.keep ?? outside,
          kind: outside ? 'external' : 'internal',
          state: outside ? 'pending_verify' : 'active',
        }
      }
      return { ok: true, address: body.address, state: 'active' }
    }),
  }
})

import { MailboxForwardingPanel } from './MailboxForwardingPanel'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { resumeRule } from './mail-forward-api'

const base = {
  id: 'f1',
  address: 'sales@brand.test',
  to: 'me@gmail.test',
  kind: 'external',
  keep: true,
  state: 'active',
  pauseReason: null,
  pauseWords: null,
  setBy: 'owner',
  createdAt: 1_800_000_000,
  updatedAt: 1_800_000_000,
  activatedAt: 1_800_000_100,
  pausedAt: null,
  verify: { state: 'verified', expiresAt: null, verifiedAt: 1_800_000_100 },
  last24h: { forwarded: 12, delivered: 9, refused: 3 },
}

beforeEach(() => {
  h.gets = []
  h.posts = []
  h.staffRefuses = false
  h.personRefuses = false
  h.forward = null
  h.events = []
})
afterEach(cleanup)

describe('MailboxForwardingPanel', () => {
  it('no forward: set sends keep only when the box was touched', async () => {
    render(<MailboxForwardingPanel address="sales@brand.test" canMutate />)
    await screen.findByText(/Not forwarded/)
    fireEvent.change(screen.getByLabelText('Forward to'), { target: { value: 'me@gmail.test' } })
    fireEvent.click(screen.getByRole('button', { name: 'Forward' }))
    await waitFor(() => expect(h.posts).toHaveLength(1))
    expect(h.posts[0]).toEqual({ route: 'mail/forward', body: { address: 'sales@brand.test', to: 'me@gmail.test' } })

    fireEvent.click(screen.getByLabelText('Keep a copy in this mailbox'))
    fireEvent.change(screen.getByLabelText('Forward to'), { target: { value: 'me@gmail.test' } })
    fireEvent.click(screen.getByRole('button', { name: 'Forward' }))
    await waitFor(() => expect(h.posts).toHaveLength(2))
    expect(h.posts[1].body).toEqual({ address: 'sales@brand.test', to: 'me@gmail.test', keep: false })
  })

  it('pending: says a confirm link was sent and when it expires', async () => {
    h.forward = { ...base, state: 'pending_verify', verify: { state: 'pending', expiresAt: 1_800_086_400, verifiedAt: null } }
    render(<MailboxForwardingPanel address="sales@brand.test" canMutate />)
    const c = await screen.findByTestId('forward-pending')
    expect(c.textContent).toContain('A confirm link was sent to me@gmail.test')
    expect(c.textContent).toContain('the link expires')
    expect(screen.queryByRole('button', { name: 'Resume' })).toBeNull()
  })

  it('paused by refusals: the reason in words, the 24 h counts, Resume for staff', async () => {
    h.forward = {
      ...base,
      state: 'paused',
      pauseReason: 'refused',
      pauseWords: 'the destination refused too many forwards as spam or by policy',
      pausedAt: 1_800_000_500,
    }
    h.events = [{ id: 'e1', forwardId: 'f1', event: 'paused', actor: 'k2', detail: { reason: 'refused' }, at: 1_800_000_500 }]
    render(<MailboxForwardingPanel address="sales@brand.test" canMutate />)
    const c = await screen.findByTestId('forward-paused')
    expect(c.textContent).toContain('the destination refused too many forwards as spam or by policy')
    expect(screen.getByTestId('forward-counts').textContent).toContain('12')
    fireEvent.click(screen.getByText(/History \(1\)/))
    expect(screen.getByTestId('forward-events').textContent).toContain('Paused (refused)')
    fireEvent.click(screen.getByRole('button', { name: 'Resume' }))
    await waitFor(() => expect(h.posts).toEqual([{ route: 'mail/forward/resume', body: { address: 'sales@brand.test' } }]))
  })

  it('a person (staff route refused) uses the self routes and gets no Resume on the server brake', async () => {
    h.staffRefuses = true
    h.forward = { ...base, state: 'paused', pauseReason: 'box_brake', pauseWords: 'too many forwards from this server were refused' }
    render(<MailboxForwardingPanel address="sales@brand.test" canMutate />)
    const c = await screen.findByTestId('forward-paused')
    expect(h.gets.map((g) => g.route)).toEqual(['mail/forward', 'mail/self/forward'])
    expect(c.textContent).toContain('The owner or an Admin resumes them')
    expect(screen.queryByRole('button', { name: 'Resume' })).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Change…' }))
    fireEvent.change(screen.getByLabelText('Forward to'), { target: { value: 'other@gmail.test' } })
    fireEvent.click(screen.getByRole('button', { name: 'Forward' }))
    await waitFor(() => expect(h.posts).toHaveLength(1))
    expect(h.posts[0]).toEqual({ route: 'mail/self/forward', body: { address: 'sales@brand.test', to: 'other@gmail.test', keep: true } })
  })

  it('neither door: says who manages it, no controls', async () => {
    h.staffRefuses = true
    h.personRefuses = true
    render(<MailboxForwardingPanel address="sales@brand.test" canMutate />)
    expect((await screen.findByTestId('forward-denied')).textContent).toContain('the owner, an Admin, or the person')
    expect(screen.queryByRole('button')).toBeNull()
  })

  it('remove asks first, then posts unset; pause on an active forward', async () => {
    h.forward = { ...base }
    useConfirmDialogStore.setState({ confirm: vi.fn(async () => true) })
    render(<MailboxForwardingPanel address="sales@brand.test" canMutate />)
    await screen.findByTestId('forward-dest')
    fireEvent.click(screen.getByRole('button', { name: 'Pause' }))
    await waitFor(() => expect(h.posts.map((p) => p.route)).toEqual(['mail/forward/pause']))
    fireEvent.click(await screen.findByRole('button', { name: 'Remove' }))
    await waitFor(() => expect(h.posts.map((p) => p.route)).toEqual(['mail/forward/pause', 'mail/forward/unset']))
  })

  it('read-only (canMutate false): no gestures', async () => {
    h.forward = { ...base, state: 'paused', pauseReason: 'manual', pauseWords: 'someone paused it' }
    render(<MailboxForwardingPanel address="sales@brand.test" canMutate={false} />)
    await screen.findByTestId('forward-paused')
    expect(screen.queryByRole('button', { name: 'Resume' })).toBeNull()
    expect(screen.queryByRole('button', { name: 'Remove' })).toBeNull()
  })
})

describe('resumeRule (mirrors the daemon)', () => {
  it('revoked and the self-lifting pauses never resume; the brake is staff only', () => {
    expect(resumeRule('staff', 'revoked').can).toBe(false)
    expect(resumeRule('person', 'air_gap').can).toBe(false)
    expect(resumeRule('staff', 'receive_only').can).toBe(false)
    expect(resumeRule('person', 'box_brake').can).toBe(false)
    expect(resumeRule('staff', 'box_brake').can).toBe(true)
    expect(resumeRule('person', 'refused').can).toBe(true)
    expect(resumeRule('person', 'manual').can).toBe(true)
  })
})
