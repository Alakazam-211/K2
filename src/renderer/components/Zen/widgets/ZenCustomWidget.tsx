// prd-zen-user-widgets-v2 UW22, UW24, UW25, UW32, UW38, UWB9 — a custom
// (agent-written) widget's box on a Garden.
//
// K2 draws everything here except the frame's own content. In order
// (UW38):
//   1. the review card, while a requested cap isn't granted (the widget's
//      code doesn't load): its name as its own words, K2's sentence per cap,
//      [Not now] [Review]. Not now collapses it to a one-line strip;
//   2. the paused cards: K2 restarted with widgets running (UW32), or the
//      runaway guard paused the grant (UWB9: [Resume], owner only);
//   3. the stopped / broken / older-daemon cards;
//   4. otherwise the sealed frame (`ZenCustomFrame`) plus K2's ⋯ corner
//      menu (Reload · Sending · Permissions… · About this widget) at the
//      box's top right, outside the frame. A `partial` grant runs with a
//      strip: "It now also asks to … [Review]".
// The grant itself lives in the daemon; this component only renders it
// and sends the owner's clicks.

import { useEffect, useState } from 'react'
import { useAnchoredMenu } from '@/hooks/useAnchoredMenu'
import { useZenGardensStore } from '@/lib/zen/zen-gardens'
import { zenGrantNeedsReview, type ZenCustomWidgetPayload } from '@/lib/zen/zen-custom-types'
import { setZenWidgetSending, resumeZenWidget, zenWidgetError } from '@/lib/zen/zen-custom-grants'
import { zenScopeWhere, zenServerName } from '@/lib/zen/zen-custom-scope'
import { zenAlsoAsksText, zenCapSentence, zenWidgetDisplayName } from '@/lib/zen/zen-custom-words'
import {
  reloadZenWidget,
  runZenWidgetsNow,
  useZenCustomRunStore,
  zenPlacementKey,
  zenWidgetStopText,
  ZEN_WIDGETS_PAUSED_TEXT,
} from '@/lib/zen/zen-custom-run'
import type { ZenWidgetProps } from '../zen-registry'
import { ZenGrantDialog, ZenOverlay, zenButtonStyle } from './ZenGrantDialog'
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
  disabled,
}: {
  onClick(): void
  primary?: boolean
  children: React.ReactNode
  testId: string
  disabled?: boolean
}): React.JSX.Element {
  return (
    <button
      type="button"
      data-zen-custom-action={testId}
      disabled={disabled}
      onClick={onClick}
      className="cursor-pointer disabled:cursor-default"
      style={zenButtonStyle(primary === true)}
    >
      {children}
    </button>
  )
}

function gardenName(gardenId: string): string {
  return useZenGardensStore.getState().gardens.find((g) => g.id === gardenId)?.name ?? 'this Garden'
}

/** The review card (UW22 step 1): K2's words, the widget's name as its own. */
function ReviewCard({
  widget,
  onReview,
  onNotNow,
}: {
  widget: ZenCustomWidgetPayload
  onReview(): void
  onNotNow(): void
}): React.JSX.Element {
  const invalid = widget.grant?.state === 'invalid'
  const where = zenScopeWhere(widget.grant?.scope ?? null)
  return (
    <Card testId="review">
      <div className="flex flex-col" style={{ gap: 8, maxWidth: 420, textAlign: 'left' }}>
        <span>
          <strong data-zen-custom-name="">“{zenWidgetDisplayName(widget.name)}”</strong>{' '}
          {invalid ? 'needs your OK again: K2 couldn’t confirm its permission.' : 'wants to:'}
        </span>
        <ul className="flex flex-col" style={{ gap: 2, margin: 0, paddingLeft: 18, listStyle: 'disc', color: 'var(--zen-text-muted)' }}>
          {widget.requested.map((c) => (
            <li key={c} data-zen-custom-card-cap={c}>
              {zenCapSentence(c, widget.grant?.scope ? where : 'a Home you pick')}
            </li>
          ))}
        </ul>
        <div className="flex justify-end" style={{ gap: 8 }}>
          <CardButton testId="not-now" onClick={onNotNow}>
            Not now
          </CardButton>
          <CardButton testId="review" primary onClick={onReview}>
            Review
          </CardButton>
        </div>
      </div>
    </Card>
  )
}

function Strip({ testId, text, action, onAction }: { testId: string; text: string; action: string; onAction(): void }): React.JSX.Element {
  return (
    <div
      data-zen-custom-strip={testId}
      className="flex items-center"
      style={{ gap: 8, padding: '6px 10px', fontSize: '0.85em', borderBottom: '1px solid var(--zen-border)', color: 'var(--zen-text-muted)' }}
    >
      <span className="min-w-0 flex-1 truncate">{text}</span>
      <button type="button" data-zen-custom-action={testId} onClick={onAction} className="cursor-pointer" style={{ ...zenButtonStyle(false), padding: '2px 10px' }}>
        {action}
      </button>
    </div>
  )
}

