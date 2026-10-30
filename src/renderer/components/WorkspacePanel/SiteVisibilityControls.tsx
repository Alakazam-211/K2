// Who may open a site, in the Published panel's details and Settings (PV-12):
// the chip, Open, Private, "Share with a PIN" and Public (which asks for a
// reason). The daemon decides who may; a refusal is shown as it comes.

import { useCallback, useEffect, useState } from 'react'
import { getSiteVisibility, openSite, setSiteVisibility, siteModeLabel, type SiteVisibility } from '@/lib/published-sites'
import { scopeMayWrite, type ServerScope } from '@/kessel/server-scope'
import { SitePinForm } from './SitePinForm'

const DEV_PUBLIC_WARNING =
  'This is a dev server. Made public, anyone with the address can see its source maps, error pages with file paths, and anything the dev server evaluates.'

const buttonCls =
  'px-1.5 py-0.5 text-[10px] border border-[var(--color-border)] text-[var(--color-text-primary)] hover:bg-[var(--color-bg-elevated)] cursor-pointer disabled:opacity-50 no-drag'
const fieldCls =
  'w-full px-2 py-1 text-[11px] bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-[var(--color-text-primary)] no-drag'

export function SiteChip({ mode, dev }: { mode: string | null | undefined; dev?: boolean }): React.JSX.Element | null {
  const text = siteModeLabel(mode, dev)
  if (!text) return null
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
}: {
  scope: ServerScope
  label: string
  onChanged?: (v: SiteVisibility) => void
}): React.JSX.Element | null {
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
      <div data-testid="site-visibility-error" className="text-[10px] text-[var(--color-text-muted)]">
        {error}
      </div>
    ) : null
  }
  const mayChange = scopeMayWrite(scope, 'publish/visibility')

  return (
    <div data-testid="site-visibility" className="flex flex-col gap-1.5 min-w-0">
      <div className="flex flex-wrap items-center gap-1.5">
        <SiteChip mode={site.mode} dev={site.dev} />
        {site.url ? (
          <button type="button" data-testid="site-open" className={buttonCls} onClick={() => void openSite(scope, site.url as string)}>
            Open
          </button>
        ) : null}
        {mayChange && site.mode !== 'private' ? (
          <button type="button" data-testid="site-make-private" disabled={busy} className={buttonCls} onClick={() => void makePrivate(false)}>
            Make private
          </button>
        ) : null}
        {mayChange ? (
          <button type="button" data-testid="site-share-pin" disabled={busy} className={buttonCls} onClick={() => setStep({ kind: 'pin' })}>
            {site.mode === 'pin' ? 'Change the PIN' : 'Share with a PIN'}
          </button>
        ) : null}
        {mayChange && site.mode !== 'public' ? (
          <button type="button" data-testid="site-make-public" disabled={busy} className={buttonCls} onClick={() => setStep({ kind: 'public' })}>
            Make public
          </button>
        ) : null}
      </div>
      {site.mode === 'public' && site.reason ? (
        <div data-testid="site-public-reason" className="text-[10px] text-[var(--color-text-muted)]">
          Public because: {site.reason}
        </div>
      ) : null}
      {site.warnings.map((w) => (
        <div key={w} data-testid="site-visibility-warning" className="text-[10px] text-[var(--color-status-warn-text)]">
          {w}
        </div>
      ))}
      {step.kind === 'pin' ? (
        <SitePinForm scope={scope} label={label} dev={site.dev} onDone={done} onCancel={() => setStep({ kind: 'idle' })} />
      ) : null}
      {step.kind === 'private-yes' ? (
        <div className="flex flex-col gap-1">
          <div data-testid="site-private-warning" className="text-[10px] text-[var(--color-status-warn-text)]">
            {step.warning}
          </div>
          <div className="flex gap-1.5">
            <button type="button" data-testid="site-private-yes" disabled={busy} className={buttonCls} onClick={() => void makePrivate(true)}>
              Go ahead
            </button>
            <button type="button" disabled={busy} className={buttonCls} onClick={() => setStep({ kind: 'idle' })}>
              Cancel
            </button>
          </div>
        </div>
      ) : null}
      {step.kind === 'public' ? (
        <div className="flex flex-col gap-1">
          {site.dev ? (
            <div data-testid="site-public-dev-warning" className="text-[10px] text-[var(--color-status-warn-text)]">
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
            className={fieldCls}
          />
          <div className="flex gap-1.5">
            <button type="button" data-testid="site-public-save" disabled={busy || !reason.trim()} className={buttonCls} onClick={() => void makePublic()}>
              {site.dev ? 'Make it public anyway' : 'Make public'}
            </button>
            <button type="button" disabled={busy} className={buttonCls} onClick={() => setStep({ kind: 'idle' })}>
              Cancel
            </button>
          </div>
        </div>
      ) : null}
      {error ? (
        <div data-testid="site-visibility-error" className="text-[10px] text-[var(--color-status-error)]">
          {error}
        </div>
      ) : null}
    </div>
  )
}
