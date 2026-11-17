// The scene's hero: the picture on its dark stage. The stage carries the
// look's dark palette (so everything on it reads in the light looks too),
// and one view or two (ScenePane: the still, its overlay, its picker and
// compass). On the stage's corners (18 px, 24 px between the chips and the
// key hint): the older-picture mark top-right, the state chips, the key
// hint, the two-views switch and ⓘ along the foot. The card says what the
// picture can't show. Keys on the picture: Home Scene, 0 the file's main
// camera, . frames the selection, Esc closes the details or the menu, else
// goes back. S5: the picture shows whom you follow's view (choosing a view
// yourself looks away), the chips by the picker follow whoever looks
// through another view, the finished line when following ends, the say
// row points at the view on screen, and the turntable strip.

import { useCallback, useEffect, useMemo, useRef, useSyncExternalStore } from 'react'
import { cx } from '@/components/ui'
import { nameOf } from '@/lib/live-view/scene/fold'
import { othersOnViews, paneRects, sayAtOf } from '@/lib/live-view/scene/follow-view'
import { localStore, type SceneLocal } from '@/lib/live-view/scene/local'
import { agentView, framedOf, pictureContext, pictureDetails, pictureStatus, viewOnScreen } from '@/lib/live-view/scene/picture-model'
import { STILL_WIDTH, coverBox, letterbox, type PictureSource, type StillRequest } from '@/lib/live-view/scene/picture-source'
import { pictureWords, type CardAction } from '@/lib/live-view/scene/picture-words'
import type { SceneControl } from '@/lib/live-view/scene/sample-source'
import { fromMockView } from '@/lib/live-view/scene/sample-views'
import { agentId, type SimState } from '@/lib/live-view/scene/sim'
import { SCENE_VIEW, parseView, turntableView } from '@/lib/live-view/scene/view-address'
import { goView, isTurntable, mainCamera, objectView, sameView, stillView, viewKey, viewName, viewRows, type ViewRow } from '@/lib/live-view/scene/views'
import type { Doc } from '@/lib/live-view/scene/types'
import type { LivePerson } from '@/lib/live-view/shared/people'
import { EdgeStack, type Edge } from '../shell/EdgeStack'
import { OkGlyph } from '../shell/glyphs'
import type { ShellHeroProps } from '../shell/shell-config'
import { DetailsPopover, InfoButton } from './DetailsPopover'
import { FinishedLine, PeerChips, TurntableStrip, TwoViewsButton } from './FollowChrome'
import { FOLLOW_CSS } from './followCss'
import { OVERLAY_CSS } from './overlayCss'
import { KeyHint, OlderMark, PictureCard, PictureChips } from './PictureChrome'
import { ScenePane } from './ScenePane'
import { STAGE_CSS } from './stageCss'
import { usePaneSize, useStageScope, useStill } from './useStage'
import { usePictureFeed } from './usePictureFeed'
import { DocsPreviewPictureSource } from '@/lib/live-view/scene/docs-preview-source'
import { camerasLeftOutWords } from '@/lib/live-view/scene/left-out'
import { useSceneFollow } from './useSceneFollow'
import { ViewPicker } from './ViewPicker'

const EDGE_PLACE = {
  left: 'left-3 top-1/2 -translate-y-1/2',
  right: 'right-3 top-1/2 -translate-y-1/2',
  top: 'top-3 left-1/2 -translate-x-1/2',
  bottom: 'bottom-3 left-1/2 -translate-x-1/2',
} as const

export interface SceneStageProps {
  S: SimState
  control: SceneControl
  source: PictureSource | null
  narrow: boolean
  /** The pane's size, when fixed (tests); measured otherwise. */
  size?: { width: number; height: number }
  offscreen: { side: Edge; people: LivePerson[] }[]
  onJump: (side: Edge) => void
  /** Whom you follow (the shell's) and its dispatch. */
  follow?: ShellHeroProps['follow']
  /** The say row points at the view on screen. */
  setSayAt?: ShellHeroProps['setSayAt']
  /** Who's here, for names. */
  people?: readonly LivePerson[]
  /** A refusal, drawn by its object in the first pane (the shell's words and actions). */
  refusal?: ShellHeroProps['refusal']
  /** Ask someone (the shell's say row): the empty scene's Ask Atlas. */
  ask?: ShellHeroProps['ask']
  /** Moves when a mesh in the scene changed (a live source): the picture asks for new stills. */
  meshEpoch?: number
}

