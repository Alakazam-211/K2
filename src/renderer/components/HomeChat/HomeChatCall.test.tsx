// @vitest-environment jsdom
// DM call pieces (prd-home-people-channels-v1 §11a.3, §11a.4): each draws what
// it's given and calls back; no logic of its own.

import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { dmCallButtonState, type DmCallView } from './HomeChatCallControls'
import { HomeChatCallButton, HomeChatCallingBar, HomeChatCallRecord, HomeChatCallStrip, HomeChatRingCard, ringLayerTop } from './HomeChatCall'

afterEach(cleanup)

describe('DM call pieces', () => {
  it('the ring card: Answer and Decline, silent says so', () => {
    const accept = vi.fn()
    const decline = vi.fn()
    render(<HomeChatRingCard callerName="Alex Doe" username="alex" picture={null} silent serverLabel="acme" onAccept={accept} onDecline={decline} />)
    expect(screen.getByTestId('home-chat-ring-card').dataset.silent).toBe('true')
    expect(screen.getByTestId('home-chat-ring-name').textContent).toBe('Alex Doe')
    expect(screen.getByRole('alertdialog').textContent).toContain('is calling you (silent) · acme')
    fireEvent.click(screen.getByTestId('home-chat-ring-accept'))
    fireEvent.click(screen.getByTestId('home-chat-ring-decline'))
    expect(accept).toHaveBeenCalledTimes(1)
    expect(decline).toHaveBeenCalledTimes(1)
  })

  it('the ring card: Answer is the filled primary, Decline a quiet danger button, both real buttons', () => {
    render(<HomeChatRingCard callerName="Alexandra Featherstonehaugh" username="alex" picture={null} silent={false} serverLabel={null} onAccept={() => {}} onDecline={() => {}} />)
    const accept = screen.getByTestId('home-chat-ring-accept')
    const decline = screen.getByTestId('home-chat-ring-decline')
    expect(accept.textContent).toBe('Answer')
    expect(decline.textContent).toBe('Decline')
    // The shared Button: cut in Terminal, the field shape in Glass.
    for (const b of [accept, decline]) {
      expect(b.getAttribute('data-k2-chamfer')).toBe('control')
      expect(b.className).toContain('min-h-8')
    }
    expect(accept.className).toContain('bg-[var(--color-accent-fill)]')
    expect(decline.className).toContain('text-[var(--color-status-error-text)]')
    expect(decline.className).not.toContain('bg-[var(--color-danger)]')
  })

  it('the ring card: the person avatar, the whole name and "is calling you"', () => {
    render(<HomeChatRingCard callerName="Alexandra Featherstonehaugh" username="alex" picture={null} silent={false} serverLabel={null} onAccept={() => {}} onDecline={() => {}} />)
    const card = screen.getByTestId('home-chat-ring-card')
    expect(card.querySelector('[data-home-chat-avatar]')).toBeTruthy()
    const name = screen.getByTestId('home-chat-ring-name')
    expect(name.className).not.toMatch(/\btruncate\b/)
    expect(name.className).toContain('line-clamp-2')
    expect(card.textContent).toContain('is calling you')
    expect(card.textContent).not.toContain('Calling…')
  })

  it('the ring card: Frost floating card in Glass, a raised cut tile in Terminal', () => {
    render(<HomeChatRingCard callerName="Alex" username="alex" picture={null} silent={false} serverLabel={null} onAccept={() => {}} onDecline={() => {}} />)
    const card = screen.getByTestId('home-chat-ring-card')
    expect(card.getAttribute('data-material')).toBe('frost')
    expect(card.getAttribute('data-float')).toBe('popover')
    expect(card.getAttribute('data-k2-chamfer')).toBe('surface')
    expect(card.className).toContain('bg-[var(--color-bg-elevated)]')
    expect(card.parentElement?.hasAttribute('data-k2-chamfer-float')).toBe(true)
  })

  it('the ring layer sits under the open chat header, never over its controls', () => {
    expect(ringLayerTop(null)).toBe(56)
    expect(ringLayerTop(40)).toBe(56)
    expect(ringLayerTop(92)).toBe(100)
  })

  it('the caller’s bar: Cancel while it rings, Call again after a decline or no answer', () => {
    const cancel = vi.fn()
    const again = vi.fn()
    const { rerender } = render(<HomeChatCallingBar name="Alex" state="ringing" onCancel={cancel} onCallAgain={again} />)
    expect(screen.getByTestId('home-chat-calling').textContent).toContain('Calling Alex…')
    fireEvent.click(screen.getByTestId('home-chat-call-cancel'))
    expect(cancel).toHaveBeenCalledTimes(1)
    rerender(<HomeChatCallingBar name="Alex" state="declined" onCancel={cancel} onCallAgain={again} />)
    expect(screen.getByTestId('home-chat-calling').textContent).toContain('Alex declined')
    expect(screen.queryByTestId('home-chat-call-cancel')).toBeNull()
    fireEvent.click(screen.getByTestId('home-chat-call-again'))
    rerender(<HomeChatCallingBar name="Alex" state="missed" onCancel={cancel} onCallAgain={again} />)
    expect(screen.getByTestId('home-chat-calling').textContent).toContain("Alex didn't answer")
    fireEvent.click(screen.getByTestId('home-chat-call-again'))
    expect(again).toHaveBeenCalledTimes(2)
  })

  it('the in-call strip: time, mute and leave; or on another device', () => {
    const mute = vi.fn()
    const leave = vi.fn()
    const { rerender } = render(<HomeChatCallStrip seconds={724} muted={false} elsewhere={null} onMute={mute} onLeave={leave} />)
    expect(screen.getByTestId('home-chat-call-time').textContent).toBe('On a call · 12:04')
    fireEvent.click(screen.getByTestId('home-chat-call-mute'))
    fireEvent.click(screen.getByTestId('home-chat-call-leave'))
    expect(mute).toHaveBeenCalledTimes(1)
    expect(leave).toHaveBeenCalledTimes(1)
    rerender(<HomeChatCallStrip seconds={null} muted={false} elsewhere="another device" onMute={mute} onLeave={leave} />)
    expect(screen.getByTestId('home-chat-call-strip').textContent).toContain('On a call on another device')
    expect(screen.queryByTestId('home-chat-call-leave')).toBeNull()
  })

  it('the record line: its words, and Call back only when offered', () => {
    const back = vi.fn()
    const { rerender } = render(<HomeChatCallRecord line={{ kind: 'missed', text: 'Missed call', action: 'Call back' }} timeLabel="2m" onCallBack={back} />)
    expect(screen.getByTestId('home-chat-call-record').dataset.kind).toBe('missed')
    expect(screen.getByTestId('home-chat-call-record-text').textContent).toBe('Missed call')
    expect(screen.getByTestId('home-chat-call-back').textContent).toBe('Call back')
    fireEvent.click(screen.getByTestId('home-chat-call-back'))
    expect(back).toHaveBeenCalledTimes(1)
    rerender(<HomeChatCallRecord line={{ kind: 'answered', text: 'Call · 0:05', action: 'Call again' }} timeLabel="now" onCallBack={back} />)
    expect(screen.getByTestId('home-chat-call-back').textContent).toBe('Call again')
    rerender(<HomeChatCallRecord line={{ kind: 'live', text: 'Call in progress', action: null }} timeLabel="now" onCallBack={back} />)
    expect(screen.queryByTestId('home-chat-call-back')).toBeNull()
    rerender(<HomeChatCallRecord line={{ kind: 'cancelled', text: 'Cancelled', action: 'Call again' }} timeLabel="now" />)
    expect(screen.queryByTestId('home-chat-call-back')).toBeNull()
  })

  it('the call button says why it can’t call', () => {
    const call = vi.fn()
    const { rerender } = render(<HomeChatCallButton disabledReason="calls need a Linux server" onCall={call} />)
    const b = screen.getByTestId('home-chat-call-button') as HTMLButtonElement
    expect(b.disabled).toBe(true)
    expect(b.title).toBe('calls need a Linux server')
    rerender(<HomeChatCallButton disabledReason={null} onCall={call} />)
    fireEvent.click(screen.getByTestId('home-chat-call-button'))
    expect(call).toHaveBeenCalledTimes(1)
  })

  it('the call button is not offered again while you call or are in the call', () => {
    const call = vi.fn()
    const { rerender } = render(<HomeChatCallButton disabledReason={null} state="calling" onCall={call} />)
    let b = screen.getByTestId('home-chat-call-button') as HTMLButtonElement
    expect(b.textContent).toContain('Calling…')
    expect(b.getAttribute('aria-pressed')).toBe('true')
    expect(b.disabled).toBe(true)
    fireEvent.click(b)
    rerender(<HomeChatCallButton disabledReason={null} state="in-call" onCall={call} />)
    b = screen.getByTestId('home-chat-call-button') as HTMLButtonElement
    expect(b.textContent).toContain('In call')
    expect(b.getAttribute('aria-pressed')).toBe('true')
    expect(b.disabled).toBe(true)
    expect(call).not.toHaveBeenCalled()
    rerender(<HomeChatCallButton disabledReason={null} state="idle" onCall={call} />)
    b = screen.getByTestId('home-chat-call-button') as HTMLButtonElement
    expect(b.textContent).toContain('Call')
    expect(b.hasAttribute('aria-pressed')).toBe(false)
  })

  it("the DM's call decides the button: calling, in the call (here or on another device), else idle", () => {
    const view = (over: Partial<DmCallView>): DmCallView => ({
      calling: null,
      strip: null,
      call: () => {},
      cancel: () => {},
      callAgain: () => {},
      leave: () => {},
      toggleMute: () => {},
      ...over,
    })
    expect(dmCallButtonState(view({}))).toBe('idle')
    expect(dmCallButtonState(view({ calling: 'joining' }))).toBe('calling')
    expect(dmCallButtonState(view({ calling: 'ringing' }))).toBe('calling')
    expect(dmCallButtonState(view({ calling: 'busy' }))).toBe('calling')
    // After a decline or no answer the bar offers Call again; the header's Call is back too.
    expect(dmCallButtonState(view({ calling: 'declined' }))).toBe('idle')
    expect(dmCallButtonState(view({ calling: 'missed' }))).toBe('idle')
    expect(dmCallButtonState(view({ strip: { seconds: 3, muted: false, elsewhere: null } }))).toBe('in-call')
    expect(dmCallButtonState(view({ strip: { seconds: null, muted: false, elsewhere: 'another device' } }))).toBe('in-call')
  })
})
