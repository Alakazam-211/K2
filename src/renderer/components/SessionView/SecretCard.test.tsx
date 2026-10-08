// @vitest-environment jsdom
// prd-k2-vault-v1 §10.1 / KV9 / VS12 / VS13: the K2-drawn secret card.
import { describe, expect, it, afterEach, vi } from 'vitest'
import { render, screen, fireEvent, cleanup, waitFor } from '@testing-library/react'
import { SecretCard, siteLabel, providerForSites } from './SecretCard'
import {
  threadAnswerBody,
  threadItemsFromSnapshot,
  type OverlaySecret,
  type ThreadAnswerPayload,
} from './overlayThread'

const VALUE = 'k2test_TESTVALUE_123'

function refusal(code: string, hint: string): Error {
  // What daemonCliPost throws for `{"error":{"code","hint"}}`.
  return new Error(JSON.stringify({ ok: false, error: { code, hint } }))
}

function card(secret: Partial<OverlaySecret>, onSubmit: (p: ThreadAnswerPayload & { secret: string }) => Promise<void>, extra: Record<string, unknown> = {}) {
  const onDismiss = vi.fn()
  const r = render(
    <SecretCard
      secret={{ name: 'STRIPE_KEY', status: 'pending', ...secret }}
      agent="sales"
      workspace="sales"
      server="This computer"
      onSubmit={onSubmit}
      onDismiss={onDismiss}
      {...extra}
    />,
  )
  return { ...r, onDismiss }
}

function field(): HTMLInputElement {
  return screen.getByTestId('thread-secret-field') as HTMLInputElement
}

function paste(v: string): void {
  fireEvent.change(field(), { target: { value: v } })
}

afterEach(() => cleanup())

