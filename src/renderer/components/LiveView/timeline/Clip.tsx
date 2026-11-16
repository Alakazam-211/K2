// One clip on a lane: coloured by media type; a 16 px name band whose label
// sits at the visible start of the clip and holds the full name, else a
// whole short name, else the kind glyph (never a cut word, never "…");
// marks after the name, never over it (the changed dot, a hold's lock, the
// refusal's glyph); a picture under the band on tall lanes, the head beside
// it on short ones, a waveform in the lower part of audio clips. Pictures
// are the sample's drawn stills until K2 Render's tiles (T8). The full name
// is always the tooltip and the accessible name.

import { useEffect, useRef, type CSSProperties } from 'react'
import { personInk } from '@/components/HomeChat/HomeChatGlyphs'
import { drawScene, tileCanvas, waveAmp } from '@/lib/live-view/timeline/footage'
import type { Item, TimelineModel } from '@/lib/live-view/timeline/fold'
import { kindGlyphOf, labelForRoom } from '@/lib/live-view/timeline/labels'
import { NAME_H, type ClipMark } from '@/lib/live-view/timeline/lanes-view'
import { pictureTiles, srcFrameAt, STRIP_MIN, type MediaFacts, type MediaInfo } from '@/lib/live-view/timeline/picture'
import { ClipMarkShape, KindIcon, LockGlyph, StopGlyph } from './glyphs'
import type { Box } from './box'
import { PAINTS } from './measure'

const HEX = /^#[0-9a-f]{6}$/i
const inkOn = (c: string): string => (HEX.test(c) ? personInk(c) : 'var(--color-on-accent)')

export interface ClipProps {
  /** The model, boxed (box.ts). */
  model: Box<TimelineModel>
  it: Item
  trackKind: string
  /** Left and width on the lane (px). */
  x: number
  w: number
  /** The lane's height. */
  h: number
  /** The clip's visible part, in its own px (scrolled-off ends excluded). */
  a: number
  b: number
  mark?: ClipMark
  /** The picture mode for this lane (off, head, headtail, strip); audio lanes use `wave`. */
  mode: string
  wave: boolean
  ppf: number
  mi: MediaInfo | null
  facts: MediaFacts
  /** The head tile at a new in-point is on its way. */
  pending: boolean
  measure: (text: string) => number
}

const dprOf = (): number => (typeof window === 'undefined' ? 1 : Math.min(2, window.devicePixelRatio || 1))

/** The clip's picture geometry: under the band on tall lanes, beside it (head, tail) on short ones. */
function pictureBox(p: ClipProps): { kind: 'strip' | 'side' | 'none'; pw: number; ph: number; head: boolean; tail: boolean } {
  const { it, trackKind, mi, mode, h, w } = p
  const isPic = trackKind === 'video' && it.kind === 'clip' && !!mi && mi.kind === 'video' && mode !== 'off'
  if (!isPic) return { kind: 'none', pw: 0, ph: 0, head: false, tail: false }
  if (h >= STRIP_MIN) return { kind: 'strip', pw: 0, ph: h - 4 - NAME_H, head: false, tail: false }
  const ph = h - 4
  const pw = Math.round((ph * 16) / 9)
  const head = w > pw + 60
  const tail = head && mode === 'headtail' && w > 2 * pw + 60
  return { kind: head ? 'side' : 'none', pw, ph, head, tail }
}

