// The one Follow control (lib/live-view/shared/follow.ts decides what it
// says): "Follow Atlas" filled; "Following Brook · Stop" as one control,
// tinted with Brook's colour, that always names Brook; "Back to Brook"
// filled. "Sam left the file" shows beside it for 4 s. Nothing once the
// agent is done. Where Back lives elsewhere (the timeline's transport),
// `backHere={false}` leaves the header empty while you've looked away;
// <BackButton> is the same button for that other place.

import type { CSSProperties } from 'react'
import { Button } from '@/components/ui'
import type { FollowView } from '@/lib/live-view/shared/follow'
import { FollowGlyph } from './glyphs'

export function FollowControl({
  view,
  backHere = true,
  disabled = false,
  onFollow,
  onStop,
  onBack,
}: {
  view: FollowView
  backHere?: boolean
  /** Not connected: shown, but it can't act. */
  disabled?: boolean
  onFollow: (id: string) => void
  onStop: () => void
  onBack: () => void
}): React.JSX.Element | null {
  const note = view.note ? (
    <span role="status" className="type-hint whitespace-nowrap text-[var(--color-text-secondary)]" data-testid="lv-follow-note">
      {view.note}
    </span>
  ) : null
  let control: React.ReactNode = null
  if (view.control === 'follow' && view.target) {
    const target = view.target
    control = (
      <Button variant="accent" size="md" data-variant="accent" aria-pressed={false} disabled={disabled} onClick={() => onFollow(target)} data-testid="lv-follow" className="inline-flex items-center gap-1.5">
        <FollowGlyph size={14} />
        {view.label}
      </Button>
    )
  } else if (view.control === 'following') {
    control = (
      <Button
        variant="default"
        size="md"
        data-variant="following"
        aria-pressed
        aria-label={`${view.label} · ${view.stop}`}
        title={`Stop following ${view.label.replace(/^Following /, '')}`}
        disabled={disabled}
        onClick={onStop}
        data-testid="lv-follow"
        style={{ '--lv-c': view.colour } as CSSProperties}
        className="lv-following inline-flex items-center gap-1.5 text-[var(--color-text-primary)]"
      >
        <span aria-hidden="true" className="lv-following-dot" />
        <span className="lv-follow-words">{view.label}</span>
        <span className="font-normal">
          <span className="lv-follow-words"> · </span>
          {view.stop}
        </span>
      </Button>
    )
  } else if (view.control === 'back' && backHere) {
    control = <BackButton label={view.label} disabled={disabled} onBack={onBack} />
  }
  if (!control && !note) return null
  return (
    <span className="inline-flex flex-none items-center gap-2">
      {note}
      {control}
    </span>
  )
}

export function BackButton({ label, disabled = false, onBack }: { label: string; disabled?: boolean; onBack: () => void }): React.JSX.Element {
  return (
    <Button variant="accent" size="md" data-variant="accent" disabled={disabled} onClick={onBack} data-testid="lv-back" className="inline-flex items-center gap-1.5">
      {label}
    </Button>
  )
}
