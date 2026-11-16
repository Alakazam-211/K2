// The view picker, top-left on the stage: the view on screen by name, and a
// menu of the file's views (Scene, each camera, the framed object,
// Turntable, and the agent's view under a divider), each with a small still,
// what it is, and its key. The highlight follows Up, Down, Home and End;
// Enter or a click picks; Esc or Tab closes and gives focus back.

import { Fragment, useEffect, useRef } from 'react'
import { cx } from '@/components/ui'
import { useBrowserOverlay } from '@/lib/browser-overlay'
import { THUMB_WIDTH, type PictureSource } from '@/lib/live-view/scene/picture-source'
import { kindOf, type ViewRow } from '@/lib/live-view/scene/views'
import { ChevronGlyph } from '../shell/glyphs'
import { CameraGlyph, FramedGlyph, SceneViewGlyph, TurntableGlyph } from './scene-glyphs'
import { useStill } from './useStage'

function ViewGlyph({ v, size = 14 }: { v: string; size?: number }): React.JSX.Element {
  const k = kindOf(v)
  return k === 'camera' ? <CameraGlyph size={size} /> : k === 'object' ? <FramedGlyph size={size} /> : k === 'turntable' ? <TurntableGlyph size={size} /> : <SceneViewGlyph size={size} />
}

function Thumb({ source, rev, view }: { source: PictureSource | null; rev: number | null; view: string | null }): React.JSX.Element {
  const st = useStill(source, rev != null && view ? { rev, view, shape: 'wide', width: THUMB_WIDTH } : null)
  const ref = useRef<HTMLCanvasElement>(null)
  useEffect(() => {
    const cv = ref.current
    if (!cv || !st) return
    cv.width = st.width
    cv.height = st.height
    const ctx = cv.getContext('2d')
    if (!ctx) return
    if (st.image.kind === 'bitmap') ctx.drawImage(st.image.image, 0, 0)
    else if (typeof ImageData !== 'undefined') ctx.putImageData(new ImageData(st.image.data as Uint8ClampedArray<ArrayBuffer>, st.width, st.height), 0, 0)
  }, [st])
  return <span className="lv-th">{st ? <canvas ref={ref} aria-hidden="true" /> : view ? <ViewGlyph v={view} size={16} /> : null}</span>
}

export interface ViewPickerProps {
  name: string
  view: string
  rows: ViewRow[]
  /** Which row is the view on screen. */
  checked: (r: ViewRow) => boolean
  open: boolean
  /** The row the scenario's keys are on (a shown highlight, not focus). */
  kbd: string | null
  onOpen: (open: boolean) => void
  onPick: (r: ViewRow) => void
  /** Thumbnails: the still on screen's rev, each row's still view. */
  source: PictureSource | null
  rev: number | null
  thumbView: (r: ViewRow) => string | null
  /** The button's own words (the second pane: "Atlas’s view"); default the view's name. */
  label?: string
  /** What the button is, for assistive tech ("Second view"); default "View". */
  prefix?: string
  /** The cameras the picture can't show: one quiet line under the cameras, each name and why in its tooltip. */
  leftOut?: { line: string; details: string[] } | null
}

/** Under the cameras: how many the picture can't show; each one's name and why in the tooltip (and read out whole). */
function LeftOutLine({ words }: { words: { line: string; details: string[] } }): React.JSX.Element {
  return (
    <div role="none">
      <div role="note" className="lv-vleft" data-testid="lv-cameras-left-out" title={words.details.length ? words.details.join('\n') : undefined} aria-label={[words.line, ...words.details].join('. ')}>
        {words.line}
      </div>
    </div>
  )
}

