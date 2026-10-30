// View-only published-service details. Overlay/escape/close conventions
// mirror PresenceModal (DialogScrim + DialogFrame, backdrop mousedown
// closes, Escape via capture-phase window listener). No Start/Stop here
// — those stay on the list row.

import { useEffect } from 'react'
import { DialogFrame, DialogScrim } from '@/components/ui'
import type { ServerScope } from '@/kessel/server-scope'
import { AppDeploys, APP_CMD_SENTINEL } from './AppDeploys'
import { AppUpdateSetting } from './AppUpdateSetting'
import { SiteVisibilityControls } from './SiteVisibilityControls'
import {
  isServiceHealthy,
  serviceKindLabel,
  serviceLaunchLabel,
  servicePidLabel,
  serviceSkinUiLabel,
  type PublishedService,
} from './urls-ports'

interface PublishedServiceDetailsModalProps {
  service: PublishedService
  hostLabel: string
  onClose: () => void
  /** The service's server. With it, an App row (`cmd === '(app)'`) shows
   *  its deploy history and Roll back (App runtime S8). */
  scope?: ServerScope
}

export interface PublishedByoDetails {
  label: string
  url: string | null
  target: string
}

function Field({
  label,
  children,
  mono,
}: {
  label: string
  children: React.ReactNode
  mono?: boolean
}): React.JSX.Element {
  return (
    <div className="min-w-0">
      <div className="text-[10px] text-[var(--color-text-muted)]">{label}</div>
      <div
        className={`text-[11px] text-[var(--color-text-primary)] break-all ${mono ? 'font-mono' : ''}`}
      >
        {children}
      </div>
    </div>
  )
}

function dash(value: string | number | null | undefined): string {
  if (value === null || value === undefined) return '—'
  const s = String(value)
  return s.trim() ? s : '—'
}

export function PublishedServiceDetailsModal({
  service,
  hostLabel,
  onClose,
  scope,
}: PublishedServiceDetailsModalProps): React.JSX.Element {
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') {
        e.preventDefault()
        e.stopPropagation()
        onClose()
      }
    }
    window.addEventListener('keydown', handleKeyDown, true)
    return () => window.removeEventListener('keydown', handleKeyDown, true)
  }, [onClose])

  const launch = serviceLaunchLabel(service)
  const skinUi = serviceSkinUiLabel(service)
  const healthy = isServiceHealthy(service)
  const launchMono = service.kind !== 'skin'

  return (
    <>
      <DialogScrim
        onMouseDown={(e) => {
          e.stopPropagation()
          onClose()
        }}
      />
      <DialogFrame
        data-published-service-details=""
        role="dialog"
        aria-label={service.name}
        style={{
          width: 420,
          maxWidth: 'calc(100vw - 48px)',
          maxHeight: 'calc(100vh - 96px)',
          display: 'flex',
          flexDirection: 'column',
        }}
      >
        <div className="flex items-center justify-between px-4 py-3 border-b border-[var(--color-border)]">
          <div className="text-sm font-semibold text-[var(--color-text-primary)] truncate pr-2">
            {service.name}
          </div>
          <button
            type="button"
            onClick={onClose}
            className="flex h-6 w-6 items-center justify-center text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)] transition-colors flex-shrink-0"
            title="Close (Esc)"
          >
            <svg width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round">
              <line x1="2" y1="2" x2="10" y2="10" />
              <line x1="10" y1="2" x2="2" y2="10" />
            </svg>
          </button>
        </div>

        <div className="flex-1 overflow-y-auto px-4 py-3 space-y-3">
          <Field label="Status">
            {service.status || 'unknown'}
            {healthy ? ' · healthy' : ''}
          </Field>
          {scope && service.visibility ? (
            <Field label="Who may open it">
              <SiteVisibilityControls scope={scope} label={service.name} />
            </Field>
          ) : null}
          {service.dev ? (
            <Field label="Dev server">
              <span data-testid="service-dev-details">
                {service.dev.lane} · {service.dev.caps} · stops after{' '}
                {service.dev.idleSecs === null ? 'no idle limit' : `${Math.round(service.dev.idleSecs / 60)} idle minutes`}
              </span>
              {service.dev.warning ? (
                <span className="block text-[10px] text-[var(--color-status-warn-text)]">{service.dev.warning}</span>
              ) : null}
            </Field>
          ) : null}
          <Field label="Desired">{dash(service.desired)}</Field>
          <Field label="PID">{servicePidLabel(service)}</Field>
          <Field label="Error">{dash(service.error)}</Field>
          {service.envDeprecation ? (
            <Field label="Environment">
              <span data-env-deprecation className="text-[var(--color-status-warn-text)]">
                {service.envDeprecation}
              </span>
            </Field>
          ) : null}
          <Field label="Last exit">{service.lastExitCode !== null ? String(service.lastExitCode) : '—'}</Field>
          <Field label="Launch" mono={launchMono}>
            {launch}
            {skinUi ? (
              <span className="block text-[10px] text-[var(--color-text-muted)] font-sans mt-0.5">
                {skinUi}
              </span>
            ) : null}
          </Field>
          <Field label="cwd" mono>
            {dash(service.cwd)}
          </Field>
          <Field label="Port">{service.port !== null ? String(service.port) : '—'}</Field>
          <Field label="Expose">{dash(service.expose)}</Field>
          <Field label="Kind">{serviceKindLabel(service)}</Field>
          <Field label="Host">{dash(hostLabel)}</Field>
          {scope && service.cmd === APP_CMD_SENTINEL ? <AppDeploys scope={scope} app={service.name} /> : null}
          {scope && service.cmd === APP_CMD_SENTINEL ? <AppUpdateSetting scope={scope} app={service.name} /> : null}
        </div>
      </DialogFrame>
    </>
  )
}

