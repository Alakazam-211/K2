// @vitest-environment jsdom
// Each shell piece on its own: presentational, props in and callbacks out.
// States, accessible names, focus, and the callbacks.

import { cleanup, fireEvent, render, screen, within } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import type { LivePerson } from '@/lib/live-view/shared/people'
import type { LiveChange } from '@/lib/live-view/shared/source'
import { AvatarStack } from './AvatarStack'
import { ChangesList } from './ChangesList'
import { EdgeStack } from './EdgeStack'
import { FollowControl } from './FollowControl'
import { LiveAvatar } from './LiveAvatar'
import { LiveHeader } from './LiveHeader'
import { NoticeBar } from './NoticeBar'
import { PanelSection } from './PanelSection'
import { SayBar } from './SayBar'
import { StateChip } from './StateChip'
import { WatchingList } from './WatchingList'

afterEach(() => cleanup())

const P = (id: string, name: string, kind: 'person' | 'agent' = 'person', extra: Partial<LivePerson> = {}): LivePerson => ({ id, name, kind, colour: '#7c3aed', away: false, ...extra })
const you = P('you', 'You', 'person', { self: true, doing: 'following Atlas' })
const atlas = P('atlas', 'Atlas', 'agent', { doing: 'holding 2 clips' })
const brook = P('brook', 'Brook', 'agent', { doing: 'in the file, not editing' })
const sam = P('sam', 'Sam', 'person', { away: true, doing: 'away' })

describe('LiveAvatar', () => {
  it('is the shared avatar shape: cut in Terminal, round in Glass, its colour from the data', () => {
    render(<LiveAvatar person={atlas} size={22} />)
    const av = screen.getByTestId('lv-avatar')
    expect(av.hasAttribute('data-k2-avatar')).toBe(true)
    expect(av.getAttribute('data-k2-chamfer')).toBe('surface')
    expect(av.style.getPropertyValue('--k2-ch-cut')).toBe('6px')
    expect(av.style.backgroundColor).not.toBe('')
    expect(av.textContent).toBe('A')
    expect(av.getAttribute('aria-hidden')).toBe('true')
  })

  it('marks away and greyed avatars', () => {
    render(<LiveAvatar person={sam} grey />)
    const av = screen.getByTestId('lv-avatar')
    expect(av.hasAttribute('data-away')).toBe(true)
    expect(av.hasAttribute('data-grey')).toBe(true)
  })
})

describe('StateChip', () => {
  it('is one dot and one word, as written', () => {
    render(<StateChip state="reconnecting" />)
    const chip = screen.getByTestId('lv-state')
    expect(chip.textContent).toBe('Reconnecting')
    expect(chip.getAttribute('data-tone')).toBe('warn')
    expect(chip.getAttribute('data-case')).toBe('as-written')
    expect(chip.querySelector('[data-lv-dot]')).not.toBeNull()
    expect(chip.getAttribute('data-state')).toBe('reconnecting')
  })
})

describe('LiveHeader', () => {
  it('names the file once, says the state and one sentence, and toggles the panel', () => {
    const onPanel = vi.fn()
    render(
      <LiveHeader fileName="rough-cut" kindLabel="Timeline" state="live" sentence={{ who: atlas, text: 'trimmed the start of Interview B by 1s' }} panelOpen onTogglePanel={onPanel} right={<span>slot</span>} />,
    )
    const head = screen.getByTestId('lv-shell-header')
    expect(within(head).getAllByText('rough-cut')).toHaveLength(1)
    expect(within(head).getByTestId('lv-state').textContent).toBe('Live')
    expect(within(head).getByTestId('lv-sentence').textContent).toBe('Atlas trimmed the start of Interview B by 1s')
    expect(within(head).getByText('slot')).toBeTruthy()
    const toggle = within(head).getByRole('button', { name: 'Hide the side panel' })
    expect(toggle.getAttribute('aria-pressed')).toBe('true')
    fireEvent.click(toggle)
    expect(onPanel).toHaveBeenCalledTimes(1)
  })

  it('shows no sentence when there is none', () => {
    render(<LiveHeader fileName="living-room" state="loading" sentence={null} panelOpen={false} onTogglePanel={() => {}} />)
    expect(screen.queryByTestId('lv-sentence')).toBeNull()
    expect(screen.getByRole('button', { name: 'Show the side panel' }).getAttribute('aria-pressed')).toBe('false')
  })
})

