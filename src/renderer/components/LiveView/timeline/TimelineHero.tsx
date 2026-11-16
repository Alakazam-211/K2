// The timeline tab's content (T4), laid out as the mock: an upper row with
// the monitor, the transport under it (Back to X, loop, zoom and fit, and
// View live on its right) and the say row, beside the side panel; then the
// lanes and ruler, full width under all of it. The lanes take what they
// need up to 34% of the area, so the monitor stays the biggest thing. The
// watching keys work anywhere in the tab but the say field and the menus.
// Without a timeline beside the snapshot (the shell's own sample) it stays
// the Phase 1 placeholder.

import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { Button } from '@/components/ui'
import { personById } from '@/lib/live-view/shared/people'
import { KEY_MAP, keysHint, playbackKey, type WatchKey } from '@/lib/live-view/timeline/keys'
import { DEFAULT_SEQUENCE, finishedWords, sayAtOf, scriptedLoop } from '@/lib/live-view/timeline/follow-play'
import { stageCard } from '@/lib/live-view/timeline/stage-card'
import { actorOf } from '@/lib/live-view/timeline/actors'
import { agentOf } from '@/lib/live-view/timeline/snapshot'
import { bracketsOf, type Bracket, defaultPpf, fitPpf, isBig, lanesHeight, loadPrefs, savePrefs, zoomStep, type ViewPrefs } from '@/lib/live-view/timeline/lanes-view'
import { detailRows } from '@/lib/live-view/timeline/monitor-details'
import { monitorView, type LocalPlay } from '@/lib/live-view/timeline/monitor-view'
import { loopRange } from '@/lib/live-view/timeline/picture'
import { RULER_H } from '@/lib/live-view/timeline/ruler'
import { monitorSourceFor } from '@/lib/live-view/timeline/sample-footage'
import type { TimelineSnapshot, TimelineView } from '@/lib/live-view/timeline/snapshot'
import { editStarts, scrubMarks, whoLine } from '@/lib/live-view/timeline/transport'
import { ATLAS, FACTS } from '@/lib/live-view/timeline/world'
import type { ShellHeroProps } from '../shell/shell-config'
import { StagePlaceholder } from '../shell/StagePlaceholder'
import { box } from './box'
import { FitGlyph, ZoomInGlyph, ZoomOutGlyph } from './glyphs'
import { Lanes } from './Lanes'
import { MONITOR_CSS } from './monitorCss'
import { Monitor } from './Monitor'
import { TIMELINE_CSS } from './timelineCss'
import { Transport } from './Transport'
import { usePlayback } from './usePlayback'
import { useTimelineFollow } from './useTimelineFollow'
import { ViewMenu } from './ViewMenu'

/** The monitor row takes at least 66% of the content area (the mock's 62% of a tab without the sample banner; with a notice showing too, 62% let the lanes outgrow the picture at 1440). */
export const MONITOR_SHARE = 0.66

