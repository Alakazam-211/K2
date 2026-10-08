// prd-zen-user-widgets-v2 UW25, UW32, UW38, UWB9 — a custom (agent-written)
// widget's box on a Garden.
//
// No permissions (Rosson 2026-10-08: "If the widget exists, it should be
// able to interact with agents"): a widget in your own Garden runs at
// once, with every Garden-safe cap it asks for. There is no review card,
// grant dialog, scope picker or Sending switch.
//
// K2 draws everything here except the frame's own content. In order:
//   1. a widget that isn't from your own Garden (v4's imported widgets,
//      not built): a card, never the frame (`zenWidgetMayRun`, the seam);
//   2. the paused card: K2 restarted with widgets running (UW32);
//   3. the stopped / broken / older-daemon cards;
//   4. otherwise the sealed frame (`ZenCustomFrame`). While the runaway
//      guard has posting paused, a small strip above it says "Paused: too
//      many posts. [Resume]"; the frame keeps running. A widget an agent
//      wrote has K2's ⋯ menu (Reload) at the box's top right, outside the
//      frame; K2's own built-ins (the Diary) draw edge to edge with none.

import { useEffect, useState } from 'react'
import { useAnchoredMenu } from '@/hooks/useAnchoredMenu'
import { zenWidgetMayRun } from '@/lib/zen/zen-custom-types'
import { resumeZenWidget, zenWidgetError } from '@/lib/zen/zen-widget-routes'
import { zenWidgetDisplayName } from '@/lib/zen/zen-custom-words'
import {
  reloadZenWidget,
  resumeZenWidgetPosting,
  runZenWidgetsNow,
  useZenCustomRunStore,
  useZenWidgetsGate,
  zenPlacementKey,
  zenWidgetStopText,
  ZEN_WIDGET_PAUSED_POSTING_TEXT,
  ZEN_WIDGETS_PAUSED_TEXT,
} from '@/lib/zen/zen-custom-run'
import type { ZenWidgetProps } from '../zen-registry'
import { zenButtonStyle } from './ZenOverlay'
import { ZenCustomFrame, type ZenCustomLoadProblem } from './ZenCustomFrame'

function Card({
  testId,
  children,
}: {
  testId: string
  children: React.ReactNode
}): React.JSX.Element {
  return (
    <div
      data-zen-custom-card={testId}
      className="flex h-full w-full flex-col items-center justify-center text-center"
      style={{ padding: 16, gap: 10, minHeight: 0, flex: '1 1 0%', color: 'var(--zen-text)' }}
    >
      {children}
    </div>
  )
}

function CardButton({
  onClick,
  primary,
  children,
  testId,
}: {
  onClick(): void
  primary?: boolean
  children: React.ReactNode
  testId: string
}): React.JSX.Element {
  return (
    <button
      type="button"
      data-zen-custom-action={testId}
      onClick={onClick}
      className="cursor-pointer"
      style={zenButtonStyle(primary === true)}
    >
      {children}
    </button>
  )
}

/** "Paused: too many posts. [Resume]" (R6): small, above the running frame. */
function PausedStrip({ onResume, busy, error }: { onResume(): void; busy: boolean; error: string | null }): React.JSX.Element {
  return (
    <div
      data-zen-custom-strip="paused"
      role="status"
      className="flex items-center"
      style={{ gap: 8, padding: '6px 10px', fontSize: '0.85em', borderBottom: '1px solid var(--zen-border)', color: 'var(--zen-text-muted)' }}
    >
      <span className="min-w-0 flex-1 truncate">{error ?? ZEN_WIDGET_PAUSED_POSTING_TEXT}</span>
      <button
        type="button"
        data-zen-custom-action="resume"
        disabled={busy}
        onClick={onResume}
        className="cursor-pointer disabled:cursor-default"
        style={{ ...zenButtonStyle(false), padding: '2px 10px' }}
      >
        Resume
      </button>
    </div>
  )
}

/** K2's ⋯ menu at the box's top right, outside the frame (UW25): Reload. */
function CornerMenu({ name, onReload }: { name: string; onReload(): void }): React.JSX.Element {
  const [open, setOpen] = useState(false)
  const menu = useAnchoredMenu<HTMLButtonElement>({ open, onClose: () => setOpen(false), gap: 4, width: 'min', minWidth: 160, align: 'end' })
  return (
    <>
      <button
        ref={menu.anchorRef}
        type="button"
        aria-label={`${zenWidgetDisplayName(name)} menu`}
        aria-haspopup="menu"
        aria-expanded={open}
        data-zen-custom-menu=""
        data-zen-soft-button=""
        onClick={() => setOpen((v) => !v)}
        className="no-drag cursor-pointer"
        style={{
          position: 'absolute',
          top: 6,
          right: 6,
          zIndex: 2,
          width: 26,
          height: 22,
          lineHeight: '18px',
          borderRadius: 999,
          border: '1px solid var(--zen-border)',
          background: 'var(--zen-surface-raised)',
          color: 'var(--zen-text-muted)',
        }}
      >
        ⋯
      </button>
      {open &&
        menu.portal(
          <div
            ref={menu.menuRef}
            role="menu"
            data-zen-custom-menu-list=""
            data-placement={menu.placement}
            className="no-drag flex flex-col"
            style={{
              ...menu.style,
              padding: 4,
              gap: 2,
              background: 'var(--zen-surface-raised)',
              border: '1px solid var(--zen-border)',
              borderRadius: 'var(--zen-radius)',
              boxShadow: '0 10px 30px rgba(0, 0, 0, 0.14)',
            }}
          >
            <button
              type="button"
              role="menuitem"
              data-zen-custom-menu-item="reload"
              onClick={() => {
                setOpen(false)
                onReload()
              }}
              className="flex w-full items-center text-left cursor-pointer"
              style={{ padding: '6px 10px', borderRadius: 'calc(var(--zen-radius) - 4px)', color: 'var(--zen-text)' }}
            >
              Reload
            </button>
          </div>,
        )}
    </>
  )
}

