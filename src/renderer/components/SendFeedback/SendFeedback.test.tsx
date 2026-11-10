// @vitest-environment jsdom
//
// Send feedback to K2 (prd-k2-product-feedback-v1 PF3, PF5, PF7, PF25–PF27):
// the top-bar button (Help's size and classes, right of Help), and the
// dialog against a mocked daemon:
//   - nothing about the computer is collected until the box is ticked;
//   - ticking collects once; "Leave out" drops a section from what is sent,
//     and the send names the preview by its draft id;
//   - a server that can't send says why and offers Copy as text;
//   - the Sent tab lists rows with their state and refetches on the event.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({
  settings: {} as Record<string, unknown>,
  calls: [] as Array<{ method: string; route: string; body?: unknown }>,
  items: [] as unknown[],
  statusHandler: null as null | ((e: unknown) => void),
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/app', () => ({ getVersion: vi.fn(async () => '0.46.11-test') }))
vi.mock('@/lib/is-web', () => ({ isWebClient: () => false }))
vi.mock('@/kessel/server-scope', () => ({
  primaryScope: () => ({
    id: 'primary',
    isPrimary: true,
    hostKey: 'local',
    isRemote: false,
    label: 'This computer',
    serverSupports: () => true,
  }),
}))
vi.mock('@/stores/session-events', () => ({
  onProductFeedbackStatus: (_scope: unknown, fn: (e: unknown) => void) => {
    h.statusHandler = fn
    return () => {
      h.statusHandler = null
    }
  },
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (_scope: unknown, route: string) => {
    h.calls.push({ method: 'GET', route })
    if (route === 'product-feedback/settings') return h.settings
    if (route === 'product-feedback/list') return { items: h.items, all: false }
    throw new Error(`unexpected GET ${route}`)
  }),
  daemonCliPost: vi.fn(async (_scope: unknown, route: string, body?: unknown) => {
    h.calls.push({ method: 'POST', route, body })
    if (route === 'product-feedback/diagnostics') {
      return {
        draftId: 'pfd_1',
        bytes: 40,
        sections: [
          { id: 'versions', title: 'Versions', text: 'Daemon: 0.46.11', bytes: 15 },
          { id: 'log-tail', title: "End of K2's log", text: 'line 1\nline 2', bytes: 13 },
        ],
      }
    }
    if (route === 'product-feedback/send') return { id: 'pf_0123456789012345678901', state: 'queued' }
    if (route === 'product-feedback/settings') return { ...h.settings, ...(body as object) }
    throw new Error(`unexpected POST ${route}`)
  }),
}))

import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import SendFeedbackButton from './SendFeedbackButton'
import SendFeedbackDialog from './SendFeedbackDialog'
import { FeedbackIcon, TOP_BAR_ICON_ACTIVE_CLASS, TOP_BAR_ICON_REST_CLASS } from '@/components/icons/K2Icons'
import { useSendFeedbackStore } from '@/stores/send-feedback'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const CAN_SEND = {
  enabled: true,
  agents: true,
  agentDiagnostics: false,
  rememberDiagnostics: false,
  canEdit: true,
  canSend: true,
  why: null,
  message: null,
  endpoint: 'https://feedback.akzm.k2.dev',
  pending: 0,
  service: { state: 'unknown' },
}

beforeEach(() => {
  h.settings = { ...CAN_SEND }
  h.calls = []
  h.items = []
  h.statusHandler = null
  useSendFeedbackStore.setState({ open: true, tab: 'send', openedFrom: 'Agents' })
})

afterEach(() => {
  cleanup()
  useSendFeedbackStore.setState({ open: false, tab: 'send', openedFrom: null })
})

const posts = (route: string): unknown[] => h.calls.filter((c) => c.method === 'POST' && c.route === route).map((c) => c.body)