function Placeholder({ p }: { p: ShellHeroProps }): React.JSX.Element {
  const line = p.state === 'loading' ? 'Opening the edit. The lanes come first, then the pictures.' : 'The edit’s monitor and lanes show here: everyone’s playhead, and each change as it lands.'
  const below = p.back ? (
    <div data-testid="lv-transport-place" className="flex flex-none items-center justify-center">
      {p.back}
    </div>
  ) : null
  return (
    <div className="relative flex min-h-0 flex-1">
      <div className="flex min-w-0 flex-1 flex-col">
        <StagePlaceholder kind="timeline" line={line} p={p} below={below} />
        {p.say}
      </div>
      {p.panel}
    </div>
  )
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

/** The agent a card offers to ask, even before the file is live (no one listed yet; the sample's is Atlas). */
const cardAgent = (view: TimelineView): string => {
  const a = agentOf(view.state) ?? (view.sample ? ATLAS : null)
  return a ? actorOf(a).name : 'the agent'
}

/** Keys inside these are theirs: typing, menus, dialogs. */
const OWN_KEYS = 'input, textarea, select, [role=dialog], [contenteditable=true]'

function TimelineContent({ p, view }: { p: ShellHeroProps; view: TimelineView }): React.JSX.Element {
  const M = view.model
  const S = view.state
  const facts = view.facts ?? FACTS
  const tab = M.T.name || 'timeline'
  const [prefs, setPrefs] = useState<ViewPrefs>(() => loadPrefs(tab, M))
  // Pixels a frame: null is the edit's default for the width, 'fit' the whole edit at any width.
  const [ppf, setPpf] = useState<number | 'fit' | null>(null)
  const [fitTick, setFitTick] = useState(0)
  const tw = useRef(1000)
  const [ref, heroH] = useHeight()
  const [upper, setUpper] = useState<HTMLDivElement | null>(null)
  // A new edit (another file or sample): its own choices and zoom.
  const key = `${tab}|${view.scenario}`
  const [seen, setSeen] = useState(key)
  if (seen !== key) {
    setSeen(key)
    setPrefs(loadPrefs(tab, M))
    setPpf(null)
  }
  // A step that zooms (the sample's "fit", "zoom in" on a long edit): the lanes follow it.
  const z = S.zoom
  const cue = z && z.at === view.step ? `${key}|${view.step}` : null
  const [cued, setCued] = useState<string | null>(null)
  if (z && cue && cued !== cue) {
    setCued(cue)
    setPpf(z.fit ? 'fit' : (z.ppf ?? null))
    if (z.fit) setFitTick((n) => n + 1)
  }
  const centre = z && cue && !z.fit && z.center != null ? { f: z.center, key: cue } : null

  // The monitor: whose playhead (whom you follow, else yours), your own playback, and what it shows.
  const fps = M.T.rate ? M.T.rate.fps : 24
  const stepKey = `${key}|${view.step}`
  const agentName0 = (p.snap.agent ? personById(p.snap.people, p.snap.agent)?.name : null) ?? 'the agent'
  const scripted = scriptedLoop(S, M, agentName0)
  const [local, setLocal] = usePlayback(stepKey, M.endF, fps, scripted ? { f: scripted.f0, rate: 1, loop: scripted } : null)
  const fl = useTimelineFollow(p, S, stepKey, setLocal)
  const whose = fl.whose
  const now = fl.now
  const mv = monitorView({ S, M, facts, now, whose, local, people: p.snap.people })
  fl.noteHead(mv.head.f, mv.head.self)
  /** You take the monitor over (play, a key, a scrub, a loop): following looks away, and your playhead starts where theirs was. */
  const take = (next: LocalPlay | null): void => {
    if (!next) return
    fl.lookAway()
    setLocal(next)
  }
  const source = monitorSourceFor(!!view.sample)
  const marks = useMemo(() => scrubMarks(S, M, mv, now), [S, M, mv, now])
  const agent = p.snap.agent ? personById(p.snap.people, p.snap.agent) : undefined
  const agentName = agent?.name ?? agentName0
  const edits = useMemo(() => editStarts(S, p.snap.agent), [S, p.snap.agent])
  const latest = useMemo(() => bracketsOf(S).filter((b) => b.who === p.snap.agent).pop() ?? null, [S, p.snap.agent])
  const loop = local?.loop ? local.loop : null
  const headPerson = mv.head.self ? (p.snap.people.find((x) => x.self) ?? null) : (personById(p.snap.people, mv.head.id) ?? null)

  const onMeasure = useCallback((w: number) => {
    tw.current = w
  }, [])
  const zoom = (dir: 1 | -1): void => setPpf((v) => zoomStep(v === 'fit' ? fitPpf(M, tw.current) : (v ?? defaultPpf(M, tw.current)), dir, M, tw.current))
  const fit = (): void => {
    setPpf('fit')
    setFitTick((n) => n + 1)
  }
  const play = (): void => {
    take(playbackKey('play', local, { f: mv.head.f, playing: mv.playing, canPlay: mv.canPlay, ended: mv.state === 'ended', endF: M.endF, edits }))
  }
  const onWatchKey = (k: WatchKey): boolean => {
    if (k === 'zoom+') zoom(1)
    else if (k === 'zoom-') zoom(-1)
    else if (k === 'fit') fit()
    else if (k === 'escape') {
      if (!local?.loop) return false
      setLocal({ f: mv.head.f, rate: 0 })
    } else {
      take(playbackKey(k, local, { f: mv.head.f, playing: mv.playing, canPlay: mv.canPlay, ended: mv.state === 'ended', endF: M.endF, edits }))
    }
    return true
  }
  const onKeyDown = (e: React.KeyboardEvent): void => {
    if (e.metaKey || e.ctrlKey || e.altKey || e.defaultPrevented) return
    const k = KEY_MAP[e.key]
    if (!k) return
    const t = e.target as HTMLElement
    if (t.closest?.(OWN_KEYS)) return
    if (e.key === ' ' && t.closest?.('button')) return // Space on a button presses it
    if (onWatchKey(k)) e.preventDefault()
  }
  /** Loop a change from 1 s before it to 1 s after; the same one again ends the loop. */
  const loopOf = (b: Bracket | null): void => {
    if (!b) return
    const r = loopRange(M, b)
    if (local?.loop && local.loop.f0 === r.f0 && local.loop.f1 === r.f1) return setLocal({ f: mv.head.f, rate: 0 })
    take({ f: r.f0, rate: 1, loop: { f0: r.f0, f1: r.f1, who: personById(p.snap.people, b.who)?.name ?? agentName, verb: b.verb } })
  }
  const toggleLoop = (): void => {
    if (local?.loop) return setLocal({ f: mv.head.f, rate: 0 })
    loopOf(latest)
  }
  // The say row points at what the monitor shows (its time settles while playing).
  const sayAt = sayAtOf(M, mv, view.sequence ?? DEFAULT_SEQUENCE)
  const setSayAt = p.setSayAt
  useEffect(() => {
    setSayAt?.(sayAt)
  }, [setSayAt, sayAt.address, sayAt.label, mv.playing ? 'playing' : sayAt.time.at]) // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => () => setSayAt?.(null), [setSayAt])
  const fw = fl.finished ? finishedWords(fl.finished) : null
  const finished = fw
    ? {
        text: p.narrow ? fw.short : fw.text,
        title: fw.text,
        back: p.narrow ? fw.backShort : fw.back,
        backTitle: fw.back,
        again: p.narrow ? fw.againShort : fw.again,
        onBack: () => {
          const h = fl.backHome()
          if (h != null) setLocal({ f: h, rate: 0 })
        },
        onAgain: fl.followAgain,
      }
    : null
  const choose = (group: keyof ViewPrefs, value: string): void => {
    const next = { ...prefs, [group]: value } as ViewPrefs
    setPrefs(next)
    savePrefs(tab, next)
  }

  // The lanes take what they need, up to 34% of the area; the monitor row the rest.
  const big = isBig(M)
  const need = RULER_H + lanesHeight(M, prefs.lanes) + 16 + (big ? 14 : 0)
  const upperH = Math.max(heroH * MONITOR_SHARE, heroH - need)
  const maxFrame = Math.max(48, Math.round(heroH - upperH) - RULER_H - (big ? 14 : 0) - 1)
  const details = detailRows(S, M, mv, M.T.name || 'this timeline')

  const zoomNode = (
    <span role="group" aria-label="Zoom" data-zoom="" className="inline-flex items-center gap-1">
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
  )

  return (
    <div ref={ref} data-testid="lv-hero" data-kind="timeline" data-state={p.state} data-jumped={p.jumped ?? undefined} className="lv-tl flex min-h-0 flex-1 flex-col" onKeyDown={onKeyDown}>
      <style>{TIMELINE_CSS + MONITOR_CSS}</style>
      <div ref={setUpper} data-testid="lv-upper" className="lv-tl-mon-row relative flex min-h-0 flex-1 px-2.5 pt-2.5">
        <div className="flex min-w-0 flex-1 flex-col" data-testid="lv-moncol">
          <Monitor
            view={box(mv)}
            model={box(M)}
            source={source}
            people={p.snap.people}
            details={details}
            narrow={p.narrow}
            sheetHost={upper}
            onSkip={(f) => {
              const it = monitorView({ S, M, facts, now, whose, local: { f, rate: 0 } }).picture?.it
              take({ f, rate: 0, skipped: it?.name ?? null })
            }}
            onRestart={() => take({ f: 0, rate: 1 })}
            onShowCant={fit}
            stage={stageCard(S, M, { agent: agent?.name ?? cardAgent(view), file: p.fileName || M.T.name || 'the edit' })}
            onStageAction={(id) => {
              if (id === 'ask') ref.current?.closest('[data-testid="lv-tab"]')?.querySelector<HTMLInputElement>('[data-testid="lv-say-field"]')?.focus()
              else p.onAction?.(id)
            }}
          />
          <Transport
            model={box(M)}
            view={box(mv)}
            marks={marks}
            who={whoLine(S, mv, local)}
            person={headPerson}
            narrow={p.narrow}
            agentName={agentName}
            hasEdits={edits.length > 0}
            canLoop={!!latest}
            loop={loop}
            back={p.back}
            zoom={zoomNode}
            viewMenu={<ViewMenu prefs={prefs} onChange={choose} />}
            keysHint={keysHint(agentName)}
            onKey={(k) => onWatchKey(k)}
            onPlay={play}
            onLoop={toggleLoop}
            onScrub={(f) => take({ f, rate: 0 })}
            finished={finished}
          />
          {p.say}
        </div>
        {p.panel}
      </div>
      <Lanes
        view={view}
        facts={facts}
        people={p.snap.people}
        narrow={p.narrow}
        prefs={prefs}
        ppf={ppf}
        fitTick={fitTick}
        centre={centre}
        maxFrame={maxFrame}
        onMeasure={onMeasure}
        onKey={() => {}}
        onJump={p.onJump}
        monitor={{ f: mv.head.f, colour: mv.head.self ? 'var(--color-accent)' : mv.head.colour, name: mv.head.self ? 'your playhead' : `${mv.head.name}’s playhead` }}
        follow={{ playing: mv.playing, following: !!whose, scroll: prefs.scroll }}
        onUserScroll={() => {
          if (whose) take({ f: mv.head.f, rate: 0 })
        }}
        loop={local?.loop ?? null}
        onBracket={loopOf}
      />
    </div>
  )
}

export function TimelineHero(p: ShellHeroProps): React.JSX.Element {
  const view = (p.snap as Partial<TimelineSnapshot>).timeline
  return view ? <TimelineContent p={p} view={view} /> : <Placeholder p={p} />
}
