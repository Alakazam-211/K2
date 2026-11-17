// @vitest-environment jsdom
//
// The call's wave button (Sew 0.10.2, Rosson): noise removal, on by default,
// one tap turns it off live; its look and its tooltip say which. It is the
// voice prefs' `noiseRemoval` (Settings → Voice), passed in; a seat-only
// call (no native engine) has none.

import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import type { VoiceCall } from '@/stores/voice-core'
import CallControls, { CALL_BAR_BUTTON, CALL_PANEL_BUTTON, CallControlBar, CallPanelButton, noiseRemovalLabel, type CallControlsProps } from './CallControls'

const call: VoiceCall = {
  hostKey: 'sewlab.k2.dev',
  serverLabel: 'sewlab',
  roomId: 'room-1',
  roomName: 'Standup',
  callId: 'r_x',
  phase: 'connected',
  muted: false,
  deafened: false,
  speakDenied: false,
  serverMuted: false,
  micDenied: false,
  relayOnly: false,
  connectedAt: 1,
  participants: [],
  recording: null,
  recordingNotice: null,
  recordingPerms: null,
  recordingControls: null,
}

function actions(): CallControlsProps['actions'] & { setNoiseRemoval: ReturnType<typeof vi.fn> } {
  return { setMuted: vi.fn(), setDeafened: vi.fn(), leave: vi.fn(async () => {}), setNoiseRemoval: vi.fn() }
}

afterEach(cleanup)

describe('the wave button', () => {
  for (const variant of ['topbar', 'tile', 'overlay'] as const) {
    it(`${variant}: on shows on (plain wave, "Noise removal on"), a tap asks for off`, () => {
      const a = actions()
      render(<CallControls variant={variant} call={call} actions={a} video={null} noiseRemoval />)
      const b = screen.getByTestId(`call-${variant}-noise`)
      expect(b.getAttribute('data-state')).toBe('on')
      expect(b.getAttribute('aria-pressed')).toBe('true')
      expect(b.getAttribute('data-on')).toBe('false')
      expect(b.getAttribute('title')).toBe('Noise removal on')
      expect(b.querySelector('[data-k2-icon]')?.getAttribute('data-k2-icon')).toBe('audio-waveform')
      fireEvent.click(b)
      expect(a.setNoiseRemoval).toHaveBeenCalledWith(false)
    })
  }

  it('off shows off (the slashed wave, engaged, "Noise removal off"), a tap asks for on', () => {
    const a = actions()
    render(<CallControls variant="tile" call={call} actions={a} video={null} noiseRemoval={false} />)
    const b = screen.getByTestId('call-tile-noise')
    expect(b.getAttribute('data-state')).toBe('off')
    expect(b.getAttribute('aria-pressed')).toBe('false')
    expect(b.getAttribute('data-on')).toBe('true')
    expect(b.getAttribute('title')).toBe('Noise removal off')
    expect(b.querySelector('[data-k2-icon]')?.getAttribute('data-k2-icon')).toBe('audio-waveform-off')
    fireEvent.click(b)
    expect(a.setNoiseRemoval).toHaveBeenCalledWith(true)
  })

  it('a seat-only call (no native engine: null) has no wave button', () => {
    render(<CallControls variant="topbar" call={call} actions={actions()} video={null} noiseRemoval={null} />)
    expect(screen.queryByTestId('call-topbar-noise')).toBeNull()
  })

  it('the tooltip words', () => {
    expect([noiseRemovalLabel(true), noiseRemovalLabel(false)]).toEqual(['Noise removal on', 'Noise removal off'])
  })
})

