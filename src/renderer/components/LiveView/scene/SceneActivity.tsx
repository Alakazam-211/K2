// Activity and People and holds, the scene panel's first two sections, in
// the mock's words: a Now line per agent hold, the conversation above it
// when it's newest, then the last few changes with when; and each person
// with their view on one line and what they're doing below (their holds in
// their colour), you last. Clicking someone follows them.

import type { CSSProperties } from 'react'
import type { ActivityItem, PersonRow } from '@/lib/live-view/scene/panel'
import type { Actor } from '@/lib/live-view/scene/types'
import { LiveAvatar } from '../shell/LiveAvatar'
import { CameraGlyph, FramedGlyph, LockGlyph, RefusedGlyph, SceneViewGlyph } from './scene-glyphs'

const person = (a: Actor, self = false): { name: string; kind: 'person' | 'agent'; colour: string; away: boolean; self: boolean } => ({ name: a.name, kind: a.kind === 'agent' ? 'agent' : 'person', colour: a.colour, away: false, self })
const tint = (c: string | undefined): CSSProperties | undefined => (c ? ({ ['--lv-c' as string]: c } as CSSProperties) : undefined)

function RefChip({ name, address }: { name: string; address: string | null }): React.JSX.Element | null {
  if (!address) return null
  return (
    <span data-k2-chamfer="chip" className="lv-refchip" title={name}>
      {address.startsWith('@obj:') ? <FramedGlyph size={11} /> : address.startsWith('@cam:') ? <CameraGlyph size={11} /> : <SceneViewGlyph size={11} />}
      {name}
    </span>
  )
}

function Line({ item }: { item: ActivityItem }): React.JSX.Element {
  const cur = 'cur' in item && item.cur ? '' : undefined
  switch (item.kind) {
    case 'now':
      return (
        <div data-testid="lv-scene-activity" data-kind="now" className="lv-arow">
          <LiveAvatar person={person(item.actor)} size={16} />
          <span className="lv-at">
            <b>{item.actor.name}</b> is {item.text}
          </span>
          <span className="lv-tm">now</span>
        </div>
      )
    case 'refused':
      return (
        <div data-testid="lv-scene-activity" data-kind="refused" data-cur={cur} className="lv-arow" title={item.detail}>
          <span className="lv-lk" style={tint(item.holder?.colour)}>
            {item.holder ? <LockGlyph size={12} /> : <RefusedGlyph size={12} />}
          </span>
          <span className="lv-at">
            <b>{item.who}</b>
            {item.text}
          </span>
          <span className="lv-tm">refused</span>
        </div>
      )
    case 'said':
      return (
        <div data-testid="lv-scene-activity" data-kind="said" data-cur={cur} className="lv-arow">
          <LiveAvatar person={{ name: 'You', kind: 'person', colour: '', away: false, self: true }} size={16} />
          <span className="lv-at">
            <b>You → {item.to.name}:</b> {item.text}
          </span>
          <RefChip name={item.refName} address={item.ref} />
        </div>
      )
    case 'reply':
      return (
        <div data-testid="lv-scene-activity" data-kind="reply" data-cur={cur} className="lv-arow">
          <LiveAvatar person={person(item.actor)} size={16} />
          <span className="lv-at">
            <b>{item.actor.name}:</b> {item.text}
          </span>
          <RefChip name={item.refName} address={item.ref} />
        </div>
      )
    case 'done':
      return (
        <div data-testid="lv-scene-activity" data-kind="done" data-cur={cur} className="lv-arow">
          <LiveAvatar person={person(item.actor)} size={16} />
          <span className="lv-at">
            <b>{item.actor.name}</b> is done
          </span>
          <span className="lv-tm">{item.when}</span>
        </div>
      )
    default:
      return (
        <div data-testid="lv-scene-activity" data-kind={item.kind} data-cur={cur} className="lv-arow">
          <LiveAvatar person={person(item.actor)} size={16} />
          <span className="lv-at" title={item.kind === 'change' ? `${item.who}${item.text}` : undefined}>
            <b>{item.kind === 'let-go' ? item.actor.name : item.who}</b>
            {item.text}
          </span>
          <span className="lv-tm">{item.when}</span>
        </div>
      )
  }
}

export function ActivityLines({ items, empty }: { items: ActivityItem[]; empty: string }): React.JSX.Element {
  if (!items.length) return <div className="lv-nopick">{empty}</div>
  return (
    <div data-testid="lv-scene-activity-list">
      {items.map((it, i) => (
        <Line key={`${it.kind}-${i}`} item={it} />
      ))}
    </div>
  )
}

function PersonLine({ r, stale, onFollow }: { r: PersonRow; stale: boolean; onFollow: (id: string) => void }): React.JSX.Element {
  const inner = (
    <>
      <LiveAvatar person={{ name: r.name, kind: r.kind === 'agent' ? 'agent' : 'person', colour: r.colour, away: r.away, self: r.self }} size={18} grey={stale} />
      <span className="lv-pt">
        <span className="lv-pl1">
          <b>{r.name}</b>
          {r.following ? (
            <span data-k2-chamfer="chip" className="lv-foltag" title={`You’re following ${r.name}: your view is theirs`}>
              You’re following
            </span>
          ) : (
            <span className="lv-pv">
              {r.camera ? <CameraGlyph size={11} /> : <SceneViewGlyph size={11} />}
              {r.view}
            </span>
          )}
        </span>
        <span className="lv-pwhat">
          {r.holding ? (
            <span className="lv-lk" style={tint(r.colour)}>
              <LockGlyph size={10} />
            </span>
          ) : null}
          {r.what}
        </span>
      </span>
    </>
  )
  if (r.self || stale)
    return (
      <div data-testid={`lv-scene-person-${r.id}`} data-away={r.away ? '' : undefined} className="lv-prow">
        {inner}
      </div>
    )
  return (
    <button
      type="button"
      data-k2-focus=""
      data-testid={`lv-scene-person-${r.id}`}
      data-away={r.away ? '' : undefined}
      aria-pressed={r.following}
      aria-label={`${r.name}${r.kind === 'agent' ? ' (agent)' : ''}, looking at ${r.view}, ${r.what}${r.following ? '. You’re following them' : ''}`}
      title={r.following ? `Stop following ${r.name}` : `Follow ${r.name}: see what they see`}
      onClick={() => onFollow(r.id)}
      className="lv-prow no-drag"
    >
      {inner}
    </button>
  )
}

export function PeopleLines({ rows, stale, onFollow }: { rows: PersonRow[]; stale: boolean; onFollow: (id: string) => void }): React.JSX.Element {
  return (
    <div data-testid="lv-scene-people">
      {rows.map((r) => (
        <PersonLine key={r.id} r={r} stale={stale} onFollow={onFollow} />
      ))}
    </div>
  )
}
