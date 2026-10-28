// @vitest-environment jsdom
// prd-agent-secret-handoff-v1 §10.1 / H18: the handoff Thread card.
import { describe, expect, it, afterEach, vi } from 'vitest'
import { render, screen, fireEvent, cleanup, waitFor } from '@testing-library/react'
import { HandoffCard } from './HandoffCard'
import { coerceHandoff, threadItemsFromSnapshot, type OverlayHandoff } from './overlayThread'
import type { HandoffDecideBody } from '@/components/Settings/sections/handoff-api'

function refusal(code: string, hint: string): Error {
  return new Error(JSON.stringify({ ok: false, error: { code, hint } }))
}

const BASE: OverlayHandoff = {
  deliveryId: 'hd_7f3c0000000000000000000000000000',
  name: 'STRIPE_KEY',
  from: 'sales::rosson.k2.dev',
  kind: 'api_key',
  hosts: ['https://api.stripe.com:443'],
  note: 'for the refund job',
  clash: false,
  status: 'pending',
  workspace: 'ops',
}

function view(status: string, ref: string | null = null) {
  return {
    deliveryId: BASE.deliveryId,
    direction: 'in' as const,
    status,
    workspaceId: 'ws-ops',
    workspace: 'ops',
    peer: BASE.from,
    cross: true,
    name: 'STRIPE_KEY',
    kind: 'api_key',
    hosts: BASE.hosts,
    note: BASE.note,
    createdAt: '2026-10-16T10:00:00Z',
    ref,
  }
}

afterEach(() => cleanup())

describe('HandoffCard', () => {
  it("shows the sender's --ttl (2026-10-28: a 1-hour send read 'expires in 7 days')", () => {
    const now = 1_800_000_000
    const h = coerceHandoff({ ...BASE, expiresAt: now + 3600, ttlSecs: 3600 })
    expect(h?.ttlSecs).toBe(3600)
    render(<HandoffCard handoff={h as OverlayHandoff} nowSec={now} onDecide={async () => {}} />)
    expect(screen.getByTestId('thread-handoff-expiry').textContent).toBe('expires in 1 hour')
    expect(screen.getByTestId('thread-handoff-ttl').textContent).toContain('kept 1 hour after it was sent')
    cleanup()
    render(<HandoffCard handoff={BASE} onDecide={async () => {}} />)
    expect(screen.queryByTestId('thread-handoff-ttl')).toBeNull()
    expect(coerceHandoff({ ...BASE, ttlSecs: 'x' })?.ttlSecs).toBeUndefined()
  })

  it('names the sender, host, secret, kind, site and note, and has no value field', () => {
    const { container } = render(<HandoffCard handoff={BASE} onDecide={async () => {}} />)
    const header = screen.getByTestId('thread-handoff-header').textContent ?? ''
    expect(header).toContain('sales')
    expect(header).toContain('(rosson.k2.dev)')
    expect(header).toContain('STRIPE_KEY')
    expect(header).toContain('api key, for api.stripe.com')
    expect(screen.getByTestId('thread-handoff-note').textContent).toContain('for the refund job')
    expect(container.querySelector('input[type="password"]')).toBeNull()
    expect(screen.getByText('Always accept from sales')).toBeTruthy()
  })

  it('Accept posts the delivery id (with always when ticked) and shows the reference', async () => {
    const calls: HandoffDecideBody[] = []
    const onDecide = vi.fn(async (b: HandoffDecideBody) => {
      calls.push(b)
      return { ok: true, delivery: view('landed', 'k2v://ws/ops/STRIPE_KEY'), ref: 'k2v://ws/ops/STRIPE_KEY' }
    })
    render(<HandoffCard handoff={BASE} onDecide={onDecide} />)
    fireEvent.click(screen.getByTestId('thread-handoff-always'))
    fireEvent.click(screen.getByTestId('thread-handoff-accept'))
    await waitFor(() => expect(screen.getByTestId('thread-handoff-card').getAttribute('data-status')).toBe('landed'))
    expect(calls).toEqual([{ id: BASE.deliveryId, action: 'accept', always: true }])
    expect(screen.getByTestId('thread-handoff-ref').textContent).toBe('k2v://ws/ops/STRIPE_KEY')
    expect(screen.queryByTestId('thread-handoff-accept')).toBeNull()
  })

  it('Decline posts decline without always', async () => {
    const calls: HandoffDecideBody[] = []
    render(
      <HandoffCard
        handoff={BASE}
        onDecide={async (b) => {
          calls.push(b)
          return { ok: true, delivery: view('declined') }
        }}
      />,
    )
    fireEvent.click(screen.getByTestId('thread-handoff-always'))
    fireEvent.click(screen.getByTestId('thread-handoff-decline'))
    await waitFor(() => expect(screen.getByTestId('thread-handoff-card').getAttribute('data-status')).toBe('declined'))
    expect(calls).toEqual([{ id: BASE.deliveryId, action: 'decline' }])
  })

  it('name_taken offers Accept as (a new name) and Replace', async () => {
    const calls: HandoffDecideBody[] = []
    render(
      <HandoffCard
        handoff={BASE}
        onDecide={async (b) => {
          calls.push(b)
          if (calls.length === 1) throw refusal('name_taken', 'this workspace already has STRIPE_KEY; accept it as another name, or replace the current one')
          return { ok: true, delivery: view('landed', 'k2v://ws/ops/STRIPE_KEY_SALES') }
        }}
      />,
    )
    fireEvent.click(screen.getByTestId('thread-handoff-accept'))
    await screen.findByTestId('thread-handoff-error')
    expect(screen.getByTestId('thread-handoff-clash')).toBeTruthy()
    expect(screen.getByTestId('thread-handoff-replace')).toBeTruthy()
    const save = screen.getByTestId('thread-handoff-as-save') as HTMLButtonElement
    expect(save.disabled).toBe(true)
    fireEvent.change(screen.getByTestId('thread-handoff-as'), { target: { value: 'bad name' } })
    expect(save.disabled).toBe(true)
    fireEvent.change(screen.getByTestId('thread-handoff-as'), { target: { value: 'STRIPE_KEY_SALES' } })
    fireEvent.click(save)
    await waitFor(() => expect(calls).toHaveLength(2))
    expect(calls[1]).toEqual({ id: BASE.deliveryId, action: 'accept', as: 'STRIPE_KEY_SALES' })
  })

  it('a known clash shows Replace in place of Accept and posts replace', async () => {
    const calls: HandoffDecideBody[] = []
    render(<HandoffCard handoff={{ ...BASE, clash: true }} onDecide={async (b) => void calls.push(b)} />)
    expect(screen.queryByTestId('thread-handoff-accept')).toBeNull()
    fireEvent.click(screen.getByTestId('thread-handoff-replace'))
    await waitFor(() => expect(calls).toEqual([{ id: BASE.deliveryId, action: 'accept', replace: true }]))
  })

  it('a decided card shows status and who decided, and no buttons', () => {
    render(
      <HandoffCard
        handoff={{ ...BASE, status: 'landed', reference: 'k2v://ws/ops/STRIPE_KEY' }}
        answeredBy="lee"
        onDecide={async () => {
          throw new Error('must not be called')
        }}
      />,
    )
    const card = screen.getByTestId('thread-handoff-card')
    expect(card.textContent).toContain('accepted')
    expect(screen.getByTestId('thread-handoff-answered-by').textContent).toBe(' by lee')
    expect(screen.getByTestId('thread-handoff-ref').textContent).toBe('k2v://ws/ops/STRIPE_KEY')
    expect(card.querySelectorAll('button')).toHaveLength(0)
  })

  it('other refusals show the hint', async () => {
    render(
      <HandoffCard
        handoff={BASE}
        onDecide={async () => {
          throw refusal('handoff_person_only', 'a person on this server accepts or declines secrets')
        }}
      />,
    )
    fireEvent.click(screen.getByTestId('thread-handoff-accept'))
    expect((await screen.findByTestId('thread-handoff-error')).textContent).toContain('a person on this server')
    expect(screen.getByTestId('thread-handoff-card').getAttribute('data-status')).toBe('pending')
  })
})