describe('the top-bar button', () => {
  it('draws message-square at Help\'s size and classes, labelled, and opens the dialog', () => {
    useSendFeedbackStore.setState({ open: false })
    render(<SendFeedbackButton />)
    const b = screen.getByRole('button', { name: 'Send feedback to K2' })
    expect(b.getAttribute('title')).toBe('Send feedback to K2')
    const svg = b.querySelector('svg')
    expect(svg?.getAttribute('data-k2-icon')).toBe('feedback')
    expect(svg?.getAttribute('width')).toBe('16')
    for (const cls of ['h-6', 'w-6', ...TOP_BAR_ICON_REST_CLASS.split(' ')]) expect([cls, b.classList.contains(cls)]).toEqual([cls, true])
    fireEvent.click(b)
    expect(useSendFeedbackStore.getState().open).toBe(true)
    expect(b.getAttribute('aria-pressed')).toBe('true')
    for (const cls of TOP_BAR_ICON_ACTIVE_CLASS.split(' ')) expect([cls, b.classList.contains(cls)]).toEqual([cls, true])
  })

  it('the icon is one squared path', () => {
    render(<FeedbackIcon />)
    expect(document.querySelectorAll('svg path')).toHaveLength(1)
  })
})

describe('the dialog', () => {
  it('collects nothing until the box is ticked, then sends what is left in', async () => {
    render(<SendFeedbackDialog />)
    await waitFor(() => expect(h.calls.some((c) => c.route === 'product-feedback/settings')).toBe(true))
    expect(screen.getByTestId('send-feedback-whats-sent').textContent).toBe(
      'Sent to Alakazam Labs with your K2 and OS versions. Nothing else from your computer unless you tick the box below.',
    )
    expect(screen.getByTestId('send-feedback-through').textContent).toBe('Sent through This computer')
    const diag = screen.getByTestId('send-feedback-diagnostics') as HTMLInputElement
    expect(diag.checked).toBe(false)
    expect(posts('product-feedback/diagnostics')).toEqual([])

    fireEvent.change(screen.getByTestId('send-feedback-text'), { target: { value: 'The Wiki tab froze.' } })
    await act(async () => {
      fireEvent.click(diag)
    })
    await waitFor(() => expect(posts('product-feedback/diagnostics')).toHaveLength(1))
    expect(posts('product-feedback/diagnostics')[0]).toEqual({
      client: 'desktop',
      page: 'Agents',
      k2Version: '0.46.11-test',
      includeClientFaults: true,
    })
    await waitFor(() => expect((screen.getByTestId('send-feedback-preview-toggle') as HTMLButtonElement).disabled).toBe(false))
    fireEvent.click(screen.getByTestId('send-feedback-preview-toggle'))
    expect(screen.getByTestId('send-feedback-preview').textContent).toContain('Daemon: 0.46.11')
    fireEvent.click(screen.getByTestId('send-feedback-leave-out-log-tail'))
    // The remember box appears once the first is ticked (PF27).
    expect(screen.getByTestId('send-feedback-remember')).toBeTruthy()

    await act(async () => {
      fireEvent.click(screen.getByTestId('send-feedback-send'))
    })
    await waitFor(() => expect(posts('product-feedback/send')).toHaveLength(1))
    expect(posts('product-feedback/send')[0]).toEqual({
      category: 'bug',
      text: 'The Wiki tab froze.',
      client: 'desktop',
      k2Version: '0.46.11-test',
      locale: navigator.language,
      diagnostics: { draftId: 'pfd_1', drop: ['log-tail'], edited: {} },
    })
    expect(posts('product-feedback/diagnostics')).toHaveLength(1)
    await waitFor(() => expect(useSendFeedbackStore.getState().tab).toBe('sent'))
  })

  it('starts with the ask-your-agent hint, and the picker is Bug, Idea, Other (no Praise)', async () => {
    render(<SendFeedbackDialog />)
    await waitFor(() => expect(screen.getByTestId('send-feedback-ask-agent').textContent).toBe(
      'Tip: in any agent\u2019s chat you can say \u201cSend this issue as feedback to K2\u201d, and the agent sends it for you.',
    ))
    const picker = screen.getByTestId('send-feedback-category')
    expect(Array.from(picker.querySelectorAll('button')).map((b) => b.textContent)).toEqual(['Bug', 'Idea', 'Other'])
  })

  it('no ask-your-agent hint while the owner keeps agents off', async () => {
    h.settings = { ...CAN_SEND, agents: false }
    render(<SendFeedbackDialog />)
    await waitFor(() => expect(h.calls.some((c) => c.route === 'product-feedback/settings')).toBe(true))
    expect(screen.queryByTestId('send-feedback-ask-agent')).toBeNull()
  })

  it('without the tick, the send carries no diagnostics', async () => {
    render(<SendFeedbackDialog />)
    await waitFor(() => expect(h.calls.some((c) => c.route === 'product-feedback/settings')).toBe(true))
    fireEvent.change(screen.getByTestId('send-feedback-text'), { target: { value: 'An idea' } })
    await act(async () => {
      fireEvent.click(screen.getByTestId('send-feedback-send'))
    })
    await waitFor(() => expect(posts('product-feedback/send')).toHaveLength(1))
    expect((posts('product-feedback/send')[0] as Record<string, unknown>).diagnostics).toBeUndefined()
    expect(posts('product-feedback/diagnostics')).toEqual([])
  })

  it('a server that can\'t send says why and offers Copy as text', async () => {
    h.settings = { ...CAN_SEND, canSend: false, why: 'needs_connect', message: "This server isn't paired with K2 Connect." }
    render(<SendFeedbackDialog />)
    await waitFor(() => expect(screen.getByTestId('send-feedback-unavailable').textContent).toContain("isn't paired with K2 Connect"))
    expect(screen.getByTestId('send-feedback-unavailable').textContent).toContain('hello@alakazamlabs.com')
    fireEvent.change(screen.getByTestId('send-feedback-text'), { target: { value: 'kept' } })
    expect((screen.getByTestId('send-feedback-send') as HTMLButtonElement).disabled).toBe(true)
    expect((screen.getByTestId('send-feedback-copy') as HTMLButtonElement).disabled).toBe(false)
  })

  it('the service not reachable yet is said plainly', async () => {
    h.settings = { ...CAN_SEND, service: { state: 'unreachable', message: "K2's feedback service isn't reachable yet. Your feedback is saved on this server and sends when it is." } }
    render(<SendFeedbackDialog />)
    await waitFor(() => expect(screen.getByTestId('send-feedback-service').textContent).toContain("isn't reachable yet"))
    expect((screen.getByTestId('send-feedback-send') as HTMLButtonElement).disabled).toBe(true)
    fireEvent.change(screen.getByTestId('send-feedback-text'), { target: { value: 'still saved' } })
    expect((screen.getByTestId('send-feedback-send') as HTMLButtonElement).disabled).toBe(false)
  })

  it('a remembered choice starts ticked and collects at once', async () => {
    h.settings = { ...CAN_SEND, rememberDiagnostics: true }
    render(<SendFeedbackDialog />)
    await waitFor(() => expect((screen.getByTestId('send-feedback-diagnostics') as HTMLInputElement).checked).toBe(true))
    await waitFor(() => expect(posts('product-feedback/diagnostics')).toHaveLength(1))
  })

  it('the Sent tab shows each row and refetches on the event', async () => {
    h.items = [
      { id: 'pf_a', createdAt: 1, category: 'bug', text: 'first', state: 'queued', mine: true, errorMessage: "K2's feedback service isn't reachable yet." },
      { id: 'pf_b', createdAt: 2, category: 'idea', text: 'second', state: 'failed', mine: true },
    ]
    useSendFeedbackStore.setState({ tab: 'sent' })
    render(<SendFeedbackDialog />)
    await waitFor(() => expect(screen.getAllByTestId('send-feedback-row')).toHaveLength(2))
    const rows = screen.getAllByTestId('send-feedback-row')
    expect(rows.map((r) => r.getAttribute('data-state'))).toEqual(['queued', 'failed'])
    expect(rows[0].textContent).toContain("isn't reachable yet")
    expect(rows[1].textContent).toContain('Retry')
    const lists = h.calls.filter((c) => c.route === 'product-feedback/list').length
    h.items = [{ id: 'pf_a', createdAt: 1, category: 'bug', text: 'first', state: 'sent', mine: true }]
    await act(async () => {
      h.statusHandler?.({ kind: 'product_feedback_status', id: 'pf_a', state: 'sent', error: null })
    })
    await waitFor(() => expect(h.calls.filter((c) => c.route === 'product-feedback/list').length).toBe(lists + 1))
    await waitFor(() => expect(screen.getAllByTestId('send-feedback-row')[0].getAttribute('data-state')).toBe('sent'))
  })
})
