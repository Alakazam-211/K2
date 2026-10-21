// @vitest-environment jsdom
// Regression (0.46.2, Rosson 2026-10-21): a secret card with a readable
// delivery could never be saved. Every Save got "the agent will be able to
// see this value; confirm to save" again and the card stayed pending.
//
// Root cause: since 0.46.1 `daemon-cli` throws a `DaemonHttpError` whose
// message is the hint and whose `code` rides the error, but the card read
// the refusal code only out of a raw JSON message (`daemonErrorOf`). The
// 409 was shown as a plain error, the card never entered its warning state,
// and the next click sent no `confirm`. SecretCard.test.tsx threw the old
// raw-JSON Error by hand, so it never saw what the app really throws.
//
// This file wires the card to the REAL `daemonCliPost` (only `fetch` and
// the creds are stubbed), with a stub daemon that answers like the vault:
// 409 unless the body carries `confirm=1`. A live Thread frame re-renders
// the card between the two clicks.

import { describe, expect, it, afterEach, beforeEach, vi } from 'vitest'
import { render, screen, fireEvent, cleanup, waitFor } from '@testing-library/react'

const { credsMock } = vi.hoisted(() => ({ credsMock: vi.fn() }))

vi.mock('@/kessel/daemon-ws', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@/kessel/daemon-ws')>()
  return {
    ...actual,
    resolveWindowHostCreds: (...a: unknown[]) => credsMock(...a),
    invalidateDaemonWs: () => {},
  }
})

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))

import { SecretCard } from './SecretCard'
import { threadAnswerBody, type OverlaySecret } from './overlayThread'
import { daemonCliPost } from '@/lib/daemon-cli'
import { daemonErrorOf } from '@/lib/daemon-error'
import { DaemonHttpError } from '@/lib/daemon-http-error'
import { primaryScope } from '@/kessel/server-scope'

const VALUE = 'smoke-0462-value'
const READABLE_HINT = 'the agent will be able to see this value; confirm to save'
const MISMATCH_HINT = 'this looks like a Stripe key, but the card sends it somewhere else; confirm to save anyway'

/** The bodies the stub daemon received, parsed. */
let posted: Record<string, string>[] = []

/** A stand-in for `POST /cli/thread/answer` with the vault's VS12 rule. */
function stubDaemon(refusal: { code: string; hint: string }): void {
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string, init?: RequestInit) => {
      expect(url).toContain('/cli/thread/answer')
      const body = JSON.parse(String(init?.body)) as Record<string, string>
      posted.push(body)
      const confirmed = body.confirm === '1' || body.confirm === 'true'
      const [status, text] = confirmed
        ? [200, JSON.stringify({ ok: true, id: body.id, status: 'set', ref: 'k2v://ws/sales/SMOKE_0462_TEST_SECRET' })]
        : [409, JSON.stringify({ ok: false, error: refusal })]
      return { ok: status === 200, status, url, text: async () => text } as unknown as Response
    }),
  )
}

function Card({ secret }: { secret: OverlaySecret }) {
  return (
    <SecretCard
      secret={secret}
      agent="sales"
      workspace="sales"
      server="This computer"
      onSubmit={async (payload) => {
        // What useOverlayThread.answer does.
        await daemonCliPost(primaryScope(), 'thread/answer', threadAnswerBody('sales', 'c1', payload))
      }}
      onDismiss={() => {}}
    />
  )
}

function field(): HTMLInputElement {
  return screen.getByTestId('thread-secret-field') as HTMLInputElement
}

beforeEach(() => {
  posted = []
  credsMock.mockReset()
  credsMock.mockResolvedValue({ port: 47800, token: 'local-tok', host: '127.0.0.1', secure: false })
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe('secret card through the real daemon client', () => {
  it('daemonErrorOf reads the code off the DaemonHttpError daemon-cli throws', () => {
    expect(daemonErrorOf(new DaemonHttpError(READABLE_HINT, 409, 'card_readable_delivery'))).toEqual({
      code: 'card_readable_delivery',
      hint: READABLE_HINT,
    })
    // The raw-body shape still reads.
    expect(daemonErrorOf(new Error(JSON.stringify({ error: { code: 'x', hint: 'y' } })))).toEqual({ code: 'x', hint: 'y' })
    expect(daemonErrorOf(new Error('plain'))).toBeNull()
  })

  it('a readable delivery saves on the second click, even after a live frame re-renders the card', async () => {
    stubDaemon({ code: 'card_readable_delivery', hint: READABLE_HINT })
    const proposal: OverlaySecret = {
      name: 'SMOKE_0462_TEST_SECRET',
      status: 'pending',
      scope: 'workspace',
      delivery: 'file',
      secretKind: 'password',
      hosts: [],
    }
    const { rerender } = render(<Card secret={proposal} />)
    fireEvent.change(field(), { target: { value: VALUE } })
    fireEvent.click(screen.getByTestId('thread-secret-submit'))
    await waitFor(() => expect(posted).toHaveLength(1))
    // Today's bug: this came back as a plain error and the button said Save.
    await screen.findByTestId('thread-secret-warning')
    expect(screen.queryByTestId('thread-secret-error')).toBeNull()
    expect(screen.getByTestId('thread-secret-card').getAttribute('data-warning')).toBe('card_readable_delivery')
    expect(screen.getByTestId('thread-secret-submit').textContent).toBe('Save anyway')

    // A live Thread frame (activity, read state, a re-read): a new, equal doc.
    rerender(<Card secret={{ ...proposal, hosts: [] }} />)
    expect(field().value).toBe(VALUE)
    expect(screen.getByTestId('thread-secret-submit').textContent).toBe('Save anyway')

    fireEvent.click(screen.getByTestId('thread-secret-submit'))
    await waitFor(() => expect(posted).toHaveLength(2))
    expect(posted[0].confirm).toBeUndefined()
    expect(posted[1]).toEqual({ addr: 'sales', id: 'c1', secret: VALUE, confirm: '1' })
    await waitFor(() => expect(field().value).toBe(''))
    expect(screen.queryByTestId('thread-secret-warning')).toBeNull()
    expect(screen.queryByTestId('thread-secret-error')).toBeNull()
  })

  it('a host mismatch also turns into "Save anyway" and confirms on the second click', async () => {
    stubDaemon({ code: 'card_host_mismatch', hint: MISMATCH_HINT })
    render(
      <Card
        secret={{ name: 'STRIPE_KEY', status: 'pending', delivery: 'fetch', hosts: ['https://api.example.com:443'] }}
      />,
    )
    fireEvent.change(field(), { target: { value: VALUE } })
    fireEvent.click(screen.getByTestId('thread-secret-submit'))
    expect((await screen.findByTestId('thread-secret-warning')).textContent).toBe(MISMATCH_HINT)
    fireEvent.click(screen.getByTestId('thread-secret-submit'))
    await waitFor(() => expect(posted).toHaveLength(2))
    expect(posted[1].confirm).toBe('1')
    await waitFor(() => expect(field().value).toBe(''))
  })
})