describe('coerceHandoff', () => {
  it('keeps names and the reference and drops anything else', () => {
    const h = coerceHandoff({ ...BASE, valueB64: 'c2VjcmV0', value: 'secret', reference: 'k2v://ws/ops/K' })
    expect(h).not.toBeNull()
    expect(JSON.stringify(h)).not.toContain('secret"')
    expect((h as unknown as Record<string, unknown>).valueB64).toBeUndefined()
    expect(h?.reference).toBe('k2v://ws/ops/K')
    expect(coerceHandoff({ name: 'x' })).toBeNull()
  })

  it('a snapshot item of kind handoff carries the card', () => {
    const snap = threadItemsFromSnapshot({
      items: [{ collection: 'thread', seq: 3, id: 'c1', doc: { id: 'c1', kind: 'handoff', from: 'k2', body: 'sales sent STRIPE_KEY.', handoff: BASE } }],
    })
    expect(snap.items[0]?.doc.handoff?.deliveryId).toBe(BASE.deliveryId)
  })
})

// Home / remote rooms (Rosson hit these live, 0.46.6): a handoff doc from
// another server can come without `hosts`, and a remote room can't decide.
describe('HandoffCard from another server', () => {
  it('a doc without hosts renders (no crash, no "for …" sites)', () => {
    const raw = { ...BASE } as Partial<OverlayHandoff>
    delete raw.hosts
    render(<HandoffCard handoff={raw as OverlayHandoff} onDecide={async () => {}} />)
    const header = screen.getByTestId('thread-handoff-header').textContent ?? ''
    expect(header).toContain('STRIPE_KEY')
    expect(header).not.toContain(', for ')
    expect(screen.getByTestId('thread-handoff-accept')).toBeTruthy()
  })

  it('in a remote room: no Accept or Decline, just "Open <host> directly to accept this"', () => {
    const onDecide = vi.fn(async () => {})
    render(<HandoffCard handoff={BASE} onDecide={onDecide} remoteHost="rosson.k2.dev" />)
    expect(screen.getByTestId('thread-handoff-remote').textContent).toBe('Open rosson.k2.dev directly to accept this.')
    for (const id of ['thread-handoff-accept', 'thread-handoff-decline', 'thread-handoff-accept-as', 'thread-handoff-replace', 'thread-handoff-always']) {
      expect(screen.queryByTestId(id)).toBeNull()
    }
    expect(screen.getByTestId('thread-handoff-header').textContent).toContain('STRIPE_KEY')
    expect(onDecide).not.toHaveBeenCalled()
  })
})