/** About this widget (UW25, UWA8): folder, Garden, hash, grant, entries. */
function AboutDialog({ widget, gardenId, onClose }: { widget: ZenCustomWidgetPayload; gardenId: string; onClose(): void }): React.JSX.Element {
  const builtin = widget.widget.startsWith('k2:')
  const g = widget.grant
  const entries = g?.entries ?? []
  return (
    <ZenOverlay label="About this widget" onClose={onClose} testId="zen-custom-about">
      <strong>{zenWidgetDisplayName(widget.name)}</strong>
      <dl className="grid" style={{ gridTemplateColumns: 'auto 1fr', gap: '4px 12px', margin: 0, fontSize: '0.9em' }}>
        <dt style={{ color: 'var(--zen-text-muted)' }}>Folder</dt>
        <dd data-zen-about-folder="" style={{ margin: 0 }}>
          {builtin ? `Built into K2 (${widget.widget})` : `~/.k2/zen/widgets/${widget.widget}`}
        </dd>
        <dt style={{ color: 'var(--zen-text-muted)' }}>Garden</dt>
        <dd style={{ margin: 0 }}>{gardenName(gardenId)}</dd>
        <dt style={{ color: 'var(--zen-text-muted)' }}>Version</dt>
        <dd data-zen-about-hash="" style={{ margin: 0, fontFamily: 'monospace' }}>
          {widget.hash ? widget.hash.slice(0, 12) : 'none yet'}
        </dd>
        <dt style={{ color: 'var(--zen-text-muted)' }}>Allowed</dt>
        <dd style={{ margin: 0 }}>{g?.grantedAt ? `${zenScopeWhere(g.scope)}, ${g.grantedAt}` : 'Not allowed'}</dd>
        <dt style={{ color: 'var(--zen-text-muted)' }}>Sending</dt>
        <dd style={{ margin: 0 }}>{g?.caps.includes('thread:post') ? (g.sending ? 'On' : 'Off') : 'Can’t send'}</dd>
      </dl>
      {entries.length > 0 && (
        <div className="flex flex-col" style={{ gap: 2, fontSize: '0.85em' }} data-zen-about-entries={entries.length}>
          <span style={{ color: 'var(--zen-text-muted)' }}>Agents when you allowed it:</span>
          {entries.slice(0, 20).map((e) => (
            <span key={`${e.server}/${e.room}`}>
              {e.room} · {zenServerName(e.server)}
            </span>
          ))}
          {entries.length > 20 && <span style={{ color: 'var(--zen-text-muted)' }}>and {entries.length - 20} more</span>}
        </div>
      )}
      <div className="flex justify-end">
        <button type="button" onClick={onClose} className="cursor-pointer" style={zenButtonStyle(false)}>
          Close
        </button>
      </div>
    </ZenOverlay>
  )
}

