// What the live change highlight draws.
//   - ChangeOutlines: inside the drawing's SVG, in drawing coordinates, so the
//     outlines move and zoom with the drawing. Each changed entity's box,
//     padded a few pixels so the outline clears the line it surrounds, in its
//     author's presence colour with a faint tint; where a moved entity was,
//     quieter and untinted, with a faint line to where it went; approximate
//     boxes dashed; rows paired across an outside save quiet. It fades on the
//     slow motion token.
//   - ChangeNote: the one line under the viewer's header that says who
//     changed what, with Show and Undo, or Undo's refusal in its own words.

import { Undo2Icon } from 'lucide-react'
import { Button } from '@/components/ui'
import { K2_ICON_STROKE } from '@/components/icons/K2Icons'
import { CloseGlyph } from '@/components/LiveView/shell/glyphs'
import { PRESENCE_TOKEN, type PresenceSlot } from '@/lib/live-view/shared/presence-slots'
import type { ChangeMarks } from './cad-change-marks'
import type { UndoState } from './useCadChanges'

const HAIR = 'non-scaling-stroke'

/** Your own change: your accent, never a presence colour. */
export const SELF_TOKEN = 'var(--color-accent)'

export function ChangeOutlines({ m, phase, pad }: { m: ChangeMarks; phase: 'on' | 'fading'; pad: number }): React.JSX.Element {
  return (
    <g
      transform="scale(1 -1)"
      pointerEvents="none"
      data-testid="dxf-change-layer"
      data-phase={phase}
      style={{ opacity: phase === 'fading' ? 0 : 1, transition: 'opacity var(--motion-duration-slow) var(--motion-ease-standard)' }}
    >
      {m.links.map((l, i) => (
        <line
          key={`l${i}`}
          data-testid="dxf-change-link"
          x1={l.from[0]}
          y1={l.from[1]}
          x2={l.to[0]}
          y2={l.to[1]}
          stroke={l.self ? SELF_TOKEN : PRESENCE_TOKEN[l.slot]}
          strokeOpacity={0.6}
          strokeWidth={1}
          strokeDasharray="2 3"
          vectorEffect={HAIR}
        />
      ))}
      {m.marks.map((k, i) => {
        const token = k.self ? SELF_TOKEN : PRESENCE_TOKEN[k.slot]
        const soft = k.was || k.quiet
        return (
          <rect
            key={i}
            data-testid="dxf-change-mark"
            data-address={k.address}
            data-box={k.box.join(' ')}
            data-was={String(k.was)}
            data-quiet={String(k.quiet)}
            data-self={k.self ? '' : undefined}
            x={k.box[0] - pad}
            y={k.box[1] - pad}
            width={k.box[2] - k.box[0] + 2 * pad}
            height={k.box[3] - k.box[1] + 2 * pad}
            fill={token}
            fillOpacity={soft ? 0 : 0.12}
            stroke={token}
            strokeOpacity={soft ? 0.5 : 1}
            strokeWidth={soft ? 1 : 2}
            strokeDasharray={k.dashed ? '5 3' : undefined}
            vectorEffect={HAIR}
          />
        )
      })}
    </g>
  )
}

export function ChangeNote(p: {
  text: string
  /** Each author's dot: a presence slot, or `self` for you. */
  slots: readonly (PresenceSlot | 'self')[]
  undoState: UndoState
  canUndo: boolean
  canShow: boolean
  onShow: () => void
  onUndo: () => void
  onDismiss: () => void
}): React.JSX.Element {
  const refused = p.undoState.kind === 'refused' ? p.undoState.words : null
  return (
    <div
      role="status"
      aria-live="polite"
      data-testid="dxf-change-note"
      className="flex shrink-0 items-center gap-2 border-t border-[var(--color-border)] px-3 py-1 text-[11px] text-[var(--color-text-primary)]"
    >
      <span aria-hidden="true" className="flex flex-none items-center gap-0.5">
        {p.slots.map((s) => (
          <svg key={s} width="8" height="8" viewBox="0 0 8 8" data-testid="dxf-change-dot">
            <circle cx="4" cy="4" r="4" fill={s === 'self' ? SELF_TOKEN : PRESENCE_TOKEN[s]} />
          </svg>
        ))}
      </span>
      <span className="min-w-0 flex-1 truncate" data-testid="dxf-change-words">
        {refused ?? (p.undoState.kind === 'done' ? `${p.text} · Undone` : p.text)}
      </span>
      {p.canShow ? (
        <Button size="sm" variant="quiet" className="!px-2 !py-0.5" onClick={p.onShow} data-testid="dxf-change-show">
          Show
        </Button>
      ) : null}
      {p.canUndo && p.undoState.kind !== 'refused' && p.undoState.kind !== 'done' ? (
        <Button
          size="sm"
          variant="quiet"
          className="inline-flex items-center gap-1 !px-2 !py-0.5"
          onClick={p.onUndo}
          disabled={p.undoState.kind === 'busy'}
          data-testid="dxf-change-undo"
        >
          <Undo2Icon size={12} strokeWidth={K2_ICON_STROKE} aria-hidden="true" focusable="false" />
          Undo
        </Button>
      ) : null}
      <button
        type="button"
        data-k2-focus=""
        aria-label="Dismiss"
        title="Dismiss"
        onClick={p.onDismiss}
        data-testid="dxf-change-dismiss"
        className="lv-icon-btn inline-flex h-[22px] w-[22px] flex-none cursor-pointer items-center justify-center rounded-[var(--radius-field)] text-[var(--color-text-secondary)]"
      >
        <CloseGlyph size={12} />
      </button>
    </div>
  )
}
