// @vitest-environment jsdom
// The live timeline tab on a real file (T7), its socket scripted from the
// timeline pack's watched-turn golden: the real lanes, no Sample data banner
// or chip, and a monitor that says Preparing video and draws nothing (never
// sample footage, no waveforms); a refusal at open is the can't-be-live card.

import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import type { ServerScope } from '@/kessel/server-scope'
import { LiveConnectError, type LiveClose, type LiveTransport, type LiveWelcome } from '@/lib/docs-live-transport'
import golden from '@/lib/live-view/timeline/samples/pack-trim_held_outside_save.json'
import type { TimelineModel } from '@/lib/live-view/timeline/fold'
import { KeyedWatchTimelineSource } from '@/lib/live-view/timeline/keyed-source'
import { defaultPpf, openFitPpf } from '@/lib/live-view/timeline/lanes-view'
import TimelineLive from './TimelineLive'

vi.mock('@/lib/daemon-cli', () => ({ daemonCliPost: vi.fn(async () => ({ ok: true })) }))

afterEach(() => cleanup())

type Step = { step: string; roster: Record<string, unknown>[]; watch?: Record<string, unknown>[] }
const STEPS = (golden as unknown as { steps: Step[] }).steps
const WELCOME = { type: 'welcome', root: 'tracks', version: 0, client: 1, self: 'ada', rights: 'edit', features: ['keyed_watch', 'view_address', 'time_position'] } as unknown as LiveWelcome

/** The live socket's push, for a frame after the golden's (the latest scripted socket). */
let pushLive: (f: unknown) => void = () => {}

/** K2's docs socket, scripted: the welcome, then on `watch` the golden's frames up to `upto`. */
/** What each socket was asked for: the doc and its root. */
const DIALLED: { path: string; root?: string | null }[] = []

