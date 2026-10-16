// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { resetSubscriptionUsageForTests } from '@/stores/subscription-usage'
import type { SubscriptionDoc } from '@/lib/subscription-usage'

const h = vi.hoisted(() => ({
  remote: false,
  daemonCliGet: vi.fn(),
  daemonCliPost: vi.fn(),
  openSettings: vi.fn(),
}))

vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly((...a: unknown[]) => h.daemonCliGet(...a)),
    daemonCliPost: primaryOnly((...a: unknown[]) => h.daemonCliPost(...a)),
  }
})

vi.mock('@/lib/open-settings-page', () => ({
  openSettingsPage: async (section: string) => h.openSettings(section),
}))

vi.mock('@/stores/connect-host', () => ({
  useConnectHostStore: (sel: (s: { activeHost: 'local' | { id: string; label: string; hostname: string } }) => unknown) =>
    sel({ activeHost: h.remote ? { id: 'remote-box', label: 'Remote box', hostname: 'box.k2.dev' } : 'local' }),
  // The stores drop the window server's entry on a top-switcher change.
  onActiveHostChange: () => () => {},
}))

import UsageButton from './UsageButton'

function claudeDoc(checkedAt: string, extra: Partial<SubscriptionDoc['harnesses'][number]> = {}): SubscriptionDoc {
  return {
    harnesses: [
      {
        harness: 'claude',
        plan: 'Max 20x',
        windows: [
          { label: 'Weekly', used: 0.31, resetsAt: '2099-10-03T00:00:00Z' },
          { label: 'Opus Weekly', used: 0.12, resetsAt: '2099-10-03T00:00:00Z' },
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

/** Answer the GET and the menu's refresh POST with the same body. */
function serve(body: unknown): void {
  h.daemonCliGet.mockResolvedValue(body)
  h.daemonCliPost.mockResolvedValue(body)
}

beforeEach(() => {
  cleanup()
  h.remote = false
  h.daemonCliGet.mockReset()
  h.daemonCliPost.mockReset()
  h.openSettings.mockReset()
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
          windows: [{ label: 'Weekly', used: 0.31, resetsAt: '2099-10-03T00:00:00Z' }],
          checkedAt,
          status: '',
        },
        {
          harness: 'codex',
          plan: 'plus',
          windows: [{ label: 'Weekly', used: 0.4, resetsAt: '2099-10-03T00:00:00Z' }],
          checkedAt,
          status: '',
        },
        {
          harness: 'grok',
          plan: 'SuperGrok Heavy',
          windows: [{ label: 'Weekly', used: 0.05, resetsAt: '2099-10-03T00:00:00Z' }],
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
    expect(rules[1]?.className).toContain('-mx-3')
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
    expect(menu.textContent).not.toContain('Not signed in')
    expect(menu.textContent).not.toContain('0%')
  })

  it('keeps an expired sign-in listed with Sign in again, and marks the chip (B22, P8)', async () => {
    const checkedAt = new Date(Date.now() - 9 * 60_000).toISOString()
    serve({
      harnesses: [
        { harness: 'claude', plan: '', windows: [], checkedAt, status: 'Not signed in' },
        { harness: 'codex', plan: 'plus', windows: [], checkedAt, status: 'Sign-in expired', state: 'expired' },
      ],
    })
    render(<UsageButton />)
    await waitFor(() => {
      expect(screen.getByTestId('usage-warning')).toBeTruthy()
    })
    expect(screen.getByTestId('subscription-usage').getAttribute('title')).toContain('Codex updated 9m ago')
    fireEvent.click(screen.getByTestId('subscription-usage'))
    const menu = await screen.findByTestId('subscription-usage-menu')
    expect(menu.textContent).not.toContain('Nothing is signed in')
    expect(menu.textContent).toContain('Codex · plus')
    expect(menu.textContent).toContain('Sign-in expired. Sign in again to see its limits.')
    expect(screen.getByTestId('usage-row-age-codex').textContent).toBe('updated 9m ago')
    fireEvent.click(screen.getByTestId('usage-sign-in-again-codex'))
    await waitFor(() => expect(h.openSettings).toHaveBeenCalledWith('llm-tokens'))
    expect(screen.queryByTestId('subscription-usage-menu')).toBeNull()
    expect(menu.textContent).not.toContain('Claude')
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

  it('names the window’s remote server as the owner of the numbers (Z16)', async () => {
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
    expect(screen.getByTestId('subscription-usage-whose').textContent).toBe("These numbers are Remote box's logins, not this computer's.")
    expect(menu.textContent).not.toContain('laptop')
  })

  it('shows a calm Off (air-gap) state, not a number or an error, when the daemon is air-gapped', async () => {
    const checkedAt = new Date().toISOString()
    const airgapDoc = {
      airgap: true,
      reason: 'Off in air-gap mode',
      harnesses: ['claude', 'codex', 'grok'].map((harness) => ({
        harness,
        plan: '',
        windows: [],
        checkedAt,
        status: 'Off in air-gap mode',
      })),
    }
    h.daemonCliGet.mockResolvedValue(airgapDoc)
    h.daemonCliPost.mockResolvedValue(airgapDoc)
    render(<UsageButton />)
    await waitFor(() => {
      expect(screen.getByTestId('usage-airgap').textContent).toBe('Off (air-gap)')
    })
    const chip = screen.getByTestId('subscription-usage')
    expect(chip.textContent).not.toContain('%')
    expect(chip.textContent).not.toContain('Usage')
    fireEvent.click(chip)
    const menu = await screen.findByTestId('subscription-usage-menu')
    expect(screen.getByTestId('usage-airgap-note').textContent).toBe(
      'Off in air-gap mode. K2 does not check subscription usage while air-gap is on.',
    )
    expect(menu.textContent).not.toContain('Nothing is signed in')
    expect(menu.textContent).not.toContain('Claude')
    expect(menu.querySelector('[role="progressbar"]')).toBeNull()
  })

  it('the menu is a dialog with T3-style bars: thresholds, a pace mark, and freshness (P8, Q4)', async () => {
    const now = Date.now()
    const week = 7 * 24 * 3600
    serve({
      harnesses: [
        {
          harness: 'claude',
          plan: 'Max 20x',
          checkedAt: new Date(now - 4 * 60_000).toISOString(),
          status: '',
          state: 'ok',
          source: 'live_statusline',
          windows: [
            { id: 'five_hour', label: 'Session', used: 0.2, usedPercent: 20, resetsAt: new Date(now + 3 * 3_600_000).toISOString() },
            {
              id: 'seven_day',
              label: 'Weekly',
              used: 0.8,
              usedPercent: 80,
              resetsAt: new Date(now + 3 * 86_400_000).toISOString(),
              windowSecs: week,
              windowStart: new Date(now + 3 * 86_400_000 - week * 1000).toISOString(),
            },
            { id: 'model:fable:weekly', label: 'Fable Weekly', used: 0.95, usedPercent: 95, resetsAt: new Date(now + 3 * 86_400_000).toISOString() },
          ],
        },
      ],
    })
    render(<UsageButton />)
    await waitFor(() => expect(screen.getByTestId('usage-chip-claude').textContent).toContain('80%'))
    expect(screen.queryByTestId('usage-warning')).toBeNull()
    const chip = screen.getByTestId('subscription-usage')
    expect(chip.getAttribute('aria-haspopup')).toBe('dialog')
    fireEvent.click(chip)
    const menu = await screen.findByTestId('subscription-usage-menu')
    expect(menu.getAttribute('role')).toBe('dialog')
    expect(screen.getByTestId('usage-row-age-claude').textContent).toBe('updated 4m ago · live from a running session')
    const tones = [...menu.querySelectorAll('[data-limit-tone]')].map((el) => el.getAttribute('data-limit-tone'))
    expect(tones).toEqual(['ok', 'warn', 'over'])
    // Only the window with a length has a pace mark; 80% used vs 57% gone reads ahead.
    expect(menu.querySelectorAll('[data-testid="limit-pace-mark"]')).toHaveLength(1)
    expect(menu.textContent).toContain('ahead of pace')
    // The track is visible (not the menu's own background).
    const bar = menu.querySelectorAll('[role="progressbar"]')[1]
    expect(bar.className).toContain('bg-[var(--color-border)]')
    expect(bar.getAttribute('aria-valuetext')).toBe('80% used, ahead of pace')
  })

  it('shows a reset window as re-checking at 0, never the old percent (B18)', async () => {
    serve({
      harnesses: [
        {
          harness: 'claude',
          plan: 'Max',
          checkedAt: new Date().toISOString(),
          status: '',
          windows: [{ id: 'seven_day', label: 'Weekly', used: 0.97, usedPercent: 97, resetsAt: new Date(Date.now() - 60_000).toISOString(), resetPassed: true }],
        },
      ],
    })
    render(<UsageButton />)
    await waitFor(() => expect(screen.getByTestId('usage-chip-claude').textContent).toContain('0%'))
    expect(screen.getByTestId('subscription-usage').textContent).not.toContain('97')
    fireEvent.click(screen.getByTestId('subscription-usage'))
    const menu = await screen.findByTestId('subscription-usage-menu')
    expect(menu.textContent).toContain('Weekly · reset — re-checking')
    expect(menu.querySelector('[role="progressbar"]')?.getAttribute('aria-valuenow')).toBe('0')
  })

  it('a failed check keeps the last numbers but marks the chip and says why (B21)', async () => {
    serve(claudeDoc(new Date(Date.now() - 2 * 3_600_000).toISOString(), { error: 'usage endpoint status 429' }))
    render(<UsageButton />)
    await waitFor(() => expect(screen.getByTestId('usage-warning')).toBeTruthy())
    expect(screen.getByTestId('usage-chip-claude').textContent).toContain('31%')
    expect(screen.getByTestId('subscription-usage').getAttribute('title')).toContain(
      'Claude updated 2h ago · Last check failed (usage endpoint status 429)',
    )
    fireEvent.click(screen.getByTestId('subscription-usage'))
    const menu = await screen.findByTestId('subscription-usage-menu')
    expect(menu.textContent).toContain('Last check failed (usage endpoint status 429)')
    expect(screen.getByTestId('usage-row-age-claude').textContent).toBe('updated 2h ago')
  })

  it('a failed read of the cache marks the chip and shows the error in the menu (B21)', async () => {
    h.daemonCliGet.mockResolvedValue(claudeDoc(new Date().toISOString()))
    render(<UsageButton />)
    await waitFor(() => expect(screen.getByTestId('usage-chip-claude').textContent).toContain('31%'))
    expect(screen.queryByTestId('usage-warning')).toBeNull()
    h.daemonCliPost.mockRejectedValue(new Error('daemon unreachable'))
    fireEvent.click(screen.getByTestId('subscription-usage'))
    await screen.findByTestId('subscription-usage-menu')
    fireEvent.click(screen.getByTestId('subscription-usage-refresh'))
    await waitFor(() => expect(screen.getByTestId('usage-warning')).toBeTruthy())
    expect(screen.getByTestId('subscription-usage-read-error').textContent).toBe("Couldn't read usage: daemon unreachable")
    // The last good numbers stay on the chip.
    expect(screen.getByTestId('usage-chip-claude').textContent).toContain('31%')
  })
})