export function Clip(p: ClipProps): React.JSX.Element {
  const { it, trackKind, x, w, h, a, b, mark, ppf, mi, facts, pending, measure } = p
  const M = p.model.v
  // A sliver (a long edit fitted to the window) draws its colour only.
  const tiny = w < 6
  const pic = tiny ? ({ kind: 'none', pw: 0, ph: 0, head: false, tail: false } as const) : pictureBox(p)
  const canvas = useRef<HTMLCanvasElement>(null)
  const tailCanvas = useRef<HTMLCanvasElement>(null)
  const waveCanvas = useRef<HTMLCanvasElement>(null)
  const showWave = !tiny && p.wave && trackKind === 'audio' && it.kind === 'clip' && !!mi && w > 6
  const visW = Math.max(0, b - a)

  useEffect(() => {
    if (!PAINTS || !mi) return
    const dpr = dprOf()
    const cv = canvas.current
    if (cv && pic.kind === 'strip' && visW > 0) {
      const W = Math.ceil(visW * dpr)
      const H = Math.round(pic.ph * dpr)
      cv.width = W
      cv.height = H
      const g = cv.getContext('2d')
      if (g) {
        const tw = (pic.ph * 16) / 9
        for (const t of pictureTiles(M, it, p.mode, tw / ppf, Math.round(tw), facts)) {
          if (t.i === 0 && pending) continue
          const tx = t.tail ? w - tw : t.i * tw
          if (tx + tw < a || tx > b) continue
          const img = tileCanvas(mi, t.srcF)
          if (img) g.drawImage(img, Math.round((tx - a) * dpr), 0, Math.ceil(tw * dpr), H)
          g.fillStyle = 'rgba(0,0,0,.35)'
          g.fillRect(Math.round((tx - a) * dpr), 0, Math.max(1, dpr), H)
        }
      }
    }
    for (const [ref, at] of [
      [canvas, 0],
      [tailCanvas, Math.max(0, Math.round(it.f1 - it.f0) - 1)],
    ] as const) {
      const c = ref.current
      if (!c || pic.kind !== 'side' || (ref === tailCanvas && !pic.tail)) continue
      c.width = Math.round(pic.pw * dpr)
      c.height = Math.round(pic.ph * dpr)
      const g = c.getContext('2d')
      const src = srcFrameAt(M, it, it.f0)
      if (g && src != null && !(ref === canvas && pending)) drawScene(g, c.width, c.height, mi, src + at)
    }
  })

  useEffect(() => {
    const c = waveCanvas.current
    if (!PAINTS || !c || !showWave || visW <= 0) return
    const dpr = dprOf()
    const ph = h - 4 - NAME_H
    c.width = Math.ceil(visW * dpr)
    c.height = Math.round(ph * dpr)
    const g = c.getContext('2d')
    if (!g) return
    g.fillStyle = getComputedStyle(c).color || 'grey'
    const top = Math.min(c.height, Math.round(h * 0.4 * dpr))
    const step = 2 * dpr
    for (let px = 0; px < c.width; px += step) {
      const f = it.f0 + (a + px / dpr) / ppf
      const sf = srcFrameAt(M, it, f) ?? Math.floor(f)
      const hh = Math.max(1, waveAmp(mi, sf) * top)
      g.fillRect(px, c.height - hh, Math.max(1, step - dpr), hh)
    }
  })

  // The name band spans the visible part of the clip, beside a short lane's pictures.
  const left = pic.head ? pic.pw + 2 : 0
  const right = pic.tail ? pic.pw + 2 : 0
  const bandL = Math.max(a, left)
  const bandR = Math.min(b, w - right)
  const room = bandR - bandL
  const glyph = kindGlyphOf(it.kind, trackKind)
  const full = it.name || 'No name'
  const badges = (mark?.changed?.fresh ? 12 : 0) + (mark?.held ? 18 : 0) + (mark?.refused ? 18 : 0)
  const label = w >= 14 && room > 0 ? labelForRoom(full, glyph, room, measure, badges) : null
  const kind = trackKind === 'audio' ? 'audio' : it.kind === 'generator' ? 'title' : it.kind === 'nest' ? 'nest' : 'video'

  const words = [full]
  if (it.off) words.push('turned off')
  if (mark?.changed) words.push(`changed ${mark.changed.fresh ? 'just now ' : ''}by ${mark.changed.name}`)
  if (mark?.held) words.push(`held by ${mark.held.name}`)
  if (mark?.refused) words.push(mark.refused.line)
  if (mark?.released) words.push(`let go by ${mark.released.name}`)
  const span = [M.tc(it.s), M.tc(it.e)].filter(Boolean).join('–')
  const title = [full, span, M.dur(it.f1 - it.f0), ...words.slice(1)].filter(Boolean).join(' · ')

  const style = {
    left: x,
    width: w,
    ...(mark?.held ? { '--lv-hc': mark.held.colour, '--lv-hink': inkOn(mark.held.colour) } : {}),
    ...(mark?.changed ? { '--lv-cc': mark.changed.colour } : {}),
    ...(mark?.released ? { '--lv-rc': mark.released.colour } : {}),
  } as CSSProperties
  return (
    <div
      className="lv-tl-clip"
      data-k={it.key}
      data-full={full}
      data-kind={kind}
      data-off={it.off ? '' : undefined}
      data-held={mark?.held ? '' : undefined}
      data-changed={mark?.changed ? '' : undefined}
      data-refused={mark?.refused ? '' : undefined}
      data-released={mark?.released && !mark.held ? '' : undefined}
      role="img"
      aria-label={words.join(', ')}
      title={title}
      style={style}
    >
      {pic.kind === 'strip' && visW > 0 ? <canvas ref={canvas} className="lv-tl-pic" style={{ left: a, top: NAME_H, width: visW, height: pic.ph }} /> : null}
      {pic.kind === 'strip' && pending ? <i className="lv-tl-pend" style={{ width: Math.min(w, Math.round((pic.ph * 16) / 9)) }} /> : null}
      {pic.kind === 'side' ? <canvas ref={canvas} className="lv-tl-pic" style={{ left: 0, top: 0, width: pic.pw, height: pic.ph }} /> : null}
      {pic.kind === 'side' && pic.tail ? <canvas ref={tailCanvas} className="lv-tl-pic" style={{ left: w - pic.pw, top: 0, width: pic.pw, height: pic.ph }} /> : null}
      {showWave && visW > 0 ? <canvas ref={waveCanvas} className="lv-tl-wave" style={{ left: a, width: visW, height: h - 4 - NAME_H }} /> : null}
      {it.markers
        .filter((m) => m.f != null)
        .map((m) => (
          <i key={m.key} className="lv-tl-imk" title={m.name} style={{ left: ((m.f as number) - it.f0) * ppf, '--lv-mc': markerColour(m.color) } as CSSProperties}>
            <ClipMarkShape />
          </i>
        ))}
      {label && room > 0 ? (
        <span className="lv-tl-band" style={{ left: bandL, width: room }}>
          {label.glyph ? (
            <span className="lv-tl-kind">
              <KindIcon kind={label.glyph} />
            </span>
          ) : null}
          {label.text !== null ? <span className="lv-tl-name">{label.text}</span> : null}
          {mark?.refused ? (
            <span className="lv-tl-stop">
              <StopGlyph size={11} />
            </span>
          ) : mark?.changed?.fresh ? (
            <span className="lv-tl-dot" />
          ) : null}
          {mark?.held ? (
            <span className="lv-tl-lock" data-k2-chamfer="chip">
              <LockGlyph size={8} />
            </span>
          ) : null}
        </span>
      ) : null}
    </div>
  )
}

/** A marker's data colour as the theme's ink (the data says RED, GREEN, BLUE, YELLOW or PURPLE). */
export function markerColour(c: string): string {
  switch (String(c || '').toUpperCase()) {
    case 'RED':
      return 'var(--color-status-error)'
    case 'BLUE':
      return 'var(--color-chart-1)'
    case 'YELLOW':
      return 'var(--color-status-warn)'
    case 'PURPLE':
      return 'var(--color-chart-other)'
    case 'GREEN':
      return 'var(--color-status-ok)'
    default:
      return 'var(--color-text-secondary)'
  }
}
