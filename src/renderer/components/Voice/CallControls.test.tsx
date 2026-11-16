// @vitest-environment jsdom
//
// The call's wave button (Sew 0.10.2, Rosson): noise removal, on by default,
// one tap turns it off live; its look and its tooltip say which. It is the
// voice prefs' `noiseRemoval` (Settings → Voice), passed in; a seat-only
// call (no native engine) has none.

import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import type { VoiceCall } from '@/stores/voice-core'
import CallControls, { noiseRemovalLabel, type CallControlsProps } from './CallControls'

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