describe('SecretCard', () => {
  it('draws the header itself and shows the prompt as plain text under "<agent> says:"', () => {
    const long = `**bold** ${'x'.repeat(300)}`
    card({ prompt: long, hosts: ['https://api.stripe.com:443'] }, async () => {})
    expect(screen.getByTestId('thread-secret-header').textContent).toContain('sales asks for a secret')
    expect(screen.getByTestId('thread-secret-header').textContent).toContain('This computer')
    const prompt = screen.getByTestId('thread-secret-prompt')
    expect(prompt.textContent?.startsWith('sales says: **bold**')).toBe(true)
    expect(prompt.querySelector('strong')).toBeNull()
    expect(prompt.textContent!.length).toBe('sales says: '.length + 200)
    expect(field().type).toBe('password')
    expect(screen.getByTestId('thread-secret-honest').textContent).toBe(
      'This key can still do anything Stripe allows it to. Use a restricted key if you can.',
    )
  })

  it('posts the value once, keeps it out of the page and clears the field after the save', async () => {
    const calls: (ThreadAnswerPayload & { secret: string })[] = []
    const { container } = card({ hosts: ['https://api.stripe.com:443'] }, async (p) => {
      calls.push({ ...p })
    })
    paste(VALUE)
    fireEvent.click(screen.getByTestId('thread-secret-submit'))
    await waitFor(() => expect(field().value).toBe(''))
    expect(calls).toEqual([{ secret: VALUE }])
    expect(container.textContent).not.toContain(VALUE)
    expect(container.innerHTML).not.toContain(VALUE)
    expect((screen.getByTestId('thread-secret-submit') as HTMLButtonElement).disabled).toBe(true)
  })

  it('a host mismatch turns the card red; the second click resends the same value with confirm', async () => {
    const calls: (ThreadAnswerPayload & { secret: string })[] = []
    const onSubmit = vi.fn(async (p: ThreadAnswerPayload & { secret: string }) => {
      calls.push({ ...p })
      if (calls.length === 1) {
        throw refusal('card_host_mismatch', 'this looks like a Stripe key, but the card sends it somewhere else; confirm to save anyway')
      }
    })
    card({ hosts: ['https://api.example.com:443'] }, onSubmit)
    paste(VALUE)
    fireEvent.click(screen.getByTestId('thread-secret-submit'))
    const warning = await screen.findByTestId('thread-secret-warning')
    expect(warning.textContent).toContain('looks like a Stripe key')
    expect(screen.getByTestId('thread-secret-card').getAttribute('data-warning')).toBe('card_host_mismatch')
    expect(screen.getByTestId('thread-secret-submit').textContent).toBe('Save anyway')
    // The field still holds it for the second click; nothing else does.
    expect(field().value).toBe(VALUE)
    fireEvent.click(screen.getByTestId('thread-secret-submit'))
    await waitFor(() => expect(field().value).toBe(''))
    expect(calls).toEqual([{ secret: VALUE }, { secret: VALUE, confirm: true }])
    expect(screen.queryByTestId('thread-secret-warning')).toBeNull()
  })

  it('a readable delivery warns, and changing the choice drops the warning (no stale confirm)', async () => {
    const calls: (ThreadAnswerPayload & { secret: string })[] = []
    card({ hosts: ['https://api.stripe.com:443'] }, async (p) => {
      calls.push({ ...p })
      throw refusal('card_readable_delivery', 'the agent will be able to see this value; confirm to save')
    })
    fireEvent.click(screen.getByLabelText(/an environment variable sales can read/))
    expect(screen.getByTestId('thread-secret-readable').textContent).toBe('sales can see this value')
    paste(VALUE)
    fireEvent.click(screen.getByTestId('thread-secret-submit'))
    await screen.findByTestId('thread-secret-warning')
    fireEvent.click(screen.getByLabelText(/a file sales can read/))
    expect(screen.queryByTestId('thread-secret-warning')).toBeNull()
    fireEvent.click(screen.getByTestId('thread-secret-submit'))
    await waitFor(() => expect(calls).toHaveLength(2))
    expect(calls[0]).toEqual({ secret: VALUE, delivery: 'env' })
    expect(calls[1]).toEqual({ secret: VALUE, delivery: 'file' })
  })

  it('empty sites with "K2 adds it to calls" disables Save until a site is added or the agent may read it', () => {
    card({ hosts: [] }, async () => {})
    paste(VALUE)
    const save = screen.getByTestId('thread-secret-submit') as HTMLButtonElement
    expect(save.disabled).toBe(true)
    expect(screen.getByTestId('thread-secret-needs-site').textContent).toBe('Add a site, or let sales read it.')
    fireEvent.click(screen.getByLabelText(/an environment variable sales can read/))
    expect(save.disabled).toBe(false)
    fireEvent.click(screen.getByLabelText(/K2 adds it to calls/))
    expect(save.disabled).toBe(true)
    fireEvent.change(screen.getByTestId('thread-secret-site-input'), { target: { value: 'api.example.com' } })
    fireEvent.click(screen.getByTestId('thread-secret-site-add'))
    expect(save.disabled).toBe(false)
    expect(screen.queryByTestId('thread-secret-needs-site')).toBeNull()
  })

  it('labels the agent\'s sites "<agent> suggests"; edited sites and scope are sent, untouched ones are not', async () => {
    const calls: (ThreadAnswerPayload & { secret: string })[] = []
    card({ hosts: ['https://api.stripe.com:443'], scope: 'workspace' }, async (p) => {
      calls.push({ ...p })
    })
    const chips = screen.getAllByTestId('thread-secret-site')
    expect(chips).toHaveLength(1)
    expect(chips[0].textContent).toContain('api.stripe.com')
    expect(chips[0].textContent).toContain('sales suggests')
    fireEvent.change(screen.getByTestId('thread-secret-site-input'), { target: { value: 'files.stripe.com' } })
    fireEvent.click(screen.getByTestId('thread-secret-site-add'))
    const after = screen.getAllByTestId('thread-secret-site')
    expect(after[1].getAttribute('data-by-agent')).toBe('false')
    expect(after[1].textContent).not.toContain('suggests')
    fireEvent.click(screen.getByLabelText('all my agents'))
    paste(VALUE)
    fireEvent.click(screen.getByTestId('thread-secret-submit'))
    await waitFor(() => expect(calls).toHaveLength(1))
    expect(calls[0]).toEqual({
      secret: VALUE,
      scope: 'server',
      hosts: ['https://api.stripe.com:443', 'files.stripe.com'],
    })
  })

  it('a scope only an admin may pick shows the daemon hint, with no "Save anyway"', async () => {
    const onSubmit = vi.fn(async () => {
      throw refusal('scope_not_allowed', 'only an Owner or Admin can share with all agents')
    })
    card({ hosts: ['https://api.stripe.com:443'] }, onSubmit)
    fireEvent.click(screen.getByLabelText('all my agents'))
    paste(VALUE)
    fireEvent.click(screen.getByTestId('thread-secret-submit'))
    expect((await screen.findByTestId('thread-secret-error')).textContent).toBe(
      'only an Owner or Admin can share with all agents',
    )
    expect(screen.getByTestId('thread-secret-submit').textContent).toBe('Save')
    expect(screen.queryByTestId('thread-secret-warning')).toBeNull()
  })

  it('Not now empties the field and declines', () => {
    const { onDismiss } = card({ hosts: ['https://api.stripe.com:443'] }, async () => {})
    paste(VALUE)
    fireEvent.click(screen.getByTestId('thread-secret-dismiss'))
    expect(field().value).toBe('')
    expect(onDismiss).toHaveBeenCalledTimes(1)
  })

  it('compact: the defaults in one line, More options, and the same answer body', async () => {
    const calls: (ThreadAnswerPayload & { secret: string })[] = []
    const more = vi.fn()
    card(
      { hosts: ['https://api.stripe.com:443'], scope: 'workspace', delivery: 'fetch' },
      async (p) => {
        calls.push({ ...p })
      },
      { compact: true, onMoreOptions: more },
    )
    expect(screen.queryByTestId('thread-secret-scope')).toBeNull()
    expect(screen.getByTestId('thread-secret-summary').textContent).toContain(
      'Kept for sales only. Only sent to api.stripe.com. K2 adds it to calls.',
    )
    fireEvent.click(screen.getByTestId('thread-secret-more'))
    expect(more).toHaveBeenCalledTimes(1)
    paste(VALUE)
    fireEvent.click(screen.getByTestId('thread-secret-submit'))
    await waitFor(() => expect(field().value).toBe(''))
    expect(calls).toEqual([{ secret: VALUE }])
  })

  it('an answered card shows the reference, never a value', () => {
    card({ status: 'set', ref: 'k2v://ws/sales/STRIPE_KEY' }, async () => {})
    expect(screen.queryByTestId('thread-secret-field')).toBeNull()
    expect(screen.getByTestId('thread-secret-set').textContent).toBe('STRIPE_KEY saved as k2v://ws/sales/STRIPE_KEY')
  })
})