describe('AvatarStack', () => {
  it('a button per person that follows them; away faded; overflow names in its tooltip', () => {
    const onFollow = vi.fn()
    const many = [atlas, brook, sam, P('kai', 'Kai'), P('lee', 'Lee'), P('mo', 'Mo')]
    render(<AvatarStack people={many} max={4} onFollow={onFollow} />)
    const group = screen.getByRole('group', { name: 'Who’s here: click someone to follow them' })
    const buttons = within(group).getAllByRole('button')
    expect(buttons.map((b) => b.getAttribute('aria-label'))).toEqual(['Follow Atlas', 'Follow Brook', 'Follow Sam'])
    for (const b of buttons) expect(b.hasAttribute('data-k2-focus')).toBe(true)
    expect(buttons[2].getAttribute('title')).toBe('Sam, away: follow them')
    expect(buttons[0].getAttribute('title')).toBe('Atlas (agent): follow it')
    const more = within(group).getByTestId('lv-stack-more')
    expect(more.textContent).toBe('+3')
    expect(more.getAttribute('title')).toBe('Also here: Kai, Lee, Mo')
    fireEvent.click(buttons[1])
    expect(onFollow).toHaveBeenCalledWith('brook')
  })

  it('marks whom you follow', () => {
    render(<AvatarStack people={[atlas, brook]} followingId="brook" onFollow={() => {}} />)
    expect(screen.getByRole('button', { name: 'Follow Brook' }).getAttribute('aria-pressed')).toBe('true')
    expect(screen.getByRole('button', { name: 'Follow Atlas' }).getAttribute('aria-pressed')).toBe('false')
  })
})

describe('FollowControl', () => {
  it('“Follow Atlas” is filled and follows', () => {
    const onFollow = vi.fn()
    render(<FollowControl view={{ control: 'follow', label: 'Follow Atlas', target: 'atlas' }} onFollow={onFollow} onStop={() => {}} onBack={() => {}} />)
    const b = screen.getByRole('button', { name: 'Follow Atlas' })
    expect(b.getAttribute('aria-pressed')).toBe('false')
    expect(b.getAttribute('data-variant')).toBe('accent')
    fireEvent.click(b)
    expect(onFollow).toHaveBeenCalledWith('atlas')
  })

  it('following is one control that names whom and stops', () => {
    const onStop = vi.fn()
    render(<FollowControl view={{ control: 'following', label: 'Following Brook', stop: 'Stop', target: 'brook', colour: '#be185d' }} onFollow={() => {}} onStop={onStop} onBack={() => {}} />)
    const b = screen.getByRole('button', { name: 'Following Brook · Stop' })
    expect(b.getAttribute('aria-pressed')).toBe('true')
    expect(b.getAttribute('title')).toBe('Stop following Brook')
    expect(screen.getAllByRole('button')).toHaveLength(1)
    fireEvent.click(b)
    expect(onStop).toHaveBeenCalledTimes(1)
  })

  it('“Back to Brook” goes back; a leaving note is a status', () => {
    const onBack = vi.fn()
    const { rerender } = render(<FollowControl view={{ control: 'back', label: 'Back to Brook', target: 'brook' }} onFollow={() => {}} onStop={() => {}} onBack={onBack} />)
    fireEvent.click(screen.getByRole('button', { name: 'Back to Brook' }))
    expect(onBack).toHaveBeenCalledTimes(1)
    rerender(<FollowControl view={{ control: 'follow', label: 'Follow Atlas', target: 'atlas', note: 'Sam left the file' }} onFollow={() => {}} onStop={() => {}} onBack={onBack} />)
    expect(screen.getByRole('status').textContent).toBe('Sam left the file')
  })

  it('back can live elsewhere (the timeline’s transport): then the header shows nothing while away', () => {
    const { container } = render(<FollowControl view={{ control: 'back', label: 'Back to Brook', target: 'brook' }} backHere={false} onFollow={() => {}} onStop={() => {}} onBack={() => {}} />)
    expect(container.textContent).toBe('')
  })

  it('is gone once the agent is done', () => {
    const { container } = render(<FollowControl view={{ control: 'none', label: '' }} onFollow={() => {}} onStop={() => {}} onBack={() => {}} />)
    expect(container.textContent).toBe('')
  })
})

