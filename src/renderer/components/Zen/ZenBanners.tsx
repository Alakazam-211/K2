// prd-zen-mode-v1 Z13, Z29 — the two banners K2 draws inside Zen.
//   - Safe mode: "Zen is in safe mode. Your files are untouched." + the
//     cause + Try again + Exit Zen.
//   - Config error (not safe mode): "zen.toml line 12: unknown color
//     'acent'. Showing your last good version."

import { ZEN_SAFE_TITLE, zenSafeCauseText, type ZenSafeCause } from '@/lib/zen/zen-view'
import { zenErrorBannerText, type ZenIssue } from '@/lib/zen/zen-page'

const bannerStyle: React.CSSProperties = {
  background: 'var(--zen-surface-raised)',
  color: 'var(--zen-text)',
  borderBottom: '1px solid var(--zen-border)',
}

const buttonStyle: React.CSSProperties = {
  minHeight: 24,
  padding: '2px 10px',
  border: '1px solid var(--zen-border)',
  borderRadius: 999,
  color: 'var(--zen-text)',
  background: 'var(--zen-surface)',
}

export function ZenSafeBanner({
  cause,
  onTryAgain,
  onExit,
}: {
  cause: ZenSafeCause
  onTryAgain(): void
  onExit(): void
}): React.JSX.Element {
  return (
    <div
      role="status"
      data-zen-safe-banner=""
      data-zen-safe-cause={cause.kind}
      className="flex flex-shrink-0 flex-wrap items-center gap-3 px-4 py-2"
      style={bannerStyle}
    >
      <div className="min-w-0 flex-1">
        <div style={{ fontWeight: 600 }}>{ZEN_SAFE_TITLE}</div>
        <div data-zen-safe-reason="" style={{ color: 'var(--zen-text-muted)' }}>
          {zenSafeCauseText(cause)}
        </div>
      </div>
      <button type="button" className="no-drag" style={buttonStyle} onClick={onTryAgain} data-zen-try-again="">
        Try again
      </button>
      <button type="button" className="no-drag" style={buttonStyle} onClick={onExit} data-zen-safe-exit="">
        Exit Zen
      </button>
    </div>
  )
}

export function ZenConfigErrorBanner({ issue }: { issue: ZenIssue }): React.JSX.Element {
  return (
    <div
      role="status"
      data-zen-config-error=""
      className="flex-shrink-0 truncate px-4 py-1"
      style={{ ...bannerStyle, color: 'var(--zen-danger)' }}
      title={issue.message}
    >
      {zenErrorBannerText(issue)}
    </div>
  )
}
