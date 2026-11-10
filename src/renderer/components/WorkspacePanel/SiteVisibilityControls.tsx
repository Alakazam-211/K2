// Who may open a site, in the Published panel's details and Settings (PV-12):
// the chip, Open, Private, "Share with a PIN" and Public (which asks for a
// reason). The daemon decides who may; a refusal is shown as it comes.

import { useCallback, useEffect, useState } from 'react'
import { getSiteVisibility, openSite, setSiteVisibility, siteModeLabel, type SiteVisibility } from '@/lib/published-sites'
import { scopeMayWrite, type ServerScope } from '@/kessel/server-scope'
import { SitePinForm } from './SitePinForm'
import { Button, Tag } from '@/components/ui'

const DEV_PUBLIC_WARNING =
  'This is a dev server. Made public, anyone with the address can see its source maps, error pages with file paths, and anything the dev server evaluates.'

const buttonCls =
  'px-1.5 py-0.5 text-[10px] border border-[var(--color-border)] text-[var(--color-text-primary)] hover:bg-[var(--color-bg-elevated)] cursor-pointer disabled:opacity-50 no-drag'
const FIELD_SETTINGS =
  'k2-focus w-full px-2 py-1 type-control bg-[var(--color-bg)] border border-[var(--color-control-edge)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)] rounded-[var(--radius-field)] no-drag'
const fieldCls =
  'w-full px-2 py-1 text-[11px] bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-[var(--color-text-primary)] no-drag'

/** A visibility action: the drawer's compact button, or in a Settings row
 *  (`settings`) the shared quiet Button at the Settings size. */
function Act({ settings, ...rest }: React.ButtonHTMLAttributes<HTMLButtonElement> & { settings: boolean }): React.JSX.Element {
  return settings ? <Button variant="quiet" {...rest} /> : <button type="button" className={buttonCls} {...rest} />
}

export function SiteChip({ mode, dev, settings = false }: { mode: string | null | undefined; dev?: boolean; settings?: boolean }): React.JSX.Element | null {
  const text = siteModeLabel(mode, dev)
  if (!text) return null
  if (settings) {
    return (
      <Tag tone="neutral" data-testid="site-visibility-chip" data-mode={mode ?? ''} className="flex-shrink-0">
        {text}
      </Tag>
    )
  }
  return (
    <span
      data-testid="site-visibility-chip"
      data-mode={mode ?? ''}
      className="flex-shrink-0 text-[9px] font-medium uppercase tracking-wide px-1 py-px bg-[var(--color-wash-1)] text-[var(--color-text-secondary)]"
    >
      {text}
    </span>
  )
}

type Step = { kind: 'idle' } | { kind: 'pin' } | { kind: 'public' } | { kind: 'private-yes'; warning: string }

