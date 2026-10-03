// @vitest-environment jsdom

// 0.43.0 crash: a keep-alive socket desynced by GET /cli/power/status
// handed `usage/subscriptions` another route's body. The store put it in
// state as-is and `visibleHarnesses` threw `e.harnesses.filter` in render,
// taking the window to the error boundary. Every body here must render the
// button as "Usage" and open the menu without throwing.

import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import {
  resetSubscriptionUsageForTests,
  useSubscriptionUsageStore,
} from '@/stores/subscription-usage'

const h = vi.hoisted(() => ({
  daemonCliGet: vi.fn(),
  daemonCliPost: vi.fn(),
}))

vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly((...a: unknown[]) => h.daemonCliGet(...a)),
    daemonCliPost: primaryOnly((...a: unknown[]) => h.daemonCliPost(...a)),
  }
})

vi.mock('@/stores/connect-host', () => ({
  useConnectHostStore: (sel: (s: { activeHost: 'local' }) => unknown) => sel({ activeHost: 'local' }),
}))

import UsageButton from './UsageButton'

const POWER_STATUS_BODY = {
  keepAwake: { mode: 'off', state: 'off', label: 'Keep awake: off', detail: '' },
}

const BAD_BODIES: Array<[string, unknown]> = [
  ['{}', {}],
  ["{error:'role_required'}", { error: 'role_required', required: 'member', role: 'viewer' }],
  ['{harnesses:{}}', { harnesses: {} }],
  ['null', null],
  ['a power/status body', POWER_STATUS_BODY],
  ['a list body', [{ id: 'x' }]],
  ['a text body', 'not json'],
  ['rows with object windows', { harnesses: [{ harness: 'claude', windows: {}, status: '', checkedAt: '' }] }],
]

beforeEach(() => {
  cleanup()
  h.daemonCliGet.mockReset()
  h.daemonCliPost.mockReset()
  resetSubscriptionUsageForTests()
})

describe('UsageButton with a body that is not a SubscriptionDoc', () => {
  for (const [name, body] of BAD_BODIES) {
    it(`renders Usage and opens the menu for ${name}`, async () => {
      h.daemonCliGet.mockResolvedValue(body)
      h.daemonCliPost.mockResolvedValue(body)
      render(<UsageButton />)
      await waitFor(() => {
        expect(h.daemonCliGet).toHaveBeenCalledWith('usage/subscriptions')
        expect(useSubscriptionUsageStore.getState().doc).not.toBeNull()
      })
      const doc = useSubscriptionUsageStore.getState().doc
      expect(doc).not.toBeNull()
      expect(Array.isArray(doc?.harnesses)).toBe(true)
      for (const row of doc?.harnesses ?? []) {
        expect(Array.isArray(row.windows)).toBe(true)
      }
      expect(screen.getByTestId('subscription-usage').textContent).toBe('Usage')

      fireEvent.click(screen.getByTestId('subscription-usage'))
      await screen.findByTestId('subscription-usage-menu')
      await waitFor(() => {
        expect(h.daemonCliPost).toHaveBeenCalledWith('usage/subscriptions/refresh', {})
      })
      await waitFor(() => {
        expect(useSubscriptionUsageStore.getState().error).toBeNull()
      })
      expect(Array.isArray(useSubscriptionUsageStore.getState().doc?.harnesses)).toBe(true)
      expect(screen.getByTestId('subscription-usage').textContent).toBe('Usage')
    })
  }
})
