// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { resetSubscriptionUsageForTests } from '@/stores/subscription-usage'
import type { SubscriptionDoc } from '@/lib/subscription-usage'

const h = vi.hoisted(() => ({
  remote: false,
  daemonCliGet: vi.fn(),
  daemonCliPost: vi.fn(),
}))

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: h.daemonCliGet,
  daemonCliPost: h.daemonCliPost,
}))

vi.mock('@/stores/connect-host', () => ({
  useConnectHostStore: (sel: (s: { activeHost: 'local' | { id: string } }) => unknown) =>
    sel({ activeHost: h.remote ? { id: 'remote-box' } : 'local' }),
}))

import UsageButton from './UsageButton'

function claudeDoc(checkedAt: string, extra: Partial<SubscriptionDoc['harnesses'][number]> = {}): SubscriptionDoc {
  return {
    harnesses: [
      {
        harness: 'claude',
        plan: 'Max 20x',
        windows: [
          { label: 'Weekly', used: 0.31, resetsAt: '2026-10-03T00:00:00Z' },
          { label: 'Opus Weekly', used: 0.12, resetsAt: '2026-10-03T00:00:00Z' },
        ],
        checkedAt,
        status: '',
        ...extra,
      },
      {
        harness: 'codex',
        plan: 'plus',
        windows: [],
        checkedAt,
        status: 'Not signed in',
      },
      {
        harness: 'grok',
        plan: 'SuperGrok',
        windows: [],
        checkedAt,
        status: 'Not signed in',
      },
    ],
  }
}

beforeEach(() => {
  cleanup()
  h.remote = false
  h.daemonCliGet.mockReset()
  h.daemonCliPost.mockReset()
  resetSubscriptionUsageForTests()
})