export function SiteVisibilityControls({
  scope,
  label,
  onChanged,
  settings = false,
}: {
  scope: ServerScope
  label: string
  onChanged?: (v: SiteVisibility) => void
  /** In a Settings row: shared buttons and the Settings type roles. */
  settings?: boolean
}): React.JSX.Element | null {
  const small = settings ? 'type-hint' : 'text-[10px]'
  const [site, setSite] = useState<SiteVisibility | null>(null)
  const [step, setStep] = useState<Step>({ kind: 'idle' })
  const [reason, setReason] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  useEffect(() => {
    let cancelled = false
    setSite(null)
    getSiteVisibility(scope, label)
      .then((v) => {
        if (!cancelled) setSite(v)
      })
      .catch((e: unknown) => {
        if (!cancelled) setError(e instanceof Error ? e.message : String(e))
      })
    return () => {
      cancelled = true
    }
  }, [scope, label])

  const done = useCallback(
    (v: SiteVisibility) => {
      setSite(v)
      setStep({ kind: 'idle' })
      setReason('')
      setError(null)
      onChanged?.(v)
    },
    [onChanged],
  )

  const makePrivate = async (yes: boolean): Promise<void> => {
    setBusy(true)
    setError(null)
    try {
      done(await setSiteVisibility(scope, label, { mode: 'private', ...(yes ? { yes: true } : {}) }))
    } catch (e) {
      const text = e instanceof Error ? e.message : String(e)
      if (text.includes('--yes')) {
        setStep({ kind: 'private-yes', warning: text.replace(/ Pass --yes to go ahead\.?/, '') })
      } else {
        setError(text)
      }
    } finally {
      setBusy(false)
    }
  }

  const makePublic = async (): Promise<void> => {
    if (!reason.trim()) {
      setError('Say why this site is public.')
      return
    }
    setBusy(true)
    setError(null)
    try {
      done(await setSiteVisibility(scope, label, { mode: 'public', reason: reason.trim(), ...(site?.dev ? { devIsPublic: true } : {}) }))
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  if (!site) {
    return error ? (
      <div data-testid="site-visibility-error" className={`${small} text-[var(--color-text-secondary)]`}>
        {error}
      </div>
    ) : null
  }
  const mayChange = scopeMayWrite(scope, 'publish/visibility')

  return (
    <div data-testid="site-visibility" className="flex flex-col gap-1.5 min-w-0">
      <div className="flex flex-wrap items-center gap-1.5">
        <SiteChip mode={site.mode} dev={site.dev} settings={settings} />
        {site.url ? (
          <Act settings={settings} data-testid="site-open" onClick={() => void openSite(scope, site.url as string)}>
            Open
          </Act>
        ) : null}
        {mayChange && site.mode !== 'private' ? (
          <Act settings={settings} data-testid="site-make-private" disabled={busy} onClick={() => void makePrivate(false)}>
            Make private
          </Act>
        ) : null}
        {mayChange ? (
          <Act settings={settings} data-testid="site-share-pin" disabled={busy} onClick={() => setStep({ kind: 'pin' })}>
            {site.mode === 'pin' ? 'Change the PIN' : 'Share with a PIN'}
          </Act>
        ) : null}
        {mayChange && site.mode !== 'public' ? (
          <Act settings={settings} data-testid="site-make-public" disabled={busy} onClick={() => setStep({ kind: 'public' })}>
            Make public
          </Act>
        ) : null}
      </div>
      {site.mode === 'public' && site.reason ? (
        <div data-testid="site-public-reason" className={`${small} text-[var(--color-text-secondary)]`}>
          Public because: {site.reason}
        </div>
      ) : null}
      {site.warnings.map((w) => (
        <div key={w} data-testid="site-visibility-warning" className={`${small} text-[var(--color-status-warn-text)]`}>
          {w}
        </div>
      ))}
      {step.kind === 'pin' ? (
        <SitePinForm scope={scope} label={label} dev={site.dev} onDone={done} onCancel={() => setStep({ kind: 'idle' })} />
      ) : null}
      {step.kind === 'private-yes' ? (
        <div className="flex flex-col gap-1">
          <div data-testid="site-private-warning" className={`${small} text-[var(--color-status-warn-text)]`}>
            {step.warning}
          </div>
          <div className="flex gap-1.5">
            <Act settings={settings} data-testid="site-private-yes" disabled={busy} onClick={() => void makePrivate(true)}>
              Go ahead
            </Act>
            <Act settings={settings} disabled={busy} onClick={() => setStep({ kind: 'idle' })}>
              Cancel
            </Act>
          </div>
        </div>
      ) : null}
      {step.kind === 'public' ? (
        <div className="flex flex-col gap-1">
          {site.dev ? (
            <div data-testid="site-public-dev-warning" className={`${small} text-[var(--color-status-warn-text)]`}>
              {DEV_PUBLIC_WARNING}
            </div>
          ) : null}
          <input
            data-testid="site-public-reason-input"
            aria-label="Why is this site public?"
            value={reason}
            maxLength={200}
            onChange={(e) => setReason(e.target.value)}
            placeholder="Why is this site public?"
            className={settings ? FIELD_SETTINGS : fieldCls}
          />
          <div className="flex gap-1.5">
            <Act settings={settings} data-testid="site-public-save" disabled={busy || !reason.trim()} onClick={() => void makePublic()}>
              {site.dev ? 'Make it public anyway' : 'Make public'}
            </Act>
            <Act settings={settings} disabled={busy} onClick={() => setStep({ kind: 'idle' })}>
              Cancel
            </Act>
          </div>
        </div>
      ) : null}
      {error ? (
        <div data-testid="site-visibility-error" className={`${small} text-[var(--color-status-error-soft)]`}>
          {error}
        </div>
      ) : null}
    </div>
  )
}
