// @vitest-environment jsdom
import { describe, expect, it, afterEach, vi } from 'vitest'
import { render, screen, fireEvent, cleanup } from '@testing-library/react'
import {
  choiceLetter,
  ThreadItemRow,
  ThreadOverlayPane,
} from './ThreadOverlayPane'
import type { OverlayThreadItem } from './overlayThread'

const threadHook = vi.hoisted(() => ({
  items: [] as OverlayThreadItem[],
  conversationId: 'c',
  error: null as string | null,
  posting: false,
  post: async () => {},
  answer: async () => {},
  voidCard: async () => {},
  hasMore: false,
  loadingOlder: false,
  loadOlder: async () => {},
}))

vi.mock('./useOverlayThread', () => ({
  useOverlayThread: () => threadHook,
}))

vi.mock('@/stores/settings', () => ({
  useSettingsStore: (sel: (s: { editor: { fontSize: number } }) => unknown) =>
    sel({ editor: { fontSize: 12 } }),
}))

describe('Thread overlay pane', () => {
  afterEach(() => {
    cleanup()
    threadHook.items = []
    threadHook.error = null
    threadHook.hasMore = false
    threadHook.loadingOlder = false
    threadHook.loadOlder = async () => {}
  })

  it('has no compose box — Message-the-agent stays on the terminal bar', () => {
    render(<ThreadOverlayPane addr="sales" conversationId="c" />)
    expect(screen.getByTestId('thread-overlay-pane')).not.toBeNull()
    expect(screen.queryByTestId('thread-compose')).toBeNull()
  })

  it('shows Load older when hasMore and click calls loadOlder', () => {
    const loadOlder = vi.fn(async () => {})
    threadHook.hasMore = true
    threadHook.loadOlder = loadOlder
    render(<ThreadOverlayPane addr="sales" conversationId="c" />)
    fireEvent.click(screen.getByTestId('overlay-load-older'))
    expect(loadOlder).toHaveBeenCalledTimes(1)
  })

  it('hides Load older when hasMore is false', () => {
    render(<ThreadOverlayPane addr="sales" conversationId="c" />)
    expect(screen.queryByTestId('overlay-load-older')).toBeNull()
  })
})

describe('Thread overlay choice chips + secret field', () => {
  afterEach(() => cleanup())

  it('letters options A, B, … then AA', () => {
    expect(choiceLetter(0)).toBe('A')
    expect(choiceLetter(1)).toBe('B')
    expect(choiceLetter(25)).toBe('Z')
    expect(choiceLetter(26)).toBe('AA')
  })

  it('renders markdown in a project-chat style message', () => {
    const item: OverlayThreadItem = {
      collection: 'thread',
      seq: 1,
      id: 't1',
      doc: {
        id: 't1',
        kind: 'text',
        from: 'owner',
        via: 'compose',
        created_at: Math.floor(Date.now() / 1000),
        body: '**Hello** and `code`',
      },
    }
    render(<ThreadItemRow item={item} />)
    expect(screen.getByText('You')).not.toBeNull()
    expect(screen.getByText('Hello').tagName).toBe('STRONG')
    expect(screen.getByText('code').tagName).toBe('CODE')
  })

  it('renders a vertical lettered choice card; first option is primary; tap calls onAnswer', () => {
    const picks: string[] = []
    const item: OverlayThreadItem = {
      collection: 'thread',
      seq: 1,
      id: 'c1',
      doc: {
        id: 'c1',
        kind: 'choice',
        from: 'k2',
        body: 'Ship it?',
        choice: {
          prompt: 'Ship it?',
          options: [{ label: 'Go' }, { label: 'Stop' }],
          allow_custom: false,
          status: 'pending',
        },
      },
    }
    render(<ThreadItemRow item={item} onAnswer={(p) => picks.push(p.answer || '')} />)
    const card = screen.getByTestId('thread-choice-card')
    expect(card.className).toContain('flex-col')
    expect(card.className).toContain('w-full')
    expect(card.className).not.toContain('max-w-sm')
    expect(card.className).toMatch(/\bpx-2\b/)
    const chips = screen.getAllByTestId('thread-choice-chip')
    expect(chips).toHaveLength(2)
    expect(chips[0].getAttribute('data-letter')).toBe('A')
    expect(chips[1].getAttribute('data-letter')).toBe('B')
    expect(chips[0].textContent).toContain('A')
    expect(chips[0].textContent).toContain('Go')
    expect(chips[0].getAttribute('data-primary')).toBe('true')
    expect(chips[1].getAttribute('data-primary')).toBe('false')
    fireEvent.click(chips[0])
    expect(picks).toEqual(['Go'])
  })

  it('answered choice stays visible with the selected option highlighted', () => {
    const item: OverlayThreadItem = {
      collection: 'thread',
      seq: 1,
      id: 'c1',
      doc: {
        id: 'c1',
        kind: 'choice',
        from: 'k2',
        choice: {
          prompt: 'Ship it?',
          options: [{ label: 'Go' }, { label: 'Stop' }],
          allow_custom: false,
          status: 'answered',
          answer: 'Go',
        },
      },
    }
    render(<ThreadItemRow item={item} />)
    expect(screen.getByTestId('thread-choice-card')).not.toBeNull()
    const go = screen.getAllByTestId('thread-choice-chip')[0]
    expect(go.getAttribute('disabled')).not.toBeNull()
    expect(go.getAttribute('data-letter')).toBe('A')
    expect(go.className).toContain('border-[var(--color-accent)]')
    expect(go.className).toContain('bg-[var(--color-accent)]/15')
  })

  it('renders a secret field and submit/dismiss; never shows the value as body text', () => {
    const submitted: string[] = []
    let dismissed = 0
    const item: OverlayThreadItem = {
      collection: 'thread',
      seq: 2,
      id: 's1',
      doc: {
        id: 's1',
        kind: 'secret',
        from: 'k2',
        secret: { name: 'API_TOKEN', status: 'pending', prompt: 'Paste the Grok token' },
      },
    }
    render(
      <ThreadItemRow
        item={item}
        onAnswer={(p) => submitted.push(p.secret || '')}
        onVoid={() => {
          dismissed += 1
        }}
      />,
    )
    const field = screen.getByTestId('thread-secret-field') as HTMLInputElement
    expect(field.type).toBe('password')
    fireEvent.change(field, { target: { value: 's3cr3t-bytes' } })
    fireEvent.click(screen.getByTestId('thread-secret-submit'))
    expect(submitted).toEqual(['s3cr3t-bytes'])
    expect(screen.queryByText('s3cr3t-bytes')).toBeNull()
    fireEvent.click(screen.getByTestId('thread-secret-dismiss'))
    expect(dismissed).toBe(1)
  })
})
