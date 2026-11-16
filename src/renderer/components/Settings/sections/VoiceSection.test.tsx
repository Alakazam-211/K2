// @vitest-environment jsdom
//
// Settings → Voice → Noise removal (Sew 0.10.2): on unless this computer's
// person turned it off; the switch saves the choice through the voice prefs
// (which also send it to the native engine), and the line under it says
// what each position means.

import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({ prefs: { noiseRemoval: true }, setPrefs: vi.fn(), web: false }))

vi.mock('@/lib/is-web', () => ({ isWebClient: () => h.web }))
vi.mock('@/stores/voice', () => ({
  useVoice: (sel: (s: { prefs: { noiseRemoval: boolean } }) => unknown) => sel({ prefs: h.prefs }),
  voice: { setPrefs: (p: unknown) => h.setPrefs(p) },
}))

import { VoiceSection, VOICE_MANIFEST } from './VoiceSection'

afterEach(() => {
  cleanup()
  h.setPrefs.mockReset()
  h.prefs = { noiseRemoval: true }
  h.web = false
})

describe('VoiceSection', () => {
  it('shows noise removal on by default, and turning it off saves off', () => {
    render(<VoiceSection />)
    const sw = screen.getByTestId('voice-noise-removal')
    expect(sw.getAttribute('aria-checked')).toBe('true')
    expect(screen.getByText(/background noise like typing/)).toBeTruthy()
    fireEvent.click(sw)
    expect(h.setPrefs).toHaveBeenCalledWith({ noiseRemoval: false })
  })

  it('off says the full microphone goes out, echo cancellation kept', () => {
    h.prefs = { noiseRemoval: false }
    render(<VoiceSection />)
    expect(screen.getByTestId('voice-noise-removal').getAttribute('aria-checked')).toBe('false')
    expect(screen.getByText(/others hear your full microphone/)).toBeTruthy()
    expect(screen.getByText(/Echo cancellation and volume levelling stay on/)).toBeTruthy()
    fireEvent.click(screen.getByTestId('voice-noise-removal'))
    expect(h.setPrefs).toHaveBeenCalledWith({ noiseRemoval: true })
  })

  it('the web client says it applies in the desktop app', () => {
    h.web = true
    render(<VoiceSection />)
    expect(screen.getByText(/Applies to calls in the K2 desktop app/)).toBeTruthy()
  })

  it('search finds it under Voice', () => {
    expect(VOICE_MANIFEST).toEqual([expect.objectContaining({ id: 'voice.noise-removal', section: 'voice', label: 'Noise removal' })])
  })
})