describe('secret card helpers', () => {
  it('reads sites the way people do', () => {
    expect(siteLabel('https://api.stripe.com:443')).toBe('api.stripe.com')
    expect(siteLabel('https://api.example.com:8443')).toBe('api.example.com:8443')
    expect(siteLabel('http://intranet:80')).toBe('http://intranet')
    expect(providerForSites(['https://api.openai.com:443'])).toBe('OpenAI')
    expect(providerForSites(['https://api.example.com:443'])).toBeNull()
  })

  it('the answer body carries the value only in the POST body, choices only with a secret', () => {
    expect(
      threadAnswerBody('sales', 'c1', {
        secret: VALUE,
        scope: 'server',
        hosts: ['https://api.stripe.com:443', 'files.stripe.com'],
        delivery: 'fetch',
        confirm: true,
      }),
    ).toEqual({
      addr: 'sales',
      id: 'c1',
      secret: VALUE,
      scope: 'server',
      hosts: 'https://api.stripe.com:443,files.stripe.com',
      delivery: 'fetch',
      confirm: '1',
    })
    expect(threadAnswerBody('sales', 'c1', { answer: 'Go', confirm: true })).toEqual({ addr: 'sales', id: 'c1', answer: 'Go' })
  })

  it('a Thread snapshot keeps the card proposal and drops anything else on the card body', () => {
    const snap = threadItemsFromSnapshot({
      conversation_id: 'c',
      has_more: false,
      items: [
        {
          collection: 'thread',
          seq: 1,
          id: 's1',
          doc: {
            id: 's1',
            kind: 'secret',
            from: 'sales',
            secret: {
              name: 'STRIPE_KEY',
              status: 'pending',
              prompt: 'Paste it',
              scope: 'workspace',
              hosts: ['https://api.stripe.com:443'],
              delivery: 'fetch',
              secretKind: 'api_key',
              value: 'should-not-survive',
            },
          },
        },
      ],
    })
    expect(snap.items[0].doc.secret).toEqual({
      name: 'STRIPE_KEY',
      status: 'pending',
      prompt: 'Paste it',
      scope: 'workspace',
      hosts: ['https://api.stripe.com:443'],
      delivery: 'fetch',
      secretKind: 'api_key',
    })
  })
})
