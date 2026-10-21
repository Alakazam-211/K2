// @vitest-environment jsdom
// prd-k2-vault-v1 §10.1 / KV9 / VS12 / VS13: the K2-drawn secret card.
// 0.46.3 (Rosson 2026-10-21): the simple card. The agent's choice in one
// plain sentence, one field, one Save; Advanced (closed) holds the old
// controls. The wire-level regression (real daemon client) is in
// SecretCardWire.test.tsx.
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
  // The raw-body shape (daemonErrorOf reads it too; the DaemonHttpError
  // daemon-cli really throws is covered in SecretCardWire.test.tsx).
  return new Error(JSON.stringify({ ok: false, error: { code, hint } }))
}

const MISMATCH = 'this looks like a Stripe key, but the card sends it somewhere else; confirm to save anyway'

type Sent = ThreadAnswerPayload & { secret: string }

function card(secret: Partial<OverlaySecret>, onSubmit: (p: Sent) => Promise<void>, extra: Record<string, unknown> = {}) {
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

function plan(): string {
  return screen.getByTestId('thread-secret-plan').textContent ?? ''
}

function openAdvanced(): void {
  fireEvent.click(screen.getByTestId('thread-secret-advanced-toggle'))
}

function recorder(): { calls: Sent[]; onSubmit: (p: Sent) => Promise<void> } {
  const calls: Sent[] = []
  return {
    calls,
    onSubmit: async (p) => {
      calls.push({ ...p })
    },
  }
}

afterEach(() => cleanup())

describe('SecretCard: the simple view', () => {
  it('shows who asks, the prompt as plain text, the plain sentence, one field and the hint', () => {
    const long = `**bold** ${'x'.repeat(300)}`
    card({ prompt: long, hosts: ['https://api.stripe.com:443'] }, async () => {})
    expect(screen.getByTestId('thread-secret-header').textContent).toContain('sales asks for a secret: STRIPE_KEY')
    expect(screen.getByTestId('thread-secret-header').textContent).toContain('This computer')
    const prompt = screen.getByTestId('thread-secret-prompt')
    expect(prompt.textContent?.startsWith('sales says: **bold**')).toBe(true)
    expect(prompt.querySelector('strong')).toBeNull()
    expect(prompt.textContent!.length).toBe('sales says: '.length + 200)
    expect(plan()).toBe('K2 will add this to calls to api.stripe.com, for this workspace; sales never sees it.')
    expect(field().type).toBe('password')
    expect(screen.getByTestId('thread-secret-hint').textContent).toBe(
      'Not right? Tell sales in the chat; it can cancel this and ask again.',
    )
    expect(screen.getByTestId('thread-secret-submit').textContent).toBe('Save')
  })

  it('one click saves: the value once, with `disclosed`, out of the page, field cleared', async () => {
    const { calls, onSubmit } = recorder()
    const { container } = card({ hosts: ['https://api.stripe.com:443'] }, onSubmit)
    paste(VALUE)
    fireEvent.click(screen.getByTestId('thread-secret-submit'))
    await waitFor(() => expect(field().value).toBe(''))
    expect(calls).toEqual([{ secret: VALUE, disclosed: true }])
    expect(container.textContent).not.toContain(VALUE)
    expect(container.innerHTML).not.toContain(VALUE)
    expect((screen.getByTestId('thread-secret-submit') as HTMLButtonElement).disabled).toBe(true)
  })

  it('a readable delivery is said in the sentence and saves in one click (no second click)', async () => {
    const { calls, onSubmit } = recorder()
    card({ delivery: 'file', scope: 'workspace', secretKind: 'password', hosts: [] }, onSubmit)
    expect(plan()).toBe('sales will get this as a file it can read, for this workspace.')
    expect(screen.getByTestId('thread-secret-plan').querySelectorAll('strong')).toHaveLength(3)
    paste(VALUE)
    fireEvent.click(screen.getByTestId('thread-secret-submit'))
    await waitFor(() => expect(field().value).toBe(''))
    expect(calls).toEqual([{ secret: VALUE, disclosed: true }])
    expect(screen.queryByTestId('thread-secret-warning')).toBeNull()
  })

  it('says each choice in plain words', () => {
    card({ delivery: 'env', scope: 'server' }, async () => {})
    expect(plan()).toBe('sales will get this as an environment variable it can read, for all your agents.')
    cleanup()
    card({ delivery: 'fetch', scope: 'session', hosts: ['https://api.stripe.com:443', 'https://files.stripe.com:443'] }, async () => {})
    expect(plan()).toBe(
      'K2 will add this to calls to api.stripe.com and files.stripe.com, for this chat only; sales never sees it.',
    )
  })
})

describe('SecretCard: Advanced', () => {
  it('is closed by default; opening it shows the old controls', () => {
    card({ hosts: ['https://api.stripe.com:443'] }, async () => {})
    const toggle = screen.getByTestId('thread-secret-advanced-toggle')
    expect(toggle.getAttribute('aria-expanded')).toBe('false')
    expect(screen.queryByTestId('thread-secret-advanced')).toBeNull()
    expect(screen.queryByTestId('thread-secret-scope')).toBeNull()
    expect(screen.queryByTestId('thread-secret-delivery')).toBeNull()
    expect(screen.queryByTestId('thread-secret-sites')).toBeNull()
    openAdvanced()
    expect(toggle.getAttribute('aria-expanded')).toBe('true')
    expect(screen.getByTestId('thread-secret-scope')).toBeTruthy()
    expect(screen.getByTestId('thread-secret-delivery')).toBeTruthy()
    expect(screen.getByTestId('thread-secret-sites')).toBeTruthy()
    expect(screen.getByTestId('thread-secret-honest').textContent).toBe(
      'This key can still do anything Stripe allows it to. Use a restricted key if you can.',
    )
  })

  it('changing an option updates the sentence, and the change is sent', async () => {
    const { calls, onSubmit } = recorder()
    card({ hosts: ['https://api.stripe.com:443'], scope: 'workspace', delivery: 'fetch' }, onSubmit)
    openAdvanced()
    fireEvent.click(screen.getByLabelText(/an environment variable sales can read/))
    expect(plan()).toBe('sales will get this as an environment variable it can read, for this workspace.')
    expect(screen.getByTestId('thread-secret-readable').textContent).toBe('sales can see this value')
    fireEvent.click(screen.getByLabelText('this chat'))
    expect(plan()).toBe('sales will get this as an environment variable it can read, for this chat only.')
    fireEvent.click(screen.getByLabelText(/K2 adds it to calls/))
    fireEvent.change(screen.getByTestId('thread-secret-site-input'), { target: { value: 'files.stripe.com' } })
    fireEvent.click(screen.getByTestId('thread-secret-site-add'))
    expect(plan()).toBe(
      'K2 will add this to calls to api.stripe.com and files.stripe.com, for this chat only; sales never sees it.',
    )
    paste(VALUE)
    fireEvent.click(screen.getByTestId('thread-secret-submit'))
    await waitFor(() => expect(calls).toHaveLength(1))
    expect(calls[0]).toEqual({
      secret: VALUE,
      scope: 'session',
      hosts: ['https://api.stripe.com:443', 'files.stripe.com'],
      disclosed: true,
    })
  })

  it('labels the agent\'s sites "<agent> suggests"; untouched choices are not sent', async () => {
    const { calls, onSubmit } = recorder()
    card({ hosts: ['https://api.stripe.com:443'], scope: 'workspace' }, onSubmit)
    openAdvanced()
    const chips = screen.getAllByTestId('thread-secret-site')
    expect(chips).toHaveLength(1)
    expect(chips[0].textContent).toContain('api.stripe.com')
    expect(chips[0].textContent).toContain('sales suggests')
    fireEvent.change(screen.getByTestId('thread-secret-site-input'), { target: { value: 'files.stripe.com' } })
    fireEvent.click(screen.getByTestId('thread-secret-site-add'))
    const after = screen.getAllByTestId('thread-secret-site')
    expect(after[1].getAttribute('data-by-agent')).toBe('false')
    expect(after[1].textContent).not.toContain('suggests')
    fireEvent.click(screen.getByLabelText('Remove files.stripe.com'))
    paste(VALUE)
    fireEvent.click(screen.getByTestId('thread-secret-submit'))
    await waitFor(() => expect(calls).toHaveLength(1))
    expect(calls[0]).toEqual({ secret: VALUE, disclosed: true })
  })

  it('no site with "K2 adds it to calls": the sentence says so and Save waits for a site or a readable choice', () => {
    card({ hosts: [] }, async () => {})
    paste(VALUE)
    const save = screen.getByTestId('thread-secret-submit') as HTMLButtonElement
    expect(save.disabled).toBe(true)
    expect(plan()).toBe("sales didn't say which site this is for, so K2 can't use it yet. Open Advanced to add one.")
    openAdvanced()
    fireEvent.click(screen.getByLabelText(/an environment variable sales can read/))
    expect(save.disabled).toBe(false)
    fireEvent.click(screen.getByLabelText(/K2 adds it to calls/))
    expect(save.disabled).toBe(true)
    fireEvent.change(screen.getByTestId('thread-secret-site-input'), { target: { value: 'api.example.com' } })
    fireEvent.click(screen.getByTestId('thread-secret-site-add'))
    expect(save.disabled).toBe(false)
    expect(plan()).toBe('K2 will add this to calls to api.example.com, for this workspace; sales never sees it.')
  })
})

describe('SecretCard: the host-mismatch guard', () => {
  it('still needs a second click, in plain words; the second click resends the same value with confirm', async () => {
    const calls: Sent[] = []
    const onSubmit = vi.fn(async (p: Sent) => {
      calls.push({ ...p })
      if (calls.length === 1) throw refusal('card_host_mismatch', MISMATCH)
    })
    card({ hosts: ['https://api.example.com:443'] }, onSubmit)
    paste(VALUE)
    fireEvent.click(screen.getByTestId('thread-secret-submit'))
    const warning = await screen.findByTestId('thread-secret-warning')
    expect(warning.textContent).toBe(
      "Check this first. This looks like a Stripe key, but it would be sent to api.example.com. Save it only if that's right. If sales set this up wrong, tell it in the chat instead.",
    )
    expect(screen.getByTestId('thread-secret-card').getAttribute('data-warning')).toBe('card_host_mismatch')
    expect(screen.getByTestId('thread-secret-submit').textContent).toBe('Save anyway')
    // The field still holds it for the second click; nothing else does.
    expect(field().value).toBe(VALUE)
    fireEvent.click(screen.getByTestId('thread-secret-submit'))
    await waitFor(() => expect(field().value).toBe(''))
    expect(calls).toEqual([
      { secret: VALUE, disclosed: true },
      { secret: VALUE, disclosed: true, confirm: true },
    ])
    expect(screen.queryByTestId('thread-secret-warning')).toBeNull()
  })

  it('a live re-render between the clicks keeps the warning and the field (0.46.2 regression)', async () => {
    const calls: Sent[] = []
    const onSubmit = async (p: Sent): Promise<void> => {
      calls.push({ ...p })
      if (calls.length === 1) throw refusal('card_host_mismatch', MISMATCH)
    }
    const proposal: OverlaySecret = { name: 'STRIPE_KEY', status: 'pending', hosts: ['https://api.example.com:443'] }
    const el = (s: OverlaySecret) => (
      <SecretCard secret={s} agent="sales" workspace="sales" onSubmit={onSubmit} onDismiss={() => {}} />
    )
    const { rerender } = render(el(proposal))
    paste(VALUE)
    fireEvent.click(screen.getByTestId('thread-secret-submit'))
    await screen.findByTestId('thread-secret-warning')
    rerender(el({ ...proposal, hosts: [...(proposal.hosts ?? [])] }))
    rerender(el({ ...proposal, prompt: null }))
    expect(field().value).toBe(VALUE)
    expect(screen.getByTestId('thread-secret-submit').textContent).toBe('Save anyway')
    fireEvent.click(screen.getByTestId('thread-secret-submit'))
    await waitFor(() => expect(calls).toHaveLength(2))
    expect(calls[1].confirm).toBe(true)
  })

  it('changing a choice drops the warning (no stale confirm)', async () => {
    const calls: Sent[] = []
    card({ hosts: ['https://api.example.com:443'] }, async (p) => {
      calls.push({ ...p })
      throw refusal('card_host_mismatch', MISMATCH)
    })
    paste(VALUE)
    fireEvent.click(screen.getByTestId('thread-secret-submit'))
    await screen.findByTestId('thread-secret-warning')
    openAdvanced()
    fireEvent.change(screen.getByTestId('thread-secret-site-input'), { target: { value: 'api.stripe.com' } })
    fireEvent.click(screen.getByTestId('thread-secret-site-add'))
    fireEvent.click(screen.getByLabelText('Remove api.example.com'))
    expect(screen.queryByTestId('thread-secret-warning')).toBeNull()
    expect(screen.getByTestId('thread-secret-submit').textContent).toBe('Save')
    fireEvent.click(screen.getByTestId('thread-secret-submit'))
    await waitFor(() => expect(calls).toHaveLength(2))
    expect(calls[1]).toEqual({ secret: VALUE, hosts: ['api.stripe.com'], disclosed: true })
  })
})

describe('SecretCard: other answers', () => {
  it('a scope only an admin may pick shows the daemon hint, with no "Save anyway"', async () => {
    const onSubmit = vi.fn(async () => {
      throw refusal('scope_not_allowed', 'only an Owner or Admin can share with all agents')
    })
    card({ hosts: ['https://api.stripe.com:443'] }, onSubmit)
    openAdvanced()
    fireEvent.click(screen.getByLabelText('all my agents'))
    paste(VALUE)
    fireEvent.click(screen.getByTestId('thread-secret-submit'))
    expect((await screen.findByTestId('thread-secret-error')).textContent).toBe(
      'only an Owner or Admin can share with all agents',
    )
    expect(screen.getByTestId('thread-secret-submit').textContent).toBe('Save')
    expect(screen.queryByTestId('thread-secret-warning')).toBeNull()
  })

  it('a readable refusal that survives the retry is shown as an error, not a loop', async () => {
    const calls: Sent[] = []
    card({ delivery: 'file', hosts: [] }, async (p) => {
      calls.push({ ...p })
      throw refusal('card_readable_delivery', 'the agent will be able to see this value; confirm to save')
    })
    paste(VALUE)
    fireEvent.click(screen.getByTestId('thread-secret-submit'))
    expect((await screen.findByTestId('thread-secret-error')).textContent).toBe(
      'the agent will be able to see this value; confirm to save',
    )
    expect(calls).toEqual([
      { secret: VALUE, disclosed: true },
      { secret: VALUE, disclosed: true, confirm: true },
    ])
    expect(screen.getByTestId('thread-secret-submit').textContent).toBe('Save')
  })

  it('Not now empties the field and declines', () => {
    const { onDismiss } = card({ hosts: ['https://api.stripe.com:443'] }, async () => {})
    paste(VALUE)
    fireEvent.click(screen.getByTestId('thread-secret-dismiss'))
    expect(field().value).toBe('')
    expect(onDismiss).toHaveBeenCalledTimes(1)
  })

  it('compact: the same sentence and one-click Save, More options instead of Advanced', async () => {
    const { calls, onSubmit } = recorder()
    const more = vi.fn()
    card(
      { hosts: ['https://api.stripe.com:443'], scope: 'workspace', delivery: 'fetch' },
      onSubmit,
      { compact: true, onMoreOptions: more },
    )
    expect(screen.queryByTestId('thread-secret-advanced-toggle')).toBeNull()
    expect(plan()).toBe('K2 will add this to calls to api.stripe.com, for this workspace; sales never sees it.')
    fireEvent.click(screen.getByTestId('thread-secret-more'))
    expect(more).toHaveBeenCalledTimes(1)
    paste(VALUE)
    fireEvent.click(screen.getByTestId('thread-secret-submit'))
    await waitFor(() => expect(field().value).toBe(''))
    expect(calls).toEqual([{ secret: VALUE, disclosed: true }])
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
        disclosed: true,
      }),
    ).toEqual({
      addr: 'sales',
      id: 'c1',
      secret: VALUE,
      scope: 'server',
      hosts: 'https://api.stripe.com:443,files.stripe.com',
      delivery: 'fetch',
      confirm: '1',
      disclosed: '1',
    })
    // Without the sentence (older cards, the CLI) nothing is disclosed.
    expect(threadAnswerBody('sales', 'c1', { secret: VALUE })).toEqual({ addr: 'sales', id: 'c1', secret: VALUE })
    expect(threadAnswerBody('sales', 'c1', { answer: 'Go', confirm: true, disclosed: true })).toEqual({
      addr: 'sales',
      id: 'c1',
      answer: 'Go',
    })
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