function scripted(upto: string, refuse?: LiveClose, welcome: Record<string, unknown> = {}): (scope: unknown, doc: { path: string; root?: string | null }) => LiveTransport {
  return (_scope, doc) => {
    DIALLED.push(doc)
    const frames = new Set<(f: unknown) => void>()
    const push = (f: unknown): void => [...frames].forEach((cb) => cb(f))
    pushLive = push
    return {
      open: true,
      connect: async () => {
        if (refuse) throw new LiveConnectError(refuse)
        return { ...WELCOME, ...welcome } as LiveWelcome
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

/** The golden's edit as the real source folds it. */
function modelOfGolden(): TimelineModel {
  const subs = new Set<(f: unknown) => void>()
  const t = { send: () => {}, onFrame: (cb: (f: unknown) => void) => (subs.add(cb), () => subs.delete(cb)) }
  const src = new KeyedWatchTimelineSource(t as never, { post: async () => undefined })
  src.attach(WELCOME as never)
  for (const f of STEPS[0].watch!) [...subs].forEach((cb) => cb(f))
  return src.timeline().model
}
const endOf = (): number => modelOfGolden().endF

/** An outside save that makes Music on A1 \`times\` as long: the edit's length changes a lot. */
function longer(seq: number, rev: number, times: number): unknown {
  const before = seq === 13 ? '0,508032000000' : '0,5080320000000'
  const after = `0,${508032000000 * times}`
  return {
    type: 'ops_change', seq, rev, at_ms: 1793111402600 + seq, change_kind: 'external', actor: { id: 'kit:external', name: 'kit:external', kind: 'file', colour: null }, client: 4, via: 'disk', on_behalf_of: null, turn: null, verb: null, claim: null,
    ops: [{ root: 'item_props', kind: 'map', op: 'set', key: '@item:6', field: 'geom', before, after }],
    units: [{ key: '@item:6', container: '@trk:2', ops: ['move_resize'], geom_before: before, geom_after: after, fields: ['geom'], hidden_after: null, placeholder_type: null }],
    containers_changed: [], containers: ['@trk:2'], derived: null, render_hashes: null,
  }
}
const SCOPE = { id: 'local' } as unknown as ServerScope

async function mount(createTransport: (scope: unknown, doc: { path: string; root?: string | null }) => LiveTransport, filePath = '/w/edits/cut.otio'): Promise<void> {
  render(<TimelineLive scope={SCOPE} filePath={filePath} width={1440} createTransport={createTransport} />)
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

  it('with no lane tiles (null previews: no host decodes video yet), the lanes and the monitor say no preview and draw nothing', async () => {
    await mount(scripted('watch'))
    const mon = screen.getByTestId('lv-monitor')
    // The file starts on a gap (Shot A is at 1 s): the gap's card; no Preparing video, as nothing is coming.
    expect(mon.getAttribute('data-state')).toBe('gap')
    expect(screen.getByTestId('lv-monitor-frame').getAttribute('data-paint')).toBe('none')
    expect(screen.queryByTestId('lv-mon-prep')).toBeNull()
    // On a clip: the no-preview card.
    await act(async () => void fireEvent.click(screen.getByTestId('lv-mon-skip')))
    expect(screen.getByTestId('lv-monitor').getAttribute('data-state')).toBe('no_preview')
    expect(screen.getByTestId('lv-monitor-frame').getAttribute('data-paint')).toBe('none')
    expect(document.querySelector('.lv-tl-wave')).toBeNull()
    expect(document.querySelector('.lv-tl-pic canvas, .lv-tl-strip canvas')).toBeNull()
  })

  it('says where you are: your view address and your playhead go out on the socket', async () => {
    SENT.length = 0
    await mount(scripted('watch'))
    expect(SENT).toContainEqual(expect.objectContaining({ type: 'view_address', address: expect.stringMatching(/^@/) }))
    expect(SENT).toContainEqual(expect.objectContaining({ type: 'time_position', time_position: expect.objectContaining({ at: '0', state: 'paused' }) }))
  })

  it.each(['/w/edits/cut.fcpxml', '/w/edits/cut.prproj'])('%s: the timeline core indexes it but only .otio is live, so the socket (no root) is declined and the card says so', async (path) => {
    DIALLED.length = 0
    await mount(scripted('watch', { reason: 'declined', retry: false, code: 'not_a_text_doc', message: 'not a live doc' }), path)
    expect(DIALLED.at(-1)).toEqual({ path, root: null })
    expect(screen.queryByTestId('lv-sample-banner')).toBeNull()
    expect(document.body.textContent).toContain('Only .otio timelines are live, for now')
    expect(screen.getByText('Open the saved file')).toBeTruthy()
  })

  it('an .otio dials its own root, tracks, and is plain Live', async () => {
    DIALLED.length = 0
    await mount(scripted('watch'))
    expect(DIALLED.at(-1)).toEqual({ path: '/w/edits/cut.otio', root: 'tracks' })
    expect(screen.getByTestId('lv-state').getAttribute('data-state')).toBe('live')
  })

  it('a welcome with read_only_reason is live but read only, the reason on the chip', async () => {
    await mount(scripted('watch', undefined, { rights: 'read', read_only_reason: 'nobody may change this timeline yet' }))
    const chip = screen.getByTestId('lv-state')
    expect(chip.getAttribute('data-state')).toBe('read-only-file')
    expect(chip.getAttribute('title') ?? '').toContain('nobody may change this timeline yet')
  })

  it('opens fitted: the whole edit across the lanes, not the default zoom', async () => {
    await mount(scripted('watch'))
    const lanes = screen.getByTestId('lv-lanes')
    const ppf = Number(lanes.getAttribute('data-ppf'))
    const tw = Number(lanes.getAttribute('data-tw'))
    expect(ppf).toBeCloseTo(openFitPpf(endOf(), tw), 3)
    expect(ppf).not.toBeCloseTo(defaultPpf(modelOfGolden(), tw), 3)
    // The whole edit shows, with a margin.
    expect(endOf() * ppf).toBeLessThan(tw)
    expect(endOf() * ppf).toBeGreaterThan(tw * 0.9)
  })

  it('re-fits when the edit grows a lot before you touch the lanes, never after you scroll', async () => {
    await mount(scripted('watch'))
    const lanes = screen.getByTestId('lv-lanes')
    const before = Number(lanes.getAttribute('data-ppf'))
    await act(async () => pushLive(longer(13, 2, 10)))
    const after = Number(lanes.getAttribute('data-ppf'))
    expect(after).toBeLessThan(before * 0.5)
    // You scroll: the lanes stay as they are, however long the edit gets.
    const scroller = lanes.querySelector('[data-scroll-edges-list]') as HTMLElement
    await act(async () => void scroller.dispatchEvent(new Event('pointerdown')))
    await act(async () => pushLive(longer(14, 3, 40)))
    expect(Number(lanes.getAttribute('data-ppf'))).toBeCloseTo(after, 6)
  })

  it('Atlas, Nova and You are three different colours: others take presence slots, never the blue that reads as yours', async () => {
    await mount(scripted('trim'))
    const avatars = within(screen.getByTestId('lv-stack')).getAllByTestId('lv-avatar').map((a) => (a as HTMLElement).style.backgroundColor)
    expect(avatars).toContain('var(--color-presence-1)')
    expect(avatars).toContain('var(--color-presence-3)')
    expect(avatars).not.toContain('var(--color-presence-2)')
    expect(new Set(avatars).size).toBe(avatars.length)
    // Atlas's playhead tag and hold read in the same slot.
    const tags = screen.queryAllByTestId('lv-ruler-tag').map((t) => t.getAttribute('style') ?? '')
    expect(tags.some((s) => s.includes('--color-presence-1'))).toBe(true)
    expect(tags.join(' ')).not.toContain('#0ea5e9')
  })

  it('a file the pack won’t watch live opens on the can’t-be-live card, in its words', async () => {
    await mount(scripted('watch', { reason: 'declined', retry: false, error: { id: 'MPC-E201', message: 'one timeline', details: { code: 'live_many_timelines' } } as never }))
    expect(screen.queryByTestId('lv-sample-banner')).toBeNull()
    expect(document.body.textContent).toContain('This file holds more than one timeline')
    expect(screen.getByText('Open the saved file')).toBeTruthy()
  })
})