const onlyOneOf = (s: { split: string } | null): boolean => !s || s.split === 'one'

const reducedMotion = (): boolean => typeof matchMedia !== 'undefined' && matchMedia('(prefers-reduced-motion: reduce)').matches

/** Open the side panel (if it's closed) and put focus on the outliner. */
function showList(from: HTMLElement | null): void {
  const tab = from?.closest('[data-testid="lv-tab"]')
  if (!tab) return
  if (!tab.querySelector('[data-testid="lv-panel"]')) tab.querySelector<HTMLButtonElement>('[data-testid="lv-panel-toggle"]')?.click()
  requestAnimationFrame(() => tab.querySelector<HTMLElement>('[role="tree"]')?.focus())
}

export function SceneStage({ S, control, source, narrow, size, offscreen, onJump, follow, setSayAt, people = [], refusal = null, ask, meshEpoch = 0 }: SceneStageProps): React.JSX.Element {
  const store = localStore(S)
  const L = useSyncExternalStore(store.subscribe, store.get, store.get)
  const set = (patch: Partial<SceneLocal>): void => store.set((s) => ({ ...s, ...patch }))
  const ref = useRef<HTMLDivElement>(null)
  const pane = usePaneSize(ref, size)
  const { scope, style } = useStageScope()

  const own = viewOnScreen(S, L)
  const fl = useSceneFollow({ follow, snap: { people } as ShellHeroProps['snap'] }, S, own, (v) => set({ view: v }))
  const view = fl.theirView ?? own
  const framed = framedOf(S, L.pick)
  const feed = usePictureFeed(source, view, meshEpoch)
  // A live source tells everyone else which view you look through (Follow reads it).
  useEffect(() => control.viewChanged?.(view), [control, view])
  // The real picture says its own state; the sample's is the snapshot's.
  const status = feed.status ?? pictureStatus(S)
  const retryPicture = feed.status ? feed.retry : control.retryPicture
  // The cameras the real picture can't show (the listing's; a new one moves the epoch).
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const leftOut = useMemo(() => (source instanceof DocsPreviewPictureSource ? camerasLeftOutWords(source.camerasLeftOut(), source.camerasLeftOutMore()) : null), [source, feed.epoch])
  const ctx = pictureContext(S, L.pick)
  // Ask is offered only where the shell can open the say row.
  const words = pictureWords(status, { ...ctx, askAgent: ask ? ctx.askAgent : null })
  const layout = L.layout ?? (S.layout === 2 ? 2 : 1)
  const split = pane ? paneRects(pane.width, pane.height, layout) : null
  const ag = agentView(S)
  const view2 = L.view2 ?? ag?.view ?? SCENE_VIEW
  const reqOf = (v: string, i: number): StillRequest | null => {
    const r = split?.panes[i]
    // The real picture is asked before it is shown (its answer is what says it's ready).
    const rev = feed.status ? (status.shown ?? status.want) : status.shown
    return (words.still || feed.status) && r && rev != null ? { rev, view: v, shape: (onlyOneOf(split) ? letterbox(r.w, r.h) : coverBox(r.w, r.h)).shape, width: STILL_WIDTH, ids: true } : null
  }
  const still = useStill(source, reqOf(view, 0), feed.epoch)
  const still2 = useStill(source, layout === 2 ? reqOf(view2, 1) : null, feed.epoch)
  const ground = words.ground && S.D ? S.D : null
  const name = S.D ? viewName(S.D, view) : 'Scene'
  const rows = useMemo(() => (S.D ? viewRows(S.D, { framed, agent: ag ? { name: ag.name, view: ag.view } : null }) : []), [S.D, framed, ag?.name, ag?.view])
  const menu = L.menu ?? S.menu
  const details = L.info && S.D ? pictureDetails(S, { view, pick: L.pick, size: still ? { width: still.width, height: still.height } : null, bounds: source?.bounds ? (D, a) => source.bounds?.(D, a) ?? null : undefined, meta: still?.meta ?? null }) : null

  // The quiet line: your view change, or the scenario's.
  const flash = L.flash ?? (S.toast?.text?.startsWith('Now looking through') ? S.toast.text : null)
  useEffect(() => {
    if (!L.flash) return
    const t = setTimeout(() => store.set((s) => ({ ...s, flash: null })), 2500)
    return () => clearTimeout(t)
  }, [L.flash, store])

  // The say row points at the view on screen.
  useEffect(() => {
    if (setSayAt && S.D) setSayAt(sayAtOf(S.D, view))
  }, [setSayAt, S.D, view])
  useEffect(() => () => setSayAt?.(null), [setSayAt])

  /** You choose a view: following looks away. */
  const go = (v: string, say?: string): void => {
    fl.lookAway()
    const nav = goView(L, view, v)
    set({ view: nav.view, back: nav.back, menu: false, flash: say ?? null })
  }
  const pick = (r: ViewRow): void => {
    if (r.v === '@agent') {
      // "Atlas's view" follows the agent.
      if (ag) fl.follow(ag.id)
      set({ menu: false, flash: `Now following ${ag?.name ?? 'the agent'}’s view` })
    } else go(r.v, `Now looking through ${r.kind === 'object' ? 'the framed object' : r.name}`)
    ref.current?.focus()
  }
  const pick2 = (r: ViewRow): void => set({ view2: r.v === '@agent' ? null : r.v, menu2: false })
  const onCard = (id: CardAction): void => {
    if (id === 'retry') retryPicture()
    else if (id === 'try-camera') {
      const cam = S.D ? mainCamera(S.D) : null
      if (cam) go(cam)
      retryPicture()
    } else if (id === 'show-list') showList(ref.current)
    else if (id === 'ask' && ask) {
      const ag = agentId(S)
      if (ag) ask(ag, { label: 'the scene', title: 'the whole scene', address: SCENE_VIEW })
    }
  }
  const onKeyDown = (e: React.KeyboardEvent): void => {
    // Keys typed in a field, or moving in a menu or the turntable (they handle their own), aren't the picture's.
    if (e.target instanceof HTMLElement && e.target.closest('input, textarea, select, .lv-vmenu, .lv-tt')) return
    if (e.key === 'Escape' && L.info) set({ info: false })
    else if (e.key === 'Escape' && menu) set({ menu: false })
    else {
      const nav = S.D && !S.loading ? viewKey(L, e.key, { cur: view, camera: mainCamera(S.D), framed }) : null
      if (!nav) return
      fl.lookAway()
      set({ view: nav.view, back: nav.back })
    }
    e.preventDefault()
    e.stopPropagation()
  }

  // Something off the picture: frame it.
  const frameObject = (a: string): void => {
    if (S.D) go(objectView(S.D, a), `Now looking at ${nameOf(S.D, a)}, framed`)
  }
  // eslint-disable-next-line react-hooks/exhaustive-deps -- a new listing (the epoch) brings the real bounds
  const bounds = useCallback((D: Doc, a: string) => source?.bounds?.(D, a) ?? null, [source, feed.epoch])

  const ready = !!S.D && !S.loading
  const keys = !narrow && !isTurntable(view) && ready
  const others = ready ? othersOnViews(S, view) : []
  const tt = parseView(view)
  const frame = tt?.kind === 'turntable' ? tt.frame : 0
  const motion = !reducedMotion()
  const onlyOne = layout === 1 || !split
  // The labels are placed again when the stage's own chrome changes, and wait while a menu or the details lie over them.
  const chromeKey = `${fl.ended ? 'fin' : ''}|${flash ?? ''}|${isTurntable(view) ? 'tt' : ''}|${others.length}|${words.chips.length}|${layout}`
  const shared = { S, pick: L.pick, dim: words.dim, ground, bounds: source?.bounds ? bounds : null, overlay: words.still, motion, onFrame: frameObject, size: pane ?? { width: 0, height: 0 }, chromeKey }
  return (
    <div className="lv-scene-hero" data-testid="lv-hero" data-kind="scene">
      <style>{STAGE_CSS + OVERLAY_CSS + FOLLOW_CSS}</style>
      <div
        ref={ref}
        data-testid="lv-scene-stage"
        {...(scope ?? {})}
        style={style}
        data-k2-chamfer="surface"
        data-state={S.conn !== 'ok' ? 'lost' : status.state}
        data-source={source?.kind}
        data-view={view}
        data-following={fl.whose ?? undefined}
        data-layout={split?.split ?? 'one'}
        tabIndex={0}
        role="group"
        aria-label={`The scene’s picture: ${name}. Keys: Home Scene, 0 Camera, . Frame, Esc Back`}
        className={cx('lv-sstage')}
        onKeyDown={onKeyDown}
      >
        {pane && split ? (
          <ScenePane
            {...shared}
            index={0}
            quiet={menu || L.info}
            refusal={refusal}
            rect={onlyOne ? null : split.panes[0]}
            still={still}
            name={name}
            top={
              ready ? (
                <div className="lv-vtop">
                  <ViewPicker
                    name={name}
                    view={view}
                    rows={rows}
                    checked={(r) => (r.v === '@agent' ? !!fl.whose && fl.whose === ag?.id : !fl.whose && sameView(r.v, view))}
                    open={menu}
                    kbd={S.menu && L.menu === null ? fromMockView(S.D, S.ui.menuCur, S.ui.tt ?? 0) : null}
                    onOpen={(open) => set({ menu: open })}
                    onPick={pick}
                    source={source}
                    rev={status.shown}
                    thumbView={(r) => (r.disabled ? null : r.v === '@agent' ? (ag ? stillView(ag.view, 1) : null) : stillView(r.v, 1))}
                    leftOut={leftOut}
                  />
                  {!narrow ? <PeerChips others={others} onFollow={fl.follow} /> : null}
                </div>
              ) : null
            }
          >
            {ready && isTurntable(view) ? <TurntableStrip frame={frame} source={source} rev={status.shown} onFrame={(k) => { fl.lookAway(); set({ view: turntableView(k) }) }} /> : null}
          </ScenePane>
        ) : null}
        {pane && split && !onlyOne && ready ? (
          <ScenePane
            {...shared}
            index={1}
            quiet={L.menu2 || L.info}
            rect={split.panes[1]}
            still={still2}
            name={viewName(S.D, view2)}
            top={
              <div className="lv-vtop">
                <ViewPicker
                  name={viewName(S.D, view2)}
                  label={L.view2 == null && ag ? `${ag.name}’s view` : undefined}
                  prefix="Second view"
                  view={view2}
                  rows={rows}
                  checked={(r) => (r.v === '@agent' ? L.view2 == null : L.view2 != null && sameView(r.v, view2))}
                  open={L.menu2}
                  kbd={null}
                  onOpen={(open) => set({ menu2: open })}
                  onPick={pick2}
                  source={source}
                  rev={status.shown}
                  thumbView={(r) => (r.disabled ? null : r.v === '@agent' ? (ag ? stillView(ag.view, 1) : null) : stillView(r.v, 1))}
                />
                {L.view2 == null && ag ? <span className="lv-plabel lv-plate" data-k2-chamfer="chip">{viewName(S.D, view2)}</span> : null}
              </div>
            }
          />
        ) : null}
        {words.card ? <PictureCard card={words.card} onAction={onCard} /> : null}
        {words.older ? <OlderMark text={words.older} /> : null}
        <div className="lv-vfoot">
          <PictureChips chips={words.chips} onRetry={() => retryPicture()} />
          <span className="lv-sp" />
          {keys ? <KeyHint /> : null}
          {ready ? <TwoViewsButton two={layout === 2} stacked={split?.split === 'stacked' || (!!pane && paneRects(pane.width, pane.height, 2).split === 'stacked')} onToggle={() => set({ layout: layout === 2 ? 1 : 2 })} /> : null}
          {ready ? <InfoButton open={L.info} onToggle={() => set({ info: !L.info })} /> : null}
        </div>
        {details ? <DetailsPopover details={details} narrow={narrow} /> : null}
        {fl.ended ? (
          <FinishedLine
            words={fl.ended}
            onBack={() => {
              const h = fl.backHome()
              set({ view: h ?? own })
            }}
            onAgain={fl.followAgain}
          />
        ) : flash ? (
          <div className="lv-tslot">
            <div className="lv-toast lv-plate" data-k2-chamfer="chip" role="status" aria-label="View changed">
              <OkGlyph size={14} />
              <span>{flash}</span>
            </div>
          </div>
        ) : null}
        {offscreen.map((o) => (
          <EdgeStack key={o.side} side={o.side} people={o.people} onJump={onJump} className={cx('absolute z-10', EDGE_PLACE[o.side])} />
        ))}
      </div>
    </div>
  )
}