describe('UsageButton', () => {
  it('shows Claude 31% used and keeps the model-scoped row, not Grok', async () => {
    const checkedAt = new Date().toISOString()
    h.daemonCliGet.mockResolvedValue(claudeDoc(checkedAt))
    render(<UsageButton />)
    await waitFor(() => {
      expect(screen.getByTestId('subscription-usage').textContent).toContain('31%')
      expect(screen.getByTestId('subscription-usage').querySelector('svg')).toBeTruthy()
    })
    expect(screen.getByTestId('subscription-usage').getAttribute('aria-label')).toBe(
      'Subscription usage',
    )
    fireEvent.click(screen.getByTestId('subscription-usage'))
    const menu = await screen.findByTestId('subscription-usage-menu')
    expect(menu.textContent).toContain('Claude · Max 20x')
    expect(menu.textContent).toContain('Weekly 31%')
    expect(menu.textContent).toContain('Opus Weekly 12%')
    expect(menu.textContent).not.toContain('% left')
    const bars = menu.querySelectorAll('[role="progressbar"]')
    expect(bars).toHaveLength(2)
    expect(bars[0]?.getAttribute('aria-valuenow')).toBe('31')
    expect(bars[1]?.getAttribute('aria-valuenow')).toBe('12')
    expect(menu.textContent).not.toContain('Not signed in')
    expect(menu.textContent).not.toContain('Codex')
    expect(menu.textContent).not.toContain('Grok')
    expect(menu.textContent).not.toContain('0%')
    expect(h.daemonCliPost).not.toHaveBeenCalled()
    expect(screen.getByTestId('subscription-usage-refresh').textContent).toBe('Refresh')
  })

  it('puts every signed-in harness on the top bar', async () => {
    const checkedAt = new Date().toISOString()
    h.daemonCliGet.mockResolvedValue({
      harnesses: [
        {
          harness: 'claude',
          plan: 'Max 20x',
          windows: [{ label: 'Weekly', used: 0.31, resetsAt: '2026-10-03T00:00:00Z' }],
          checkedAt,
          status: '',
        },
        {
          harness: 'codex',
          plan: 'plus',
          windows: [{ label: 'Weekly', used: 0.4, resetsAt: '2026-10-03T00:00:00Z' }],
          checkedAt,
          status: '',
        },
        {
          harness: 'grok',
          plan: 'SuperGrok Heavy',
          windows: [{ label: 'Weekly', used: 0.05, resetsAt: '2026-10-03T00:00:00Z' }],
          checkedAt,
          status: '',
        },
      ],
    })
    render(<UsageButton />)
    await waitFor(() => {
      expect(screen.getByTestId('usage-chip-claude').textContent).toContain('31%')
      expect(screen.getByTestId('usage-chip-codex').textContent).toContain('40%')
      expect(screen.getByTestId('usage-chip-grok').textContent).toContain('5%')
    })
    expect(screen.getByTestId('subscription-usage').querySelectorAll('svg')).toHaveLength(3)
    fireEvent.click(screen.getByTestId('subscription-usage'))
    const menu = await screen.findByTestId('subscription-usage-menu')
    const rules = menu.querySelectorAll('section')
    expect(rules).toHaveLength(3)
    expect(rules[1]?.className).toContain('border-t')
    expect(rules[2]?.className).toContain('border-t')
    expect(rules[0]?.className).not.toContain('border-t')
  })

  it('refresh asks the daemon to probe again', async () => {
    const checkedAt = new Date().toISOString()
    h.daemonCliGet.mockResolvedValue(claudeDoc(checkedAt))
    h.daemonCliPost.mockResolvedValue(claudeDoc(checkedAt))
    render(<UsageButton />)
    await waitFor(() => {
      expect(screen.getByTestId('subscription-usage').textContent).toContain('31%')
      expect(screen.getByTestId('subscription-usage').querySelector('svg')).toBeTruthy()
    })
    fireEvent.click(screen.getByTestId('subscription-usage'))
    await screen.findByTestId('subscription-usage-menu')
    fireEvent.click(screen.getByTestId('subscription-usage-refresh'))
    await waitFor(() => {
      expect(h.daemonCliPost).toHaveBeenCalledWith('usage/subscriptions/refresh', {})
    })
  })

  it('says nothing is signed in when no harness is signed in', async () => {
    const checkedAt = new Date().toISOString()
    h.daemonCliGet.mockResolvedValue({
      harnesses: [
        {
          harness: 'claude',
          plan: '',
          windows: [],
          checkedAt,
          status: 'Not signed in',
        },
        {
          harness: 'codex',
          plan: '',
          windows: [],
          checkedAt,
          status: 'Sign-in expired',
        },
      ],
    })
    render(<UsageButton />)
    await waitFor(() => {
      expect(screen.getByTestId('subscription-usage').textContent).toBe('Usage')
    })
    fireEvent.click(screen.getByTestId('subscription-usage'))
    const menu = await screen.findByTestId('subscription-usage-menu')
    expect(menu.textContent).toContain('Nothing is signed in')
    expect(menu.textContent).not.toContain('Claude')
    expect(menu.textContent).not.toContain('Codex')
    expect(menu.textContent).not.toContain('Not signed in')
    expect(menu.textContent).not.toContain('Sign-in expired')
    expect(menu.textContent).not.toContain('0%')
  })

  it('shows No usage window without inventing a percent', async () => {
    const checkedAt = new Date().toISOString()
    const noWindow = {
      harnesses: [
        {
          harness: 'claude',
          plan: 'Max 20x',
          windows: [],
          checkedAt,
          status: 'No usage window',
        },
        {
          harness: 'codex',
          plan: '',
          windows: [],
          checkedAt,
          status: 'Not signed in',
        },
      ],
    }
    h.daemonCliGet.mockResolvedValue(noWindow)
    h.daemonCliPost.mockResolvedValue(noWindow)
    render(<UsageButton />)
    await waitFor(() => {
      expect(h.daemonCliGet).toHaveBeenCalled()
    })
    fireEvent.click(screen.getByTestId('subscription-usage'))
    const menu = await screen.findByTestId('subscription-usage-menu')
    await waitFor(() => {
      expect(menu.textContent).toContain('No usage window')
    })
    expect(menu.textContent).toContain('Claude')
    expect(menu.textContent).not.toContain('Codex')
    expect(menu.textContent).not.toContain('% left')
  })

  it('POSTs refresh when checkedAt is older than 15 seconds', async () => {
    const stale = new Date(Date.now() - 16_000).toISOString()
    const fresh = new Date().toISOString()
    h.daemonCliGet.mockResolvedValue(claudeDoc(stale))
    h.daemonCliPost.mockResolvedValue(claudeDoc(fresh))
    render(<UsageButton />)
    await waitFor(() => {
      expect(screen.getByTestId('subscription-usage').textContent).toContain('31%')
      expect(screen.getByTestId('subscription-usage').querySelector('svg')).toBeTruthy()
    })
    fireEvent.click(screen.getByTestId('subscription-usage'))
    await waitFor(() => {
      expect(h.daemonCliPost).toHaveBeenCalledWith('usage/subscriptions/refresh', {})
    })
  })

  it('says the numbers are the remote host’s, not this laptop’s', async () => {
    h.remote = true
    const checkedAt = new Date().toISOString()
    h.daemonCliGet.mockResolvedValue(claudeDoc(checkedAt))
    render(<UsageButton />)
    await waitFor(() => {
      expect(screen.getByTestId('subscription-usage').textContent).toContain('31%')
      expect(screen.getByTestId('subscription-usage').querySelector('svg')).toBeTruthy()
    })
    fireEvent.click(screen.getByTestId('subscription-usage'))
    const menu = await screen.findByTestId('subscription-usage-menu')
    expect(menu.textContent).toContain("this host's")
    expect(menu.textContent).toContain("not this laptop's")
  })
})
