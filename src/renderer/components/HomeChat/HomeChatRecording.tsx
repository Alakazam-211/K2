// A DM call's recording, in the DM: the
// pieces a person sees once Sew records. Presentational only: each takes
// what to show and callbacks; the data comes from `lib/home-chat/recordings`.
//
// - `HomeChatRecordingChip`: "● Recording" by the person's name in the DM's
//   header while a recording runs, so someone reading the DM without the
//   call open still sees it.
// - `HomeChatRecordingCard`: the recording's card in the conversation, in
//   the call line's family: a mic, "Recording · 12:04", "Started by Alex ·
//   audio kept 30 days", then its state (transcribing, Show transcript, or
//   why there's none). Text only: K2 has no audio playback, download or
//   delete. Not a bubble: no reactions, no reply. Only the two people in the
//   DM see it (Sew).
// - `HomeChatTranscriptPanel`: the transcript beside the DM, in the drawers'
//   width rules: speaker turns (avatar, name, time into the call, lines),
//   search with the matches marked, and Copy transcript. Never bubbles, and
//   never unread.

import { useEffect, useMemo, useRef, useState, type JSX } from 'react'
import { Button } from '@/components/ui/Button'
import { Spinner } from '@/components/ui/Spinner'
import { Surface } from '@/components/ui/Surface'
import { SearchField } from '@/components/ui/SearchField'
import { DialogCloseButton } from '@/components/ui/dialog-cancel'
import { MicIcon } from '@/components/icons/K2Icons'
import { RecordingBadge, RecordingDot } from '@/components/Voice/RecordingBadge'
import {
  transcriptCopyText,
  transcriptMatches,
  transcriptTime,
  transcriptTurns,
  type RecordingCardView,
  type TranscriptLine,
} from '@/lib/home-chat/recordings'
import { usePanelsStore } from '@/stores/panels'
import { SIDEBAR_MAX_WIDTH } from '../../../shared/constants'
import { HomeChatPersonAvatar } from './HomeChatGlyphs'

/** "● Recording" by the name in the DM's header: the badge's short form. */
export function HomeChatRecordingChip({ title }: { title: string }): JSX.Element {
  return <RecordingBadge startedAt={null} title={title} short testId="home-chat-header-recording" />
}

/** Sew's transcript failed and said nothing more. */
export const TRANSCRIPT_FAILED_LINE = "The transcript couldn't be made."

export function HomeChatRecordingCard({
  view,
  label,
  timeLabel,
  failReason = null,
  onShowTranscript,
}: {
  view: RecordingCardView
  /** The whole card in one line, for a screen reader (recordingCardLine). */
  label?: string
  timeLabel: string
  /** Why the transcript failed, when Sew says. */
  failReason?: string | null
  /** Absent: Show transcript isn't offered (no transcript yet, or none). */
  onShowTranscript?: () => void
}): JSX.Element {
  return (
    <div className="flex justify-center px-4 py-1" role="group" aria-label={label} data-testid="home-chat-recording-card" data-state={view.state}>
      <Surface role2="elevated" chamfer className="flex w-full max-w-[420px] items-start gap-2.5 px-3 py-2.5">
        <span className="mt-0.5 flex shrink-0 text-[var(--color-text-secondary)]">
          <MicIcon size={16} />
        </span>
        <div className="flex min-w-0 flex-1 flex-col gap-0.5">
          <div className="flex min-w-0 items-baseline gap-1.5">
            <span className="type-name truncate text-[var(--color-text-primary)]" data-testid="home-chat-recording-card-title">
              {view.title}
            </span>
            <span className="type-hint shrink-0 text-[var(--color-text-muted)]">· {timeLabel}</span>
          </div>
          {view.detail ? (
            <span className="type-hint text-[var(--color-text-secondary)]" data-testid="home-chat-recording-card-detail">
              {view.detail}
            </span>
          ) : null}
          <div className="mt-1.5 flex min-h-6 items-center gap-1.5" data-testid="home-chat-recording-card-state">
            {view.state === 'live' ? (
              <span className="type-hint flex items-center gap-1.5 text-[var(--color-text-secondary)]">
                <RecordingDot />
                Recording now
              </span>
            ) : view.state === 'transcribing' ? (
              <span className="type-hint flex items-center gap-1.5 text-[var(--color-text-secondary)]">
                <Spinner />
                Transcript in a minute or two
              </span>
            ) : view.state === 'failed' ? (
              <span className="type-hint text-[var(--color-text-muted)]" data-testid="home-chat-recording-card-failed">
                {failReason ? `No transcript: ${failReason}` : TRANSCRIPT_FAILED_LINE}
              </span>
            ) : onShowTranscript ? (
              <>
                <Button variant="quiet" className="min-h-6" data-testid="home-chat-show-transcript" onClick={onShowTranscript}>
                  Show transcript
                </Button>
                {view.partial ? <span className="type-hint text-[var(--color-text-muted)]">Some parts couldn’t be transcribed.</span> : null}
              </>
            ) : null}
          </div>
        </div>
      </Surface>
    </div>
  )
}

/** A speaker in the transcript: the name to show and the avatar's facts. */
export interface TranscriptPerson {
  name: string
  username: string
  picture?: string | null
}

/** The drawers' width rules: the right drawer's width, at least room for a
 *  line of speech, at most a drawer's widest. */