export function ViewPicker(p: ViewPickerProps): React.JSX.Element {
  const btn = useRef<HTMLButtonElement>(null)
  const menu = useRef<HTMLDivElement>(null)
  const wasOpen = useRef(p.open)
  // The menu claims its box, so a Browser page beside the stage gives way while it's open.
  useBrowserOverlay({ active: p.open, ref: menu, kind: 'menu' })
  // Opened by the viewer: focus the checked row (else the first that can be picked).
  useEffect(() => {
    if (p.open && !wasOpen.current && !p.kbd) {
      const m = menu.current
      const r = m?.querySelector<HTMLButtonElement>('[aria-checked="true"]:not(:disabled)') ?? m?.querySelector<HTMLButtonElement>('[role="menuitemradio"]:not(:disabled)')
      r?.focus()
    }
    wasOpen.current = p.open
  }, [p.open, p.kbd])
  useEffect(() => {
    if (!p.open) return
    const away = (e: PointerEvent): void => {
      const t = e.target as Node
      if (!menu.current?.contains(t) && !btn.current?.contains(t)) p.onOpen(false)
    }
    document.addEventListener('pointerdown', away)
    return () => document.removeEventListener('pointerdown', away)
  }, [p.open, p])

  const close = (): void => {
    p.onOpen(false)
    btn.current?.focus()
  }
  const onKey = (e: React.KeyboardEvent): void => {
    const rows = Array.from(menu.current?.querySelectorAll<HTMLButtonElement>('[role="menuitemradio"]:not(:disabled)') ?? [])
    const i = rows.indexOf(document.activeElement as HTMLButtonElement)
    let j: number | null = null
    if (e.key === 'ArrowDown') j = i < 0 ? 0 : Math.min(rows.length - 1, i + 1)
    else if (e.key === 'ArrowUp') j = i < 0 ? rows.length - 1 : Math.max(0, i - 1)
    else if (e.key === 'Home') j = 0
    else if (e.key === 'End') j = rows.length - 1
    else if (e.key === 'Escape' || e.key === 'Tab') {
      e.preventDefault()
      e.stopPropagation()
      close()
      return
    }
    if (j !== null) {
      e.preventDefault()
      e.stopPropagation()
      rows[j]?.focus()
    }
  }

  return (
    <>
      <button
        ref={btn}
        type="button"
        data-k2-focus=""
        data-k2-chamfer="control"
        aria-haspopup="menu"
        aria-expanded={p.open}
        aria-label={`${p.prefix ?? 'View'}: ${p.label ?? p.name}. Change view`}
        className="lv-vbtn lv-plate no-drag"
        onClick={() => p.onOpen(!p.open)}
      >
        <ViewGlyph v={p.view} />
        <span>{p.label ?? p.name}</span>
        <ChevronGlyph size={13} />
      </button>
      {p.open ? (
        <div ref={menu} role="menu" aria-label="Views" className="lv-pop lv-vmenu" data-k2-chamfer="surface" onKeyDown={onKey}>
          {p.rows.map((r, i) => {
            const on = p.checked(r)
            const lastCamera = r.kind === 'camera' && p.rows[i + 1]?.kind !== 'camera'
            return (
              <Fragment key={r.v}>
                <div role="none">
                  {r.sep ? <div className="lv-vsep" role="separator" /> : null}
                  <button
                    type="button"
                    role="menuitemradio"
                    aria-checked={on}
                    aria-label={`${r.name}, ${r.sub}`}
                    disabled={r.disabled}
                    data-kbd={p.kbd && p.kbd === r.v ? '' : undefined}
                    className={cx('lv-vrow')}
                    onClick={() => p.onPick(r)}
                  >
                    <Thumb source={r.disabled ? null : p.source} rev={p.rev} view={p.thumbView(r)} />
                    <span className="lv-vt" aria-hidden="true">
                      <b>
                        {r.name}
                        {on ? <span className="lv-check"> ✓</span> : null}
                      </b>
                      <span>{r.sub}</span>
                    </span>
                    {r.key ? <kbd aria-hidden="true">{r.key}</kbd> : null}
                  </button>
                </div>
                {lastCamera && p.leftOut ? <LeftOutLine words={p.leftOut} /> : null}
              </Fragment>
            )
          })}
          <div className="lv-vmfoot">These are still pictures. Pick a view to see the scene from another side.</div>
        </div>
      ) : null}
    </>
  )
}