/** Leftover nested URL (`k2 publish subdomain create`) — no process, no PID. */
export function PublishedByoDetailsModal({
  leftover,
  hostLabel,
  onClose,
}: {
  leftover: PublishedByoDetails
  hostLabel: string
  onClose: () => void
}): React.JSX.Element {
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') {
        e.preventDefault()
        e.stopPropagation()
        onClose()
      }
    }
    window.addEventListener('keydown', handleKeyDown, true)
    return () => window.removeEventListener('keydown', handleKeyDown, true)
  }, [onClose])

  return (
    <>
      <DialogScrim
        onMouseDown={(e) => {
          e.stopPropagation()
          onClose()
        }}
      />
      <DialogFrame
        data-published-byo-details=""
        role="dialog"
        aria-label={leftover.label}
        style={{
          width: 420,
          maxWidth: 'calc(100vw - 48px)',
          maxHeight: 'calc(100vh - 96px)',
          display: 'flex',
          flexDirection: 'column',
        }}
      >
        <div className="flex items-center justify-between px-4 py-3 border-b border-[var(--color-border)]">
          <div className="text-sm font-semibold text-[var(--color-text-primary)] truncate pr-2">
            {leftover.label}
          </div>
          <button
            type="button"
            onClick={onClose}
            className="flex h-6 w-6 items-center justify-center text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)] transition-colors flex-shrink-0"
            title="Close (Esc)"
          >
            <svg width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round">
              <line x1="2" y1="2" x2="10" y2="10" />
              <line x1="10" y1="2" x2="2" y2="10" />
            </svg>
          </button>
        </div>
        <div className="flex-1 overflow-y-auto px-4 py-3 space-y-3">
          <Field label="Kind">Nested URL</Field>
          <Field label="Public URL" mono>
            {dash(leftover.url)}
          </Field>
          <Field label="Target" mono>
            {leftover.target}
          </Field>
          <Field label="Host">{dash(hostLabel)}</Field>
          <p className="text-[10px] text-[var(--color-text-muted)]">
            Created with <span className="font-mono">k2 publish subdomain create</span>
            . No <span className="font-mono">publish run</span> process on this row.
          </p>
        </div>
      </DialogFrame>
    </>
  )
}