describe('WatchingList', () => {
  const people = [you, atlas, brook, sam]

  it('“Watching N” counts only who’s here; away people sit under a quiet Away line', () => {
    render(<WatchingList people={people} onFollow={() => {}} />)
    expect(screen.getByTestId('lv-watching-count').textContent).toBe('3')
    const away = screen.getByTestId('lv-away')
    expect(within(away).getByText('Away')).toBeTruthy()
    expect(within(away).getByText('Sam')).toBeTruthy()
  })

  it('a row per other person follows them; your own row is not a button', () => {
    const onFollow = vi.fn()
    render(<WatchingList people={people} followingId="atlas" onFollow={onFollow} />)
    const row = screen.getByRole('button', { name: /^Atlas agent/ })
    expect(row.hasAttribute('data-k2-focus')).toBe(true)
    expect(row.getAttribute('aria-pressed')).toBe('true')
    expect(within(row).getByText('You’re following')).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: /^Brook agent/ }))
    expect(onFollow).toHaveBeenCalledWith('brook')
    expect(screen.queryByRole('button', { name: /^You/ })).toBeNull()
    expect(screen.getByText('following Atlas')).toBeTruthy()
  })

  it('while not connected: “as you last saw”, greyed, nothing to follow', () => {
    render(<WatchingList people={people} stale onFollow={() => {}} />)
    expect(screen.getByTestId('lv-watching-count').textContent).toBe('as you last saw')
    // Only the section's own fold button: no person to click.
    expect(screen.queryAllByRole('button').filter((b) => !b.hasAttribute('aria-expanded'))).toHaveLength(0)
    for (const av of screen.getAllByTestId('lv-avatar')) expect(av.hasAttribute('data-grey')).toBe(true)
  })
})

describe('ChangesList', () => {
  const now = Date.UTC(2026, 10, 15, 14, 30)
  const changes: LiveChange[] = [
    { id: 'c2', who: 'atlas', verb: 'moved', text: 'moved B-roll 03 to 00:12', at_ms: now - 2 * 60_000, target: '@item:7', undoable: true },
    { id: 'c1', who: 'atlas', verb: 'trimmed', text: 'trimmed the start of Interview B by 1s', at_ms: now - 10_000, target: '@item:2' },
    { id: 'c0', who: 'file', verb: 'saved', text: 'saved outside K2', at_ms: now - 5_000, target: null },
  ]
  const people = [atlas, P('file', 'Saved outside K2', 'person')]

  it('one row per change, newest first: who, one verb, when, Go there', () => {
    const onGo = vi.fn()
    render(<ChangesList changes={changes} people={people} now={now} onGoThere={onGo} />)
    expect(screen.getByTestId('lv-changes-count').textContent).toBe('3')
    const rows = screen.getAllByTestId('lv-change')
    expect(rows.map((r) => r.querySelector('[data-lv-change-text]')?.textContent)).toEqual([
      'Saved outside K2 saved outside K2',
      'Atlas trimmed the start of Interview B by 1s',
      'Atlas moved B-roll 03 to 00:12',
    ])
    expect(within(rows[1]).getByText('just now')).toBeTruthy()
    expect(within(rows[2]).getByText('2 min ago')).toBeTruthy()
    expect(within(rows[0]).queryByRole('button', { name: /Go there/ })).toBeNull()
    fireEvent.click(within(rows[1]).getByRole('button', { name: 'Go there: Atlas trimmed the start of Interview B by 1s' }))
    expect(onGo).toHaveBeenCalledWith(changes[1])
  })

  it('Undo only where a change can be undone and the view offers it', () => {
    const onUndo = vi.fn()
    const { rerender } = render(<ChangesList changes={changes} people={people} now={now} onGoThere={() => {}} />)
    expect(screen.queryByRole('button', { name: /^Undo/ })).toBeNull()
    rerender(<ChangesList changes={changes} people={people} now={now} onGoThere={() => {}} onUndo={onUndo} />)
    const undo = screen.getAllByRole('button', { name: /^Undo/ })
    expect(undo).toHaveLength(1)
    fireEvent.click(undo[0])
    expect(onUndo).toHaveBeenCalledWith(changes[0])
  })

  it('says when there are none yet', () => {
    render(<ChangesList changes={[]} people={people} now={now} />)
    expect(screen.getByText('No changes yet')).toBeTruthy()
  })
})

