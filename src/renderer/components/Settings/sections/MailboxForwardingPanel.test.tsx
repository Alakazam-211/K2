// @vitest-environment jsdom
// Settings → Email Hosting → a mailbox → Forwarding (FW4): the panel only
// calls the forward routes. It picks the door the daemon allows (staff route
// first, else the person route), shows each destination's keep / state /
// notice / 24 h counts and the events, sends keep only when the viewer chose
// it, offers Resume only where the daemon will allow it, and acts on one
// destination without touching the others.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'

type Fwd = Record<string, unknown> | null

const h = vi.hoisted(() => ({
  gets: [] as Array<{ route: string; params: Record<string, unknown> | undefined }>,
  posts: [] as Array<{ route: string; body: Record<string, unknown> }>,
  staffRefuses: false,
  personRefuses: false,
  forward: null as Fwd,
  forwards: null as Fwd[] | null,
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
        return { ok: true, address: params?.address, forward: h.forward, forwards: h.forwards ?? (h.forward ? [h.forward] : []), maxDestinations: 5, events: h.events }
      }
      throw new Error(`unexpected GET ${route}`)
    }),
    daemonCliPost: primaryOnly(async (route: string, body: Record<string, unknown>) => {
      h.posts.push({ route, body })
      if (route.endsWith('forward') || route.endsWith('forward/add')) {
        const outside = String(body.to).endsWith('@gmail.test')
        return {
          ok: true,
          address: body.address,
          to: body.to,
          keep: body.keep ?? outside,
          kind: outside ? 'external' : 'internal',
          state: 'active',
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
  notice: 'sent',
  noticeAt: 1_800_000_100,
  last24h: { forwarded: 12, delivered: 9, refused: 3 },
}

beforeEach(() => {
  h.gets = []
  h.posts = []
  h.staffRefuses = false
  h.personRefuses = false
  h.forward = null
  h.forwards = null
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

  it('an old forward K2 left off, and a notice that failed, say so in the daemon\'s words', async () => {
    h.forwards = [
      { ...base, id: 'f1', state: 'pending_verify', notice: 'stale', noticeWords: 'not on: it was set more than 7 days ago' },
      { ...base, id: 'f2', to: 'two@gmail.test', notice: 'failed', noticeWords: 'K2 couldn’t email this address its stop link yet' },
    ]
    render(<MailboxForwardingPanel address="sales@brand.test" canMutate />)
    const notes = await screen.findAllByTestId('forward-notice')
    expect(notes[0].textContent).toContain('not on: it was set more than 7 days ago')
    expect(notes[1].textContent).toContain('stop link yet')
    expect(screen.queryByRole('button', { name: 'Resume' })).toBeNull()
    expect(screen.getByText('Off: too old')).toBeTruthy()
  })

  it('a forward whose mail can\'t arrive says so in the daemon\'s words', async () => {
    h.forward = { ...base, cantArrive: 'mail to me@gmail.test can’t arrive: this server hosts its domain' }
    render(<MailboxForwardingPanel address="sales@brand.test" canMutate />)
    expect((await screen.findByTestId('forward-cant-arrive')).textContent).toContain('can’t arrive')
  })

  it('several addresses: each row acts on its own address; Add uses the add route, and the cap hides it', async () => {
    h.forwards = [
      { ...base, id: 'f1', to: 'one@gmail.test' },
      { ...base, id: 'f2', to: 'two@gmail.test', state: 'paused', pauseReason: 'manual', pauseWords: 'someone paused it' },
    ]
    useConfirmDialogStore.setState({ confirm: vi.fn(async () => true) })
    render(<MailboxForwardingPanel address="sales@brand.test" canMutate />)
    expect(await screen.findAllByTestId('forward-row')).toHaveLength(2)
    expect(screen.queryByRole('button', { name: 'Change…' })).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Pause' }))
    await waitFor(() => expect(h.posts[0]).toEqual({ route: 'mail/forward/pause', body: { address: 'sales@brand.test', to: 'one@gmail.test' } }))
    fireEvent.click(await screen.findByRole('button', { name: 'Resume' }))
    await waitFor(() => expect(h.posts[1]).toEqual({ route: 'mail/forward/resume', body: { address: 'sales@brand.test', to: 'two@gmail.test' } }))
    fireEvent.click((await screen.findAllByRole('button', { name: 'Remove' }))[1])
    await waitFor(() => expect(h.posts[2]).toEqual({ route: 'mail/forward/rm', body: { address: 'sales@brand.test', to: 'two@gmail.test' } }))
    fireEvent.click(await screen.findByRole('button', { name: 'Add another address…' }))
    fireEvent.change(await screen.findByLabelText('Forward to'), { target: { value: 'three@gmail.test' } })
    fireEvent.click(screen.getByRole('button', { name: 'Forward' }))
    await waitFor(() => expect(h.posts[3].route).toBe('mail/forward/add'))
    expect(h.posts[3].body.to).toBe('three@gmail.test')
    cleanup()
    h.forwards = [1, 2, 3, 4, 5].map((n) => ({ ...base, id: `f${n}`, to: `d${n}@gmail.test` }))
    render(<MailboxForwardingPanel address="sales@brand.test" canMutate />)
    await screen.findAllByTestId('forward-row')
    expect(screen.queryByRole('button', { name: 'Add another address…' })).toBeNull()
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
    await waitFor(() => expect(h.posts).toEqual([{ route: 'mail/forward/resume', body: { address: 'sales@brand.test', to: 'me@gmail.test' } }]))
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
    expect(screen.getByText(/emailed this address a link to stop it/)).toBeTruthy()
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
    expect(resumeRule('staff', 'revoked').why).toContain('pressed Stop')
    expect(resumeRule('staff', 'not_told').can).toBe(false)
    expect(resumeRule('person', 'air_gap').can).toBe(false)
    expect(resumeRule('staff', 'receive_only').can).toBe(false)
    expect(resumeRule('staff', 'upgrade_required').can).toBe(false)
    expect(resumeRule('staff', 'upgrade_required').why).toContain('k2 hostmail upgrade')
    expect(resumeRule('person', 'box_brake').can).toBe(false)
    expect(resumeRule('staff', 'box_brake').can).toBe(true)
    expect(resumeRule('person', 'refused').can).toBe(true)
    expect(resumeRule('person', 'manual').can).toBe(true)
  })
})
