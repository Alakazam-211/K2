// @vitest-environment jsdom
// DM call pieces (prd-home-people-channels-v1 §11a.3, §11a.4): each draws what
// it's given and calls back; no logic of its own.

import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { dmCallButtonState, type DmCallView } from './HomeChatCallControls'
import {
  HomeChatCallButton,
  HomeChatCallingBar,
  HomeChatCallRecord,
  HomeChatCallStrip,
  HomeChatRingCard,
  RING_DOCK_BELOW,
  ringLayerPlace,
  ringLayerTop,
} from './HomeChatCall'
import { dmCallRecording, type DmCallRecording } from '@/lib/home-chat/recordings'

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

  /** The open chat's header (CSS px). */
  const header = (left: number, width: number) => ({ top: 48, bottom: 94, left, right: left + width, width, height: 46 })

  it('a wide chat: the full card floats at the top right, under the header', () => {
    expect(ringLayerPlace(header(260, 1160))).toEqual({ mode: 'float', top: 102 })
    expect(ringLayerPlace(null)).toEqual({ mode: 'float', top: 56 })
    // A header with no layout yet (no width): the float, never a zero-wide dock.
    expect(ringLayerPlace({ top: 0, bottom: 0, left: 0, right: 0, width: 0, height: 0 })).toEqual({ mode: 'float', top: 56 })
  })

  it('a narrow chat: the card goes compact and docks over the header, across its width, so the conversation stays clear', () => {
    expect(RING_DOCK_BELOW).toBeGreaterThan(525)
    expect(ringLayerPlace(header(275, 525))).toEqual({ mode: 'dock', top: 48, left: 275, width: 525, height: 46 })
    expect(ringLayerPlace(header(275, RING_DOCK_BELOW))).toEqual({ mode: 'float', top: 102 })
  })

  it('the compact card: one line, the name, "is calling you", Decline and Answer, still obvious', () => {
    const accept = vi.fn()
    render(<HomeChatRingCard compact callerName="Ana Ruiz" username="ana" picture={null} silent={false} serverLabel={null} onAccept={accept} onDecline={() => {}} />)
    const card = screen.getByTestId('home-chat-ring-card')
    expect(card.dataset.compact).toBe('true')
    expect(card.getAttribute('role')).toBe('alertdialog')
    // One row filling the header's box, not the 18rem column.
    expect(card.className).toMatch(/\bflex-row\b/)
    expect(card.className).toMatch(/\bh-full\b/)
    expect(card.className).not.toMatch(/\bw-72\b/)
    expect(card.textContent).toContain('Ana Ruiz')
    expect(card.textContent).toContain('is calling you')
    // The same look as the full card: Frost in Glass, a cut tile in Terminal.
    expect(card.getAttribute('data-material')).toBe('frost')
    expect(card.getAttribute('data-k2-chamfer')).toBe('surface')
    for (const id of ['home-chat-ring-decline', 'home-chat-ring-accept']) expect(screen.getByTestId(id).className).toContain('min-h-8')
    fireEvent.click(screen.getByTestId('home-chat-ring-accept'))
    expect(accept).toHaveBeenCalledTimes(1)
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
      record: () => {},
      stopRecording: () => {},
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

describe('recording a DM call (prd-home-call-recording-v1 §2, HR-9)', () => {
  it('the strip shows Record only when allowed, Stop while it may, and the badge while one runs', () => {
    const rec = vi.fn()
    const stop = vi.fn()
    const strip = (recording: DmCallRecording | null) => (
      <HomeChatCallStrip seconds={5} muted={false} elsewhere={null} onMute={vi.fn()} onLeave={vi.fn()} recording={recording} onRecord={rec} onStop={stop} />
    )
    const { rerender } = render(strip(null))
    expect(screen.queryByTestId('home-chat-call-record')).toBeNull()
    expect(screen.queryByTestId('home-chat-call-recording')).toBeNull()
    rerender(strip({ badge: null, canRecord: false, canStop: false }))
    expect(screen.queryByTestId('home-chat-call-record')).toBeNull()
    rerender(strip({ badge: null, canRecord: true, canStop: false }))
    fireEvent.click(screen.getByTestId('home-chat-call-record'))
    expect(rec).toHaveBeenCalledTimes(1)
    expect(screen.queryByTestId('home-chat-call-stop-recording')).toBeNull()
    rerender(strip({ badge: 'Alex started recording', canRecord: false, canStop: true }))
    const badge = screen.getByTestId('home-chat-call-recording')
    expect(badge.textContent).toBe('Recording')
    expect(badge.getAttribute('title')).toBe('Alex started recording')
    // Never red words: the dot carries the colour.
    expect(badge.className).not.toMatch(/status-error|danger/)
    expect(badge.querySelector('[data-recording-dot]')?.className).toContain('--color-status-error')
    expect(screen.queryByTestId('home-chat-call-record')).toBeNull()
    fireEvent.click(screen.getByTestId('home-chat-call-stop-recording'))
    expect(stop).toHaveBeenCalledTimes(1)
  })

  it("the strip's rule is the call bar's: the plugin's word, never a Voice & Chat only login; the badge names the starter", () => {
    const recording = { recordingId: '42', myConsent: 'granted' as const, recorderPresent: true, trigger: 'manual' as const, required: false, startedBy: '7', prompt: null }
    const participants = [{ user: 'Alex', speaking: false, muted: false, memberId: '7' }]
    expect(dmCallRecording({ recording: null, recordingPerms: null, recordingControls: null, participants })).toEqual({ badge: null, canRecord: false, canStop: false })
    expect(dmCallRecording({ recording: null, recordingPerms: { recordCalls: true, manageChannels: false }, recordingControls: { canRecord: true, canStop: false }, participants })).toEqual({
      badge: null,
      canRecord: true,
      canStop: false,
    })
    expect(
      dmCallRecording({ recording: null, recordingPerms: { recordCalls: true, manageChannels: false, voiceChatOnly: true }, recordingControls: { canRecord: true, canStop: true }, participants }),
    ).toEqual({ badge: null, canRecord: false, canStop: false })
    expect(dmCallRecording({ recording, recordingPerms: null, recordingControls: { canRecord: false, canStop: true }, participants })).toEqual({
      badge: 'Alex started recording',
      startedAt: null,
      canRecord: false,
      canStop: true,
    })
    const at = new Date(2026, 10, 17, 14, 41).getTime()
    expect(dmCallRecording({ recording: { ...recording, startedAt: at }, recordingPerms: null, recordingControls: null, participants }).badge).toMatch(
      /^Alex started recording at 2:41\sPM$/,
    )
  })

  it('the strip on another device shows the badge, with its clock, and no Record or Stop', () => {
    render(
      <HomeChatCallStrip
        seconds={null}
        muted={false}
        elsewhere="another device"
        onMute={vi.fn()}
        onLeave={vi.fn()}
        recording={{ badge: 'Alex started recording at 2:41 PM', startedAt: Date.now() - 192_000, canRecord: true, canStop: true }}
        onRecord={vi.fn()}
        onStop={vi.fn()}
      />,
    )
    expect(screen.getByTestId('home-chat-call-recording').textContent).toBe('Recording 03:12')
    expect(screen.queryByTestId('home-chat-call-record')).toBeNull()
    expect(screen.queryByTestId('home-chat-call-stop-recording')).toBeNull()
  })
})