export function ZenCustomWidget({ decl, bridge }: ZenWidgetProps): React.JSX.Element {
  const widget = decl.custom
  const gardenId = bridge.call('gardens.current') as { id: string } | null
  const gid = gardenId?.id ?? ''
  const key = zenPlacementKey(gid, decl.id)
  const stop = useZenCustomRunStore((s) => s.stopped[key] ?? null)
  const generation = useZenCustomRunStore((s) => s.generation[key] ?? 0)
  const pausedHere = useZenCustomRunStore((s) => s.postingPaused[key] === true)
  const gate = useZenWidgetsGate(gid)
  const [problem, setProblem] = useState<ZenCustomLoadProblem | null>(null)
  const [busy, setBusy] = useState(false)
  const [resumeError, setResumeError] = useState<string | null>(null)
  // A new version of the widget may load fine: forget the last problem.
  useEffect(() => setProblem(null), [widget?.hash, generation])

  if (!widget) {
    return (
      <Card testId="unreadable">
        <span>K2 can’t read this widget’s placement. Ask your agent to run k2 zen validate.</span>
      </Card>
    )
  }
  const wrap = (body: React.ReactNode): React.JSX.Element => (
    <div
      data-zen-widget="custom"
      data-zen-custom={widget.widget}
      data-zen-custom-placement={widget.id}
      className="relative flex h-full w-full flex-col"
      style={{ minHeight: 0, minWidth: 0, flex: '1 1 0%' }}
    >
      {body}
    </div>
  )

  const brokenCard = (): React.JSX.Element => {
    const first = widget.errors[0]?.message ?? problem?.message ?? 'it has no working version yet'
    return wrap(
      <Card testId="broken">
        <span>This widget has errors: {first.replace(/[.\s]+$/, '')}. Ask your agent to fix it.</span>
      </Card>,
    )
  }
  if (widget.state === 'broken' && !widget.hash) return brokenCard()

  // 1. Not from your own Garden (v4 seam): never runs here.
  if (!zenWidgetMayRun(widget)) {
    return wrap(
      <Card testId="not-local">
        <span>This widget came from somewhere else. K2 can’t run shared widgets yet.</span>
      </Card>,
    )
  }

  // 2. Paused after a freeze (UW32).
  if (gate === 'paused') {
    return wrap(
      <Card testId="paused">
        <span>{ZEN_WIDGETS_PAUSED_TEXT}</span>
        <CardButton testId="run-them" primary onClick={runZenWidgetsNow}>
          Run them
        </CardButton>
      </Card>,
    )
  }

  // 3. Stopped, broken, older daemon.
  if (stop) {
    return wrap(
      <Card testId={`stopped-${stop.reason}`}>
        <span>{zenWidgetStopText(stop.reason)}</span>
        <CardButton testId="reload" primary onClick={() => reloadZenWidget(key)}>
          Reload
        </CardButton>
      </Card>,
    )
  }
  if (widget.state === 'broken' || problem?.code === 'widget_broken') return brokenCard()
  if (problem) {
    return wrap(
      <Card testId={problem.code === 'older_daemon' ? 'older-daemon' : 'load-failed'}>
        <span>{problem.message}</span>
        {problem.code !== 'older_daemon' && (
          <CardButton testId="reload" primary onClick={() => reloadZenWidget(key)}>
            Reload
          </CardButton>
        )}
      </Card>,
    )
  }

  // The window's boot decision (UW32) isn't in yet: nothing mounts.
  if (gate === 'waiting') return wrap(<div data-zen-custom-waiting="" className="flex-1" />)

  // 4. The frame (and, while the guard has posting paused, the strip).
  const resume = (): void => {
    setBusy(true)
    setResumeError(null)
    void resumeZenWidget({ garden: gid, placement: widget.id })
      .then(() => {
        setBusy(false)
        resumeZenWidgetPosting(key)
      })
      .catch((err: unknown) => {
        setBusy(false)
        setResumeError(zenWidgetError(err).message)
      })
  }
  const builtin = widget.widget.startsWith('k2:')
  return wrap(
    <>
      {(widget.paused !== null || pausedHere) && <PausedStrip onResume={resume} busy={busy} error={resumeError} />}
      <div className="relative flex min-h-0 min-w-0 flex-1 flex-col">
        <ZenCustomFrame key={`${widget.hash}:${generation}`} widget={widget} gardenId={gid} bridge={bridge} onProblem={setProblem} />
        {!builtin && <CornerMenu name={widget.name} onReload={() => reloadZenWidget(key)} />}
      </div>
    </>,
  )
}