describe('muted, in words', () => {
  for (const variant of ['topbar', 'tile', 'overlay'] as const) {
    it(`${variant}: the mic's name and tooltip say what a press does (Mute, then Unmute)`, () => {
      const { rerender } = render(<CallControls variant={variant} call={call} actions={actions()} video={null} />)
      const mic = screen.getByTestId(`call-${variant}-mute`)
      expect([mic.getAttribute('aria-label'), mic.getAttribute('title'), mic.getAttribute('aria-pressed')]).toEqual(['Mute', 'Mute', 'false'])
      rerender(<CallControls variant={variant} call={{ ...call, muted: true }} actions={actions()} video={null} />)
      expect([mic.getAttribute('aria-label'), mic.getAttribute('title'), mic.getAttribute('aria-pressed')]).toEqual(['Unmute', 'Unmute', 'true'])
    })
  }

  it("on the call's bar, a muted mic carries the word Muted under its glyph, small and in the look's chip", () => {
    const { rerender } = render(<CallControls variant="overlay" call={call} actions={actions()} video={null} />)
    expect(screen.queryByTestId('call-overlay-muted')).toBeNull()
    rerender(<CallControls variant="overlay" call={{ ...call, muted: true }} actions={actions()} video={null} />)
    const mic = screen.getByTestId('call-overlay-mute')
    const word = screen.getByTestId('call-overlay-muted')
    expect(word.textContent).toBe('Muted')
    // On the mic button itself, so nothing in the bar moves when it shows.
    expect(mic.contains(word)).toBe(true)
    expect(mic.querySelector('[data-k2-icon]')?.getAttribute('data-k2-icon')).toBe('mic-off')
    expect(word.className).toMatch(/\babsolute\b/)
    expect(word.className).toMatch(/\btype-badge\b/)
    expect(word.className).toContain('rounded-[var(--radius-pill)]')
    expect(word.getAttribute('data-k2-chamfer')).toBe('chip')
    // The button's own name already says it; the word is for the eye.
    expect(word.getAttribute('aria-hidden')).toBe('true')
  })
})

describe("the controls' look", () => {
  it("the call's bar (overlay): 40px buttons, round in Glass and cut in Terminal, with a visible focus ring", () => {
    render(<CallControls variant="overlay" call={call} actions={actions()} video={null} noiseRemoval />)
    const buttons = Array.from(screen.getByTestId('call-overlay-controls').querySelectorAll('button'))
    expect(buttons.length).toBe(4)
    for (const b of buttons) {
      expect([b.style.width, b.style.height]).toEqual([`${CALL_BAR_BUTTON}px`, `${CALL_BAR_BUTTON}px`])
      expect(b.getAttribute('data-k2-chamfer')).toBe('control')
      expect(b.className).toContain('rounded-[var(--radius-pill)]')
      expect(b.className).toContain('k2-focus')
    }
    expect(CALL_BAR_BUTTON).toBeGreaterThanOrEqual(36)
    expect(CALL_BAR_BUTTON).toBeLessThanOrEqual(44)
  })

  it('hang up stays red, with its own focus line on the red', () => {
    render(<CallControls variant="overlay" call={call} actions={actions()} video={null} />)
    const leave = screen.getByTestId('call-overlay-leave')
    expect(leave.className).toContain('bg-[var(--color-danger)]')
    expect(leave.className).toContain('text-[var(--color-on-danger)]')
    expect(leave.className).toContain('[--k2-ch-focus-inner:var(--color-on-danger)]')
  })

  it('the headset list and call view buttons are 36px, cut or round as the bar', () => {
    render(
      <CallPanelButton label="Mute" testId="p" onPress={() => {}}>
        x
      </CallPanelButton>
    )
    const b = screen.getByTestId('p')
    expect([b.style.width, b.style.height]).toEqual([`${CALL_PANEL_BUTTON}px`, `${CALL_PANEL_BUTTON}px`])
    expect(CALL_PANEL_BUTTON).toBeGreaterThanOrEqual(36)
    expect(b.getAttribute('data-k2-chamfer')).toBe('control')
    expect(b.className).toContain('rounded-[var(--radius-pill)]')
    expect(b.className).toContain('k2-focus')
  })

  it('the top bar keeps its own small icon buttons', () => {
    render(<CallControls variant="topbar" call={call} actions={actions()} video={null} />)
    expect(screen.getByTestId('call-topbar-mute').getAttribute('data-k2-chamfer')).toBeNull()
  })

  it("the slim bar's icon buttons show keyboard focus in the look: a circle in Glass, square in Terminal", () => {
    render(<CallControls variant="topbar" call={call} actions={actions()} video={null} noiseRemoval />)
    const buttons = Array.from(screen.getByTestId('call-topbar-controls').querySelectorAll('button'))
    expect(buttons.length).toBe(4)
    for (const b of buttons) {
      expect(b.className).toMatch(/(^|\s)k2-focus(\s|$)/)
      expect(b.className).toContain('rounded-[var(--radius-pill)]')
    }
  })

  it('the bar under the call is Frost in Glass (a pill) and a raised cut bar in Terminal', () => {
    render(
      <CallControlBar>
        <span />
      </CallControlBar>
    )
    const bar = screen.getByTestId('call-control-bar')
    expect(bar.getAttribute('data-material')).toBe('frost')
    expect(bar.getAttribute('data-frost-shape')).toBe('pill')
    expect(bar.getAttribute('data-surface')).toBe('elevated')
    expect(bar.getAttribute('data-k2-chamfer')).toBe('surface')
  })
})
