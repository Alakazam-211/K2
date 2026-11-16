// @vitest-environment jsdom
// The live timeline tab on a real file (T7), its socket scripted from the
// timeline pack's watched-turn golden: the real lanes, no Sample data banner
// or chip, and a monitor that says Preparing video and draws nothing (never
// sample footage, no waveforms); a refusal at open is the can't-be-live card.

import { act, cleanup, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import type { ServerScope } from '@/kessel/server-scope'
import { LiveConnectError, type LiveClose, type LiveTransport, type LiveWelcome } from '@/lib/docs-live-transport'
import golden from '@/lib/live-view/timeline/samples/pack-trim_held_outside_save.json'
import TimelineLive from './TimelineLive'

vi.mock('@/lib/daemon-cli', () => ({ daemonCliPost: vi.fn(async () => ({ ok: true })) }))

afterEach(() => cleanup())

type Step = { step: string; roster: Record<string, unknown>[]; watch?: Record<string, unknown>[] }
const STEPS = (golden as unknown as { steps: Step[] }).steps
const WELCOME = { type: 'welcome', root: '', version: 0, client: 1, self: 'ada', rights: 'edit', features: ['keyed_watch', 'view_address', 'time_position'] } as unknown as LiveWelcome

/** K2's docs socket, scripted: the welcome, then on `watch` the golden's frames up to `upto`. */
function scripted(upto: string, refuse?: LiveClose): () => LiveTransport {
  return () => {
    const frames = new Set<(f: unknown) => void>()
    const push = (f: unknown): void => [...frames].forEach((cb) => cb(f))
    return {
      open: true,
      connect: async () => {
        if (refuse) throw new LiveConnectError(refuse)
        return WELCOME
      },
      send: (f: { type?: string }) => {
        SENT.push(f as Record<string, unknown>)
        if (f.type !== 'watch') return
        queueMicrotask(() => {
          for (const s of STEPS) {
            for (const e of s.roster) push({ type: 'roster_event', event: e })
            for (const w of s.watch ?? []) push(w)
            if (s.step === upto) break
          }
        })
      },
      onFrame: (cb: (f: unknown) => void) => (frames.add(cb), () => frames.delete(cb)),
      onClose: () => () => {},
      close: () => {},
    } as unknown as LiveTransport
  }
}

const SENT: Record<string, unknown>[] = []
const SCOPE = { id: 'local' } as unknown as ServerScope

async function mount(createTransport: () => LiveTransport): Promise<void> {
  render(<TimelineLive scope={SCOPE} filePath="/w/edits/cut.otio" width={1440} createTransport={createTransport} />)
  await act(async () => {})
  await act(async () => {})
}

describe('the live timeline tab on a real file', () => {
  it('draws the file’s own lanes, live, with no Sample data banner', async () => {
    await mount(scripted('trim'))
    expect(screen.queryByTestId('lv-sample-banner')).toBeNull()
    expect(screen.queryByTestId('lv-mon-sample')).toBeNull()
    expect(screen.getByTestId('lv-state').textContent).toBe('Live')
    expect(screen.getByTestId('lv-lane-V1')).toBeTruthy()
    expect(screen.getByTestId('lv-lane-A1')).toBeTruthy()
    const rows = screen.getAllByTestId('lv-change').map((r) => r.textContent ?? '')
    expect(rows.some((r) => r.includes('Atlas trimmed the start of Shot A by 6 frames'))).toBe(true)
  })

  it('the monitor says Preparing video and draws nothing: no footage, no pictures, no waveforms', async () => {
    await mount(scripted('watch'))
    const mon = screen.getByTestId('lv-monitor')
    // The file starts on a gap (Shot A is at 1 s): the gap's card, and Preparing video for the rest.
    expect(mon.getAttribute('data-state')).toBe('gap')
    expect(screen.getByTestId('lv-monitor-frame').getAttribute('data-paint')).toBe('none')
    expect(screen.getByTestId('lv-mon-prep').textContent).toMatch(/^Preparing video/)
    expect(document.querySelector('.lv-tl-wave')).toBeNull()
    expect(document.querySelector('.lv-tl-pic canvas, .lv-tl-strip canvas')).toBeNull()
  })

  it('says where you are: your view address and your playhead go out on the socket', async () => {
    SENT.length = 0
    await mount(scripted('watch'))
    expect(SENT).toContainEqual(expect.objectContaining({ type: 'view_address', address: expect.stringMatching(/^@/) }))
    expect(SENT).toContainEqual(expect.objectContaining({ type: 'time_position', time_position: expect.objectContaining({ at: '0', state: 'paused' }) }))
  })

  it('a file the pack won’t watch live opens on the can’t-be-live card, in its words', async () => {
    await mount(scripted('watch', { reason: 'declined', retry: false, error: { id: 'MPC-E201', message: 'one timeline', details: { code: 'live_many_timelines' } } as never }))
    expect(screen.queryByTestId('lv-sample-banner')).toBeNull()
    expect(document.body.textContent).toContain('This file holds more than one timeline')
    expect(screen.getByText('Open the saved file')).toBeTruthy()
  })
})