/** K2's ⋯ menu at the box's top right, outside the frame (UW25). */
function CornerMenu({
  widget,
  gardenId,
  onPermissions,
  onAbout,
}: {
  widget: ZenCustomWidgetPayload
  gardenId: string
  onPermissions(): void
  onAbout(): void
}): React.JSX.Element {
  const [open, setOpen] = useState(false)
  const [sendingOverride, setSendingOverride] = useState<boolean | null>(null)
  const [error, setError] = useState<string | null>(null)
  const menu = useAnchoredMenu<HTMLButtonElement>({ open, onClose: () => setOpen(false), gap: 4, width: 'min', minWidth: 190, align: 'end' })
  const g = widget.grant
  const canSend = g !== null && g.caps.includes('thread:post')
  const sending = sendingOverride ?? g?.sending ?? false
  // A fresh grant from the daemon replaces the optimistic value.
  useEffect(() => setSendingOverride(null), [g?.sending, g?.grantedAt])
  const key = zenPlacementKey(gardenId, widget.id)
  const item = (testId: string, label: string, onClick: () => void): React.JSX.Element => (
    <button
      type="button"
      role="menuitem"
      data-zen-custom-menu-item={testId}
      onClick={() => {
        setOpen(false)
        onClick()
      }}
      className="flex w-full items-center text-left cursor-pointer"
      style={{ padding: '6px 10px', borderRadius: 'calc(var(--zen-radius) - 4px)', color: 'var(--zen-text)' }}
    >
      {label}
    </button>
  )
  return (
    <>
      <button
        ref={menu.anchorRef}
        type="button"
        aria-label={`${zenWidgetDisplayName(widget.name)} menu`}
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
      {error && (
        <span role="alert" data-zen-custom-menu-error="" style={{ position: 'absolute', top: 32, right: 6, zIndex: 2, fontSize: '0.8em', color: 'var(--zen-danger)' }}>
          {error}
        </span>
      )}
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
            {item('reload', 'Reload', () => reloadZenWidget(key))}
            {canSend &&
              item('sending', sending ? 'Sending: on (turn off)' : 'Sending: off (turn on)', () => {
                const on = !sending
                setSendingOverride(on)
                setError(null)
                void setZenWidgetSending({ garden: gardenId, placement: widget.id, on, reason: 'user' }).catch((err: unknown) => {
                  setSendingOverride(null)
                  setError(zenWidgetError(err).message)
                })
              })}
            {widget.requested.length > 0 && item('permissions', 'Permissions…', onPermissions)}
            {item('about', 'About this widget', onAbout)}
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
  const pausedStart = useZenCustomRunStore((s) => s.pausedStart)
  const [notNow, setNotNow] = useState(false)
  const [dialog, setDialog] = useState<'grant' | 'about' | null>(null)
  const [problem, setProblem] = useState<ZenCustomLoadProblem | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  // A new version of the widget may load fine: forget the last problem.
  useEffect(() => setProblem(null), [widget?.hash, generation])

  if (!widget) {
    return (
      <Card testId="unreadable">
        <span>K2 can’t read this widget’s placement. Ask your agent to run k2 zen validate.</span>
      </Card>
    )
  }
  const grant = widget.grant
  const dialogs = (
    <>
      {dialog === 'grant' && <ZenGrantDialog widget={widget} gardenId={gid} gardenName={gardenName(gid)} onClose={() => setDialog(null)} />}
      {dialog === 'about' && <AboutDialog widget={widget} gardenId={gid} onClose={() => setDialog(null)} />}
    </>
  )
  const wrap = (body: React.ReactNode): React.JSX.Element => (
    <div
      data-zen-widget="custom"
      data-zen-custom={widget.widget}
      data-zen-custom-placement={widget.id}
      className="relative flex h-full w-full flex-col"
      style={{ minHeight: 0, minWidth: 0, flex: '1 1 0%' }}
    >
      {body}
      {dialogs}
    </div>
  )

  // 1. Review.
  if (zenGrantNeedsReview(grant, widget.requested)) {
    if (notNow) {
      return wrap(
        <Strip
          testId="review"
          text={`“${zenWidgetDisplayName(widget.name)}” is waiting for your OK.`}
          action="Review"
          onAction={() => setDialog('grant')}
        />,
      )
    }
    return wrap(<ReviewCard widget={widget} onReview={() => setDialog('grant')} onNotNow={() => setNotNow(true)} />)
  }

  // 2. Paused.
  if (pausedStart) {
    return wrap(
      <Card testId="paused">
        <span>{ZEN_WIDGETS_PAUSED_TEXT}</span>
        <CardButton testId="run-them" primary onClick={runZenWidgetsNow}>
          Run them
        </CardButton>
      </Card>,
    )
  }
  if (grant?.paused || stop?.reason === 'runaway') {
    const resume = (): void => {
      setBusy(true)
      setError(null)
      void resumeZenWidget({ garden: gid, placement: widget.id })
        .then(() => {
          setBusy(false)
          reloadZenWidget(key)
        })
        .catch((err: unknown) => {
          setBusy(false)
          setError(zenWidgetError(err).message)
        })
    }
    return wrap(
      <Card testId="runaway">
        <span>{zenWidgetStopText('runaway')}</span>
        <CardButton testId="resume" primary disabled={busy} onClick={resume}>
          Resume
        </CardButton>
        {error && <span role="alert" style={{ color: 'var(--zen-danger)', fontSize: '0.85em' }}>{error}</span>}
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
  if (widget.state === 'broken' || problem?.code === 'widget_broken') {
    const first = widget.errors[0]?.message ?? problem?.message ?? 'it has no working version yet'
    return wrap(
      <Card testId="broken">
        <span>This widget has errors: {first.replace(/[.\s]+$/, '')}. Ask your agent to fix it.</span>
      </Card>,
    )
  }
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

  // 4. The frame and K2's corner menu.
  const missing = grant?.state === 'partial' ? widget.requested.filter((c) => !grant.caps.includes(c)) : []
  return wrap(
    <>
      {missing.length > 0 && <Strip testId="partial" text={zenAlsoAsksText(missing)} action="Review" onAction={() => setDialog('grant')} />}
      <div className="relative flex min-h-0 min-w-0 flex-1 flex-col">
        <ZenCustomFrame key={`${widget.hash}:${generation}`} widget={widget} gardenId={gid} bridge={bridge} onProblem={setProblem} />
        <CornerMenu widget={widget} gardenId={gid} onPermissions={() => setDialog('grant')} onAbout={() => setDialog('about')} />
      </div>
    </>,
  )
}
