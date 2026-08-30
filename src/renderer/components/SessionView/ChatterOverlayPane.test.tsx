// @vitest-environment jsdom
import { describe, expect, it, afterEach, vi } from 'vitest'
import { render, screen, fireEvent, cleanup } from '@testing-library/react'
import type { OverlayThreadItem } from './overlayThread'

const chatterHook = vi.hoisted(() => ({
  items: [] as OverlayThreadItem[],
  conversationId: 'c',
  error: null as string | null,
  hasMore: false,
  loadingOlder: false,
  loadOlder: async () => {},
}))

vi.mock('./useOverlayChatter', () => ({
  useOverlayChatter: () => chatterHook,
}))

vi.mock('@/stores/settings', () => ({
  useSettingsStore: (sel: (s: { editor: { fontSize: number } }) => unknown) =>
    sel({ editor: { fontSize: 12 } }),
}))

import { ChatterOverlayPane } from './ChatterOverlayPane'

describe('Chatter overlay pane', () => {
  afterEach(() => {
    cleanup()
    chatterHook.items = []
    chatterHook.error = null
    chatterHook.hasMore = false
    chatterHook.loadingOlder = false
    chatterHook.loadOlder = async () => {}
  })

  it('has no compose / send / textarea', () => {
    render(<ChatterOverlayPane addr="sales" conversationId="c" />)
    expect(screen.getByTestId('chatter-overlay-pane')).not.toBeNull()
    expect(screen.queryByTestId('thread-compose')).toBeNull()
    expect(screen.queryByTestId('chatter-compose')).toBeNull()
    expect(screen.queryByRole('textbox')).toBeNull()
    expect(screen.queryByRole('button', { name: /send/i })).toBeNull()
    expect(document.querySelector('textarea')).toBeNull()
  })

  it('shows empty mailbox copy, not the Thread compose hint', () => {
    render(<ChatterOverlayPane addr="sales" conversationId="c" />)
    expect(screen.getByText('No agent-to-agent messages yet.')).not.toBeNull()
    expect(screen.queryByText(/Message the agent below/i)).toBeNull()
  })

  it('renders from → to and body via ChatMessage', () => {
    chatterHook.items = [
      {
        collection: 'chatter',
        seq: 1,
        id: 'c1',
        doc: {
          id: 'c1',
          kind: 'chatter',
          from: 'sales',
          to: 'sales/reviewer',
          body: '**ping** via mailbox',
          via: 'msg',
          created_at: Math.floor(Date.now() / 1000),
        },
      },
      {
        collection: 'chatter',
        seq: 2,
        id: 'c2',
        doc: {
          id: 'c2',
          kind: 'chatter',
          from: 'ops',
          to: 'sales',
          body: 'incoming',
          via: 'talk',
        },
      },
    ]
    render(<ChatterOverlayPane addr="sales" conversationId="c" />)
    const rows = screen.getAllByTestId('chatter-item')
    expect(rows).toHaveLength(2)
    expect(screen.getByText('sales → sales/reviewer')).not.toBeNull()
    expect(screen.getByText('ops → sales')).not.toBeNull()
    expect(screen.getByText('ping').tagName).toBe('STRONG')
    expect(screen.getByText('incoming')).not.toBeNull()
    expect(screen.queryByTestId('thread-choice-card')).toBeNull()
    expect(screen.queryByTestId('thread-secret-card')).toBeNull()
    expect(screen.queryByRole('textbox')).toBeNull()
  })

  it('shows Load older when hasMore and click calls loadOlder', () => {
    const loadOlder = vi.fn(async () => {})
    chatterHook.hasMore = true
    chatterHook.loadOlder = loadOlder
    render(<ChatterOverlayPane addr="sales" conversationId="c" />)
    fireEvent.click(screen.getByTestId('overlay-load-older'))
    expect(loadOlder).toHaveBeenCalledTimes(1)
  })
})
