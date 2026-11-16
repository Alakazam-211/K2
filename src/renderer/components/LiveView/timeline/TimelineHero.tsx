// The timeline tab's content area (T3): the monitor's place on top (its
// picture and transport come in T4), a row under it for Back to X, zoom
// and fit, and the View menu; then the lanes and ruler, which take what
// they need up to 46% of the area so the monitor stays the biggest thing.
// Without a timeline beside the snapshot (the shell's own sample) it stays
// the Phase 1 placeholder.

import { useCallback, useLayoutEffect, useRef, useState } from 'react'
import { Button } from '@/components/ui'
import { defaultPpf, fitPpf, loadPrefs, savePrefs, zoomStep, type ViewPrefs } from '@/lib/live-view/timeline/lanes-view'
import { RULER_H } from '@/lib/live-view/timeline/ruler'
import type { TimelineSnapshot, TimelineView } from '@/lib/live-view/timeline/snapshot'
import { FACTS } from '@/lib/live-view/timeline/world'
import type { ShellHeroProps } from '../shell/shell-config'
import { StagePlaceholder } from '../shell/StagePlaceholder'
import { FitGlyph, ZoomInGlyph, ZoomOutGlyph } from './glyphs'
import { Lanes } from './Lanes'
import { TIMELINE_CSS } from './timelineCss'
import { ViewMenu } from './ViewMenu'

/** The lanes take at most this share of the content area (the say row and the banner sit outside it, so a little more than the mock's 38% of the tab). */
export const LANES_SHARE = 0.46

function Placeholder({ p }: { p: ShellHeroProps }): React.JSX.Element {
  const line = p.state === 'loading' ? 'Opening the edit. The lanes come first, then the pictures.' : 'The edit’s monitor and lanes show here: everyone’s playhead, and each change as it lands.'
  const below = p.back ? (
    <div data-testid="lv-transport-place" className="flex flex-none items-center justify-center">
      {p.back}
    </div>
  ) : null
  return <StagePlaceholder kind="timeline" line={line} p={p} below={below} />
}

function useHeight(): [React.RefObject<HTMLDivElement | null>, number] {
  const ref = useRef<HTMLDivElement>(null)
  const [h, setH] = useState(640)
  useLayoutEffect(() => {
    const el = ref.current
    if (!el || typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver(() => setH(el.clientHeight || 640))
    ro.observe(el)
    return () => ro.disconnect()
  }, [])
  return [ref, h]
}

function TimelineContent({ p, view }: { p: ShellHeroProps; view: TimelineView }): React.JSX.Element {
  const M = view.model
  const tab = M.T.name || 'timeline'
  const [prefs, setPrefs] = useState<ViewPrefs>(() => loadPrefs(tab, M))
  // Pixels a frame: null is the edit's default for the width, 'fit' the whole edit at any width.
  const [ppf, setPpf] = useState<number | 'fit' | null>(null)
  const [fitTick, setFitTick] = useState(0)
  const tw = useRef(1000)
  const [ref, heroH] = useHeight()
  // A new edit (another file or sample): its own choices and zoom.
  const key = `${tab}|${view.scenario}`
  const [seen, setSeen] = useState(key)
  if (seen !== key) {
    setSeen(key)
    setPrefs(loadPrefs(tab, M))
    setPpf(null)
  }
  // A step that zooms (the sample's "fit", "zoom in" on a long edit): the lanes follow it.
  const z = view.state.zoom
  const cue = z && z.at === view.step ? `${key}|${view.step}` : null
  const [cued, setCued] = useState<string | null>(null)
  if (z && cue && cued !== cue) {
    setCued(cue)
    setPpf(z.fit ? 'fit' : (z.ppf ?? null))
    if (z.fit) setFitTick((n) => n + 1)
  }
  const centre = z && cue && !z.fit && z.center != null ? { f: z.center, key: cue } : null

  const onMeasure = useCallback((w: number) => {
    tw.current = w
  }, [])
  const zoom = (dir: 1 | -1): void => setPpf((v) => zoomStep(v === 'fit' ? fitPpf(M, tw.current) : (v ?? defaultPpf(M, tw.current)), dir, M, tw.current))
  const fit = (): void => {
    setPpf('fit')
    setFitTick((n) => n + 1)
  }
  const onKey = (e: React.KeyboardEvent): void => {
    if (e.metaKey || e.ctrlKey || e.altKey) return
    if (e.key === '=' || e.key === '+') zoom(1)
    else if (e.key === '-') zoom(-1)
    else if (e.key === '\\') fit()
    else return
    e.preventDefault()
  }
  const choose = (group: keyof ViewPrefs, value: string): void => {
    const next = { ...prefs, [group]: value } as ViewPrefs
    setPrefs(next)
    savePrefs(tab, next)
  }
  const maxFrame = Math.max(72, Math.round(heroH * LANES_SHARE) - RULER_H)
  const line = p.state === 'loading' ? 'Opening the edit. The lanes come first, then the pictures.' : 'The edit’s picture plays here.'

  return (
    <div ref={ref} data-testid="lv-hero" data-kind="timeline" data-state={p.state} data-jumped={p.jumped ?? undefined} className="lv-tl flex min-h-0 flex-1 flex-col">
      <style>{TIMELINE_CSS}</style>
      <div className="flex min-h-[72px] flex-1 flex-col px-2.5 pt-2.5">
        <div data-k2-chamfer="surface" data-testid="lv-monitor-place" className="lv-stage relative flex min-h-0 flex-1 items-center justify-center rounded-[var(--radius-box)]">
          <p className="type-row m-0 max-w-[44ch] px-6 text-center text-[var(--color-text-secondary)]">{line}</p>
        </div>
      </div>
      <div data-testid="lv-transport-place" className="flex flex-none items-center gap-2 px-2.5 py-1.5">
        {p.back}
        <span className="flex-1" />
        <span role="group" aria-label="Zoom" className="inline-flex items-center gap-1">
          <Button variant="quiet" size="sm" className="!px-1.5" aria-label="Zoom out" title="Zoom out ( - )" data-testid="lv-zoom-out" onClick={() => zoom(-1)}>
            <ZoomOutGlyph size={14} />
          </Button>
          <Button variant="quiet" size="sm" className="!px-1.5" aria-label="Zoom in" title="Zoom in ( = )" data-testid="lv-zoom-in" onClick={() => zoom(1)}>
            <ZoomInGlyph size={14} />
          </Button>
          <Button variant="quiet" size="sm" className="!px-1.5" aria-label="Fit the whole edit" title="Fit the whole edit ( \ )" data-testid="lv-zoom-fit" onClick={fit}>
            <FitGlyph size={14} />
          </Button>
        </span>
        <ViewMenu prefs={prefs} onChange={choose} />
      </div>
      <Lanes view={view} facts={FACTS} people={p.snap.people} narrow={p.narrow} prefs={prefs} ppf={ppf} fitTick={fitTick} centre={centre} maxFrame={maxFrame} onMeasure={onMeasure} onKey={onKey} onJump={p.onJump} />
    </div>
  )
}

export function TimelineHero(p: ShellHeroProps): React.JSX.Element {
  const view = (p.snap as Partial<TimelineSnapshot>).timeline
  return view ? <TimelineContent p={p} view={view} /> : <Placeholder p={p} />
}
