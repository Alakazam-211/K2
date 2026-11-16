// The scene's hero: the picture on its dark stage. The stage carries the
// look's dark palette (so everything on it reads in the light looks too),
// the still letterboxed in a 16 px faded margin, and on its corners (18 px,
// 24 px between the chips and the key hint): the view picker top-left, the
// older-picture mark top-right, the compass bottom-left, the state chips
// beside it, the key hint and ⓘ bottom-right. The card says what the
// picture can't show. Keys on the picture: Home Scene, 0 the file's main
// camera, . frames the selection, Esc closes the details or the menu, else
// goes back. Overlays (S4), people on other views and two views (S5) come
// later.

import { useEffect, useMemo, useRef, useSyncExternalStore } from 'react'
import { cx } from '@/components/ui'
import { localStore, type SceneLocal } from '@/lib/live-view/scene/local'
import { agentView, framedOf, pictureContext, pictureDetails, pictureStatus, viewOnScreen } from '@/lib/live-view/scene/picture-model'
import { STILL_WIDTH, letterbox, type PictureSource } from '@/lib/live-view/scene/picture-source'
import { pictureWords, type CardAction } from '@/lib/live-view/scene/picture-words'
import type { SceneControl } from '@/lib/live-view/scene/sample-source'
import { fromMockView } from '@/lib/live-view/scene/sample-views'
import type { SimState } from '@/lib/live-view/scene/sim'
import { SCENE_VIEW } from '@/lib/live-view/scene/view-address'
import { goView, isTurntable, mainCamera, sameView, stillView, viewKey, viewName, viewRows, type ViewRow } from '@/lib/live-view/scene/views'
import type { LivePerson } from '@/lib/live-view/shared/people'
import { EdgeStack, type Edge } from '../shell/EdgeStack'
import { OkGlyph } from '../shell/glyphs'
import { Compass } from './Compass'
import { DetailsPopover, InfoButton } from './DetailsPopover'
import { KeyHint, OlderMark, PictureCard, PictureChips } from './PictureChrome'
import { ScenePicture } from './ScenePicture'
import { STAGE_CSS } from './stageCss'
import { usePaneSize, useStageScope, useStill } from './useStage'
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
}

const reducedMotion = (): boolean => typeof matchMedia !== 'undefined' && matchMedia('(prefers-reduced-motion: reduce)').matches

/** Open the side panel (if it's closed) and put focus on the outliner. */
function showList(from: HTMLElement | null): void {
  const tab = from?.closest('[data-testid="lv-tab"]')
  if (!tab) return
  if (!tab.querySelector('[data-testid="lv-panel"]')) tab.querySelector<HTMLButtonElement>('[data-testid="lv-panel-toggle"]')?.click()
  requestAnimationFrame(() => tab.querySelector<HTMLElement>('[role="tree"]')?.focus())
}

