// @vitest-environment jsdom
// The Garden's "Review N pending" pill (B3, ZenSourcesPending) opens B2's
// batch review: the dialog registers itself at module load.

import { describe, it, expect, vi } from 'vitest'
import { render, screen, fireEvent, waitFor } from '@testing-library/react'

const daemonCliGet = vi.hoisted(() => vi.fn())
vi.mock('@/lib/daemon-cli', () => ({ daemonCliGet, daemonCliPost: vi.fn() }))
vi.mock('@/kessel/server-scope', () => {
  const s = { id: 'primary', isPrimary: true, hostKey: 'local', label: 'This computer', isRemote: false, connectHost: () => null }
  return { primaryScope: () => s, scopeForHost: () => s }
})
vi.mock('@/lib/zen/zen-sources-live', () => ({
  zenSourcesReader: () => ({ read: async () => ({ status: 'proposed' }) }),
}))

import { ZenSourcesPending } from '@/components/Zen/widgets/ZenSourcesPending'
import { zenSourcesBatchOpener } from '@/lib/zen/zen-sources-review'
import { openSourcesBatchReview } from './DataSourcesBatchDialog'
import type { ZenWidgetDecl } from '@/lib/zen/zen-page'

describe('Garden pill → batch review', () => {
  it('registers at load, and the pill opens the dialog for its pending sources', async () => {
    expect(zenSourcesBatchOpener()).toBe(openSourcesBatchReview)
    daemonCliGet.mockResolvedValue({
      sources: [
        {
          id: 'src-fleet',
          name: 'fleet',
          label: 'Fleet boxes',
          madeBy: 'agent:p1',
          origins: ['https://a.k2.dev:443'],
          everySecs: 60,
          targets: 2,
          secret: null,
          urlSecret: null,
          lan: false,
          specHash: 'h'.repeat(64),
          approval: 'one_click',
        },
      ],
      triggers: [],
    })
    const widgets = [{ id: 'w1', kind: 'custom', custom: { sourceAddresses: ['fleet'] } }] as unknown as ZenWidgetDecl[]
    render(<ZenSourcesPending gardenName="Fleet" widgets={widgets} />)
    const pill = await screen.findByText('Review 1 pending')
    fireEvent.click(pill)
    await waitFor(() => expect(document.querySelector('[data-testid="sources-batch"]')).not.toBeNull())
    expect(document.body.textContent).toContain('Review pending for Garden “Fleet”')
    await waitFor(() => expect(document.body.textContent).toContain('Fleet boxes'))
    expect(daemonCliGet).toHaveBeenCalledWith(expect.anything(), 'sources/pending', { names: 'fleet' })
  })
})
