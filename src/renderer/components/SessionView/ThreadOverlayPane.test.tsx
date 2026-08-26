// @vitest-environment jsdom
import { describe, expect, it, afterEach } from 'vitest'
import { render, screen, fireEvent, cleanup } from '@testing-library/react'
import { ThreadItemRow } from './ThreadOverlayPane'
import type { OverlayThreadItem } from './overlayThread'

describe('Thread overlay choice chips + secret field', () => {
  afterEach(() => cleanup())

  it('renders pending choice chips; first option is primary; tap calls onAnswer', () => {
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
    const chips = screen.getAllByTestId('thread-choice-chip')
    expect(chips).toHaveLength(2)
    expect(chips[0].textContent).toBe('Go')
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
    expect(go.className).toContain('border-[var(--color-accent)]')
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
