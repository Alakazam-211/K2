// Properties: the picked object (yours, else what the agent is on), its kind
// and name, then the Transform table as the mock draws it: 22 px rows, the
// values from about half the width, X Y Z, the units in the label
// ("Location (m)", "Rotation (°)"), three decimals. Values that just changed
// are marked in the changer's colour with a "Before" row; then who holds
// it, what can't change live, and where a child sits in the scene.

import type { CSSProperties } from 'react'
import { angle, fmt, type PropsModel } from '@/lib/live-view/scene/panel'
import { actorOf } from '@/lib/live-view/scene/wording'
import { LiveAvatar } from '../shell/LiveAvatar'
import { KindGlyph, LockGlyph } from './scene-glyphs'

type Obj = Extract<PropsModel, { state: 'obj' }>
type Part = 'loc' | 'rot' | 'scale'

function Row({ m, label, vals, part, f }: { m: Obj; label: string; vals: number[]; part: Part; f: (n: number) => string }): React.JSX.Element {
  const j = m.just
  const changed = (i: number): boolean => !!j && !!j.xb && !!j.xa && j.xb[part][i] !== j.xa[part][i]
  const any = !!j && !!j.xb && [0, 1, 2].some(changed)
  return (
    <>
      <div role="row">
        <span role="rowheader">{label}</span>
        {vals.map((v, i) => (
          <span key={i} role="cell" data-changed={changed(i) ? '' : undefined}>
            {f(v)}
          </span>
        ))}
      </div>
      {any && j?.xb ? (
        <div role="row" aria-label={`${label} before`}>
          <span data-before-label="">Before</span>
          {j.xb[part].map((v, i) => (
            <span key={i} data-before="">
              {changed(i) ? f(v) : ''}
            </span>
          ))}
        </div>
      ) : null}
    </>
  )
}

export function PropertiesBody({ m, rad = false }: { m: PropsModel; rad?: boolean }): React.JSX.Element {
  if (m.state === 'loading')
    return (
      <div className="lv-skel" aria-label="Loading">
        <i />
        <i />
        <i />
      </div>
    )
  if (m.state !== 'obj') return <div className="lv-nopick">{m.text}</div>
  const n = m.nums
  const lu = m.lenUnit === 'm' ? 'Location (m)' : 'Location'
  const style = m.just ? ({ ['--lv-c' as string]: m.just.colour } as CSSProperties) : undefined
  return (
    <div style={style}>
      <div className="lv-pr1" data-mine={m.mine ? '' : undefined}>
        <span className={`lv-ti lv-k-${m.obj.kind}`}>
          <KindGlyph kind={m.obj.kind} size={15} />
        </span>
        <b className="lv-pn" data-testid="lv-scene-prop-name">
          {m.obj.name}
        </b>
      </div>
      <div className="lv-psub" id="lv-scene-transform">
        Transform
      </div>
      {n ? (
        <div role="table" aria-labelledby="lv-scene-transform"className="lv-ptab">
          <div role="row">
            <span role="columnheader" />
            <span role="columnheader">X</span>
            <span role="columnheader">Y</span>
            <span role="columnheader">Z</span>
          </div>
          <Row m={m} label={lu} vals={n.loc} part="loc" f={fmt} />
          {n.rot.angles ? (
            <Row m={m} label={rad ? 'Rotation (rad)' : 'Rotation (°)'} vals={n.rot.angles} part="rot" f={(r) => angle(r, rad)} />
          ) : n.rot.quat ? (
            <div role="row">
              <span role="rowheader">Rotation</span>
              <span data-quat="">quaternion {n.rot.quat.map(fmt).join(' ')}</span>
            </div>
          ) : null}
          <Row m={m} label="Scale" vals={n.scale} part="scale" f={fmt} />
        </div>
      ) : (
        <div className="lv-nopick">No transform.</div>
      )}
      {m.just ? (
        <div className="lv-pjust">
          <LiveAvatar person={{ name: m.just.who, kind: m.just.actor.kind === 'agent' ? 'agent' : 'person', colour: m.just.colour, away: false }} size={14} />
          <span>
            {m.just.who} {m.just.phrase}
          </span>
        </div>
      ) : null}
      {m.hold ? (
        <div className="lv-pjust" style={{ ['--lv-c' as string]: actorOf(m.hold.actor).colour } as CSSProperties}>
          <span className="lv-lk">
            <LockGlyph size={13} />
          </span>
          <span>
            Held by <b>{actorOf(m.hold.actor).name}</b>: {m.hold.label}
          </span>
        </div>
      ) : null}
      {m.limit ? <div className="lv-plim">{m.limit}</div> : null}
      {m.attached ? <div className="lv-plim">{m.attached}</div> : null}
    </div>
  )
}