describe('PanelSection', () => {
  it('folds and unfolds by its heading', () => {
    render(
      <PanelSection id="changes" title="Changes" count={2}>
        <p>body</p>
      </PanelSection>,
    )
    const head = screen.getByRole('button', { name: /Changes/ })
    expect(head.getAttribute('aria-expanded')).toBe('true')
    expect(head.hasAttribute('data-k2-focus')).toBe(true)
    fireEvent.click(head)
    expect(head.getAttribute('aria-expanded')).toBe('false')
    expect(screen.queryByText('body')).toBeNull()
  })
})

describe('NoticeBar', () => {
  const ctx = { fileName: 'rough-cut' }

  it('kept both: words, the copy to open, History, Dismiss', () => {
    const onAction = vi.fn()
    const save = { outcome: 'kept_both' as const, rev: 3, kept: 2, copy: { name: 'c.otio', path: '/ws/c.otio', doc: 'doc-9' } }
    render(<NoticeBar notice={{ kind: 'outside_save', save }} ctx={ctx} onAction={onAction} />)
    const bar = screen.getByRole('alert')
    expect(bar.textContent).toContain('Saved outside K2: that save is now live.')
    fireEvent.click(within(bar).getByRole('button', { name: 'Open the copy' }))
    expect(onAction).toHaveBeenCalledWith('open-copy', { kind: 'outside_save', save })
    fireEvent.click(within(bar).getByRole('button', { name: 'Dismiss' }))
    expect(onAction).toHaveBeenLastCalledWith('dismiss', { kind: 'outside_save', save })
  })

  it('each outcome says its own words with its own next step', () => {
    const cases = [
      [{ outcome: 'ingested', rev: 2 }, 'status', 'Saved outside K2 · now in the live file', ['Dismiss']],
      [{ outcome: 'declined', code: 'x', message: 'Not a timeline.' }, 'alert', 'Saved outside K2, and the file can’t be live any more.', ['Open the saved file']],
      [{ outcome: 'missing' }, 'alert', 'rough-cut was moved or deleted outside K2.', ['Save it again']],
    ] as const
    for (const [save, role, title, actions] of cases) {
      const { unmount } = render(<NoticeBar notice={{ kind: 'outside_save', save: save as never }} ctx={ctx} onAction={() => {}} />)
      const bar = screen.getByRole(role)
      expect(bar.getAttribute('data-testid')).toBe('lv-notice')
      expect(within(bar).getByText(title)).toBeTruthy()
      expect(within(bar).getAllByRole('button').map((b) => b.textContent)).toEqual([...actions])
      unmount()
    }
  })

  it('a connection bar: Trying… is disabled; Retry now retries', () => {
    const onAction = vi.fn()
    const { rerender } = render(<NoticeBar notice={{ kind: 'connection', state: 'reconnecting' }} ctx={ctx} onAction={onAction} />)
    fireEvent.click(screen.getByRole('button', { name: 'Retry now' }))
    expect(onAction).toHaveBeenCalledWith('retry', { kind: 'connection', state: 'reconnecting' })
    rerender(<NoticeBar notice={{ kind: 'connection', state: 'trying' }} ctx={ctx} onAction={onAction} />)
    expect((screen.getByRole('button', { name: 'Trying…' }) as HTMLButtonElement).disabled).toBe(true)
  })

  it('renders nothing without a notice', () => {
    const { container } = render(<NoticeBar notice={null} ctx={ctx} onAction={() => {}} />)
    expect(container.textContent).toBe('')
  })
})