export function SceneStage({ S, control, source, narrow, size, offscreen, onJump }: SceneStageProps): React.JSX.Element {
  const store = localStore(S)
  const L = useSyncExternalStore(store.subscribe, store.get, store.get)
  const set = (patch: Partial<SceneLocal>): void => store.set((s) => ({ ...s, ...patch }))
  const ref = useRef<HTMLDivElement>(null)
  const pane = usePaneSize(ref, size)
  const { scope, style } = useStageScope()

  const view = viewOnScreen(S, L)
  const framed = framedOf(S, L.pick)
  const status = pictureStatus(S)
  const words = pictureWords(status, { ...pictureContext(S, L.pick), askAgent: null })
  const lb = pane ? letterbox(pane.width, pane.height) : null
  const tt = S.ui.tt ?? 0
  const req = words.still && lb && status.shown != null ? { rev: status.shown, view, shape: lb.shape, width: STILL_WIDTH, ids: true } : null
  const still = useStill(source, req)
  const ground = words.ground && S.D ? S.D : null
  const name = S.D ? viewName(S.D, view) : 'Scene'
  const ag = agentView(S)
  const rows = useMemo(() => (S.D ? viewRows(S.D, { framed, agent: ag ? { name: ag.name, view: ag.view } : null }) : []), [S.D, framed, ag?.name, ag?.view])
  const menu = L.menu ?? S.menu
  const details = L.info && S.D ? pictureDetails(S, { view, pick: L.pick, size: still ? { width: still.width, height: still.height } : null }) : null

  // The quiet line: your view change, or the scenario's.
  const flash = L.flash ?? (S.toast?.text?.startsWith('Now looking through') ? S.toast.text : null)
  useEffect(() => {
    if (!L.flash) return
    const t = setTimeout(() => store.set((s) => ({ ...s, flash: null })), 2500)
    return () => clearTimeout(t)
  }, [L.flash, store])

  const go = (v: string, say?: string): void => {
    const nav = goView(L, view, v)
    set({ view: nav.view, back: nav.back, menu: false, flash: say ?? null })
  }
  const pick = (r: ViewRow): void => {
    // Following the agent's view is S5's; until then it shows the view the agent is on.
    const v = r.v === '@agent' ? (ag?.view ?? SCENE_VIEW) : r.v
    go(v, `Now looking through ${r.v === '@agent' ? `${ag?.name ?? 'the agent'}’s view` : r.kind === 'object' ? 'the framed object' : r.name}`)
    ref.current?.focus()
  }
  const onCard = (id: CardAction): void => {
    if (id === 'retry') control.retryPicture()
    else if (id === 'try-camera') {
      const cam = S.D ? mainCamera(S.D) : null
      if (cam) go(cam)
      control.retryPicture()
    } else if (id === 'show-list') showList(ref.current)
  }
  const onKeyDown = (e: React.KeyboardEvent): void => {
    // Keys typed in a field, or moving in the views menu (it handles its own), aren't the picture's.
    if (e.target instanceof HTMLElement && e.target.closest('input, textarea, select, .lv-vmenu')) return
    if (e.key === 'Escape' && L.info) set({ info: false })
    else if (e.key === 'Escape' && menu) set({ menu: false })
    else {
      const nav = S.D && !S.loading ? viewKey(L, e.key, { cur: view, camera: mainCamera(S.D), framed }) : null
      if (!nav) return
      set({ view: nav.view, back: nav.back })
    }
    e.preventDefault()
    e.stopPropagation()
  }

  const ready = !!S.D && !S.loading
  const keys = !narrow && !isTurntable(view) && ready
  return (
    <div className="lv-scene-hero" data-testid="lv-hero" data-kind="scene">
      <style>{STAGE_CSS}</style>
      <div
        ref={ref}
        data-testid="lv-scene-stage"
        {...(scope ?? {})}
        style={style}
        data-k2-chamfer="surface"
        data-state={S.conn !== 'ok' ? 'lost' : status.state}
        data-view={view}
        tabIndex={0}
        role="group"
        aria-label={`The scene’s picture: ${name}. Keys: Home Scene, 0 Camera, . Frame, Esc Back`}
        className={cx('lv-sstage')}
        onKeyDown={onKeyDown}
      >
        {lb ? <ScenePicture lb={lb} still={still} label={`Clay picture of the scene, ${name}`} dim={words.dim} ground={ground} motion={!reducedMotion()} /> : null}
        {words.card ? <PictureCard card={words.card} onAction={onCard} /> : null}
        {ready ? (
          <div className="lv-vtop">
            <ViewPicker
              name={name}
              view={view}
              rows={rows}
              checked={(r) => (r.v === '@agent' ? false : sameView(r.v, view))}
              open={menu}
              kbd={S.menu && L.menu === null ? fromMockView(S.D, S.ui.menuCur, tt) : null}
              onOpen={(open) => set({ menu: open })}
              onPick={pick}
              source={source}
              rev={status.shown}
              thumbView={(r) => (r.disabled ? null : r.v === '@agent' ? (ag ? stillView(ag.view, 1) : null) : stillView(r.v, 1))}
            />
          </div>
        ) : null}
        {words.older ? <OlderMark text={words.older} /> : null}
        {still?.camera ? (
          <div className="lv-cmpslot">
            <Compass camera={still.camera} />
          </div>
        ) : null}
        <div className="lv-vfoot">
          <PictureChips chips={words.chips} onRetry={() => control.retryPicture()} />
          <span className="lv-sp" />
          {keys ? <KeyHint /> : null}
          {ready ? <InfoButton open={L.info} onToggle={() => set({ info: !L.info })} /> : null}
        </div>
        {details ? <DetailsPopover details={details} narrow={narrow} /> : null}
        {flash ? (
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