const TRANSCRIPT_MIN_WIDTH = 320

/** The transcript panel's width; a voice room's recordings list beside it
 *  takes the same. */
export function useTranscriptPanelWidth(): number {
  const drawerWidth = usePanelsStore((s) => s.rightPanelWidth)
  return Math.min(SIDEBAR_MAX_WIDTH, Math.max(TRANSCRIPT_MIN_WIDTH, drawerWidth))
}

export function HomeChatTranscriptPanel({
  heading,
  lines,
  personOf,
  state,
  onClose,
}: {
  /** "Transcript · 12:04 · Nov 17, 2:41 PM". */
  heading: string
  lines: TranscriptLine[] | null
  personOf: (memberId: string) => TranscriptPerson
  /** `Loading…`, an error line, or null. */
  state: string | null
  onClose: () => void
}): JSX.Element {
  const width = useTranscriptPanelWidth()
  const [query, setQuery] = useState('')
  const [copied, setCopied] = useState(false)
  const closeRef = useRef<HTMLButtonElement>(null)
  useEffect(() => closeRef.current?.focus(), [])
  useEffect(() => {
    if (!copied) return
    const t = setTimeout(() => setCopied(false), 1500)
    return () => clearTimeout(t)
  }, [copied])
  const turns = useMemo(() => transcriptTurns(lines ?? []), [lines])
  const q = query.trim().toLowerCase()
  const hits = q ? turns.reduce((n, t) => n + t.lines.reduce((m, l) => m + transcriptMatches(l.text, q).filter((p) => p.hit).length, 0), 0) : 0
  const copy = (): void => {
    const text = transcriptCopyText(heading, lines ?? [], (id) => personOf(id).name)
    void navigator.clipboard?.writeText(text).then(
      () => setCopied(true),
      () => {},
    )
  }
  return (
    <aside
      aria-label="Transcript"
      data-testid="home-chat-transcript"
      className="flex max-w-[45%] shrink-0 flex-col border-l border-[var(--color-border)] bg-[var(--color-bg)]"
      style={{ width }}
      onKeyDown={(e) => {
        if (e.key !== 'Escape' || e.defaultPrevented) return
        e.preventDefault()
        onClose()
      }}
    >
      <div className="flex items-center gap-2 border-b border-[var(--color-border)] px-3 py-2">
        <h2 className="type-name min-w-0 flex-1 truncate text-[var(--color-text-primary)]" data-testid="home-chat-transcript-heading" title={heading}>
          {heading}
        </h2>
        <DialogCloseButton ref={closeRef} className="-mr-1" data-testid="home-chat-transcript-close" onClick={onClose} />
      </div>
      {lines && lines.length > 0 ? (
        <div className="flex flex-col gap-2 border-b border-[var(--color-border)] px-3 py-2">
          <SearchField
            className="w-full"
            placeholder="Search the transcript"
            aria-label="Search the transcript"
            data-testid="home-chat-transcript-search"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
          <div className="flex items-center gap-2">
            <span className="type-hint min-w-0 flex-1 text-[var(--color-text-muted)]" data-testid="home-chat-transcript-hits" aria-live="polite">
              {q ? (hits === 0 ? 'No matches' : hits === 1 ? '1 match' : `${hits} matches`) : ''}
            </span>
            <Button variant="quiet" data-testid="home-chat-transcript-copy" onClick={copy}>
              {copied ? 'Copied' : 'Copy transcript'}
            </Button>
          </div>
        </div>
      ) : null}
      <div className="min-h-0 flex-1 overflow-y-auto px-3 py-3">
        {state ? (
          <p className="type-hint text-[var(--color-text-muted)]" data-testid="home-chat-transcript-state">
            {state}
          </p>
        ) : turns.length === 0 ? (
          <p className="type-hint text-[var(--color-text-muted)]" data-testid="home-chat-transcript-empty">
            No lines yet.
          </p>
        ) : (
          <ol className="flex flex-col gap-3">
            {turns.map((t) => {
              const who = personOf(t.speaker)
              const time = transcriptTime(t.startMs)
              return (
                <li key={t.id} className="flex gap-2.5" data-testid="home-chat-transcript-turn">
                  <HomeChatPersonAvatar username={who.username} name={who.name} picture={who.picture ?? null} size={24} />
                  <div className="min-w-0 flex-1">
                    <div className="flex items-baseline gap-1.5">
                      <span className="type-name truncate text-[var(--color-text-primary)]">{who.name}</span>
                      {time ? <span className="type-hint shrink-0 tabular-nums text-[var(--color-text-muted)]">{time}</span> : null}
                    </div>
                    {t.lines.map((l) => (
                      <p key={l.id} className="type-row whitespace-pre-wrap break-words text-[var(--color-text-primary)]" data-testid="home-chat-transcript-line">
                        {transcriptMatches(l.text, q).map((p, i) =>
                          p.hit ? (
                            <mark key={i} data-transcript-hit="" className="rounded-[var(--radius-status)] bg-[color-mix(in_srgb,var(--color-accent)_28%,transparent)] text-[var(--color-text-primary)]">
                              {p.text}
                            </mark>
                          ) : (
                            <span key={i}>{p.text}</span>
                          ),
                        )}
                      </p>
                    ))}
                  </div>
                </li>
              )
            })}
          </ol>
        )}
      </div>
    </aside>
  )
}