describe('SayBar', () => {
  const base = { toName: 'Atlas', address: { label: 'Interview B', title: 'Interview B on V1' }, offline: false, paused: false }

  it('a named field, a chip for what it points at, and Send apart', () => {
    const onDraft = vi.fn()
    const onSend = vi.fn()
    render(<SayBar {...base} state={{ draft: 'Keep the cut?', status: 'idle', tries: 0 }} onDraft={onDraft} onSend={onSend} onSendAgain={() => {}} />)
    const field = screen.getByRole('textbox', { name: 'Say something to Atlas about Interview B' })
    expect(field.getAttribute('placeholder')).toBe('Say something to Atlas…')
    fireEvent.change(field, { target: { value: 'Keep the cut on the breath?' } })
    expect(onDraft).toHaveBeenCalledWith('Keep the cut on the breath?')
    expect(screen.getByTestId('lv-say-chip').textContent).toBe('Interview B')
    const send = screen.getByRole('button', { name: 'Send' })
    expect(send.getAttribute('data-variant')).toBe('accent')
    fireEvent.click(send)
    expect(onSend).toHaveBeenCalledTimes(1)
    fireEvent.keyDown(field, { key: 'Enter' })
    expect(onSend).toHaveBeenCalledTimes(2)
  })

  it('off while offline: “Back online to send”', () => {
    render(<SayBar {...base} offline state={{ draft: '', status: 'idle', tries: 0 }} onDraft={() => {}} onSend={() => {}} onSendAgain={() => {}} />)
    const field = screen.getByRole('textbox') as HTMLInputElement
    expect(field.disabled).toBe(true)
    expect(field.placeholder).toBe('Back online to send')
    expect((screen.getByRole('button', { name: 'Send' }) as HTMLButtonElement).disabled).toBe(true)
  })

  it('a failure keeps the words and offers Send again', () => {
    const onAgain = vi.fn()
    render(<SayBar {...base} state={{ draft: 'Is it better?', status: 'failed', tries: 1 }} onDraft={() => {}} onSend={() => {}} onSendAgain={onAgain} />)
    expect(screen.getByRole('alert').textContent).toContain('Couldn’t send: the connection dropped. Your message is kept here.')
    expect((screen.getByRole('textbox') as HTMLInputElement).value).toBe('Is it better?')
    fireEvent.click(screen.getByRole('button', { name: 'Send again' }))
    expect(onAgain).toHaveBeenCalledTimes(1)
    expect((screen.getByRole('button', { name: 'Send' }) as HTMLButtonElement).disabled).toBe(true)
  })

  it('the sent line and the reply show in the row', () => {
    render(<SayBar {...base} state={{ draft: '', status: 'sent', tries: 0, last: { from: 'them', text: 'On it.' } }} onDraft={() => {}} onSend={() => {}} onSendAgain={() => {}} />)
    expect(screen.getByTestId('lv-say-last').textContent).toBe('Atlas → You: On it.')
  })
})

describe('EdgeStack', () => {
  it('names who is off screen and where, and jumps there', () => {
    const onJump = vi.fn()
    render(<EdgeStack side="right" people={[atlas, brook]} onJump={onJump} />)
    const b = screen.getByRole('button', { name: 'Atlas, Brook, off screen to the right: go there' })
    expect(b.hasAttribute('data-k2-focus')).toBe(true)
    expect(within(b).getAllByTestId('lv-avatar')).toHaveLength(2)
    fireEvent.click(b)
    expect(onJump).toHaveBeenCalledWith('right', ['atlas', 'brook'])
  })

  it('renders nothing for nobody', () => {
    const { container } = render(<EdgeStack side="left" people={[]} onJump={() => {}} />)
    expect(container.textContent).toBe('')
  })
})
