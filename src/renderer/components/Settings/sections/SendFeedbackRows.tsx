// Settings → General → "Send feedback" (prd-k2-product-feedback-v1 PF8,
// PF10a, PF19, PF27). Daemon-owned, on the window's server:
//   - "Remember including K2 details": this person's own remembered choice
//     (only "start with the box ticked"; agents never inherit it);
//   - for the owner and admins: sending on/off, agents on/off, and whether
//     agents may include K2 details (off by default; a person's change).
// Hidden on a server without the feature.

import { useCallback, useEffect, useState } from 'react'
import { Toggle } from '@/components/ui'
import { primaryScope } from '@/kessel/server-scope'
import {
  PRODUCT_FEEDBACK_FEATURE,
  fetchFeedbackSettings,
  refusalMessage,
  saveFeedbackSettings,
  type FeedbackSettings,
} from '@/lib/product-feedback'
import { openSendFeedback } from '@/stores/send-feedback'

function Row({
  id,
  label,
  hint,
  checked,
  disabled,
  onChange,
}: {
  id: string
  label: string
  hint: string
  checked: boolean
  disabled?: boolean
  onChange: (next: boolean) => void
}): React.JSX.Element {
  return (
    <div className="flex items-center justify-between py-2 border-b border-[var(--color-border)]" data-settings-id={`general.send-feedback.${id}`}>
      <div className="flex-1 min-w-0 mr-3">
        <span className="type-row text-[var(--color-text-secondary)]">{label}</span>
        <p className="type-hint text-[var(--color-text-muted)] mt-0.5">{hint}</p>
      </div>
      <Toggle checked={checked} disabled={disabled} onChange={onChange} aria-label={label} data-testid={`send-feedback-setting-${id}`} />
    </div>
  )
}

export function SendFeedbackRows(): React.JSX.Element | null {
  const scope = primaryScope()
  const supported = scope.serverSupports(PRODUCT_FEEDBACK_FEATURE)
  const [s, setS] = useState<FeedbackSettings | null>(null)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    if (!supported) return
    let live = true
    fetchFeedbackSettings(scope)
      .then((v) => {
        if (live) setS(v)
      })
      .catch((e) => {
        if (live) setError(refusalMessage(e))
      })
    return () => {
      live = false
    }
  }, [scope, supported])

  const save = useCallback(
    async (patch: Parameters<typeof saveFeedbackSettings>[1]) => {
      try {
        setS(await saveFeedbackSettings(scope, patch))
        setError(null)
      } catch (e) {
        setError(refusalMessage(e))
      }
    },
    [scope],
  )

  if (!supported) return null
  return (
    <div data-settings-group="send-feedback" data-testid="general-send-feedback-group">
      <h3 className="type-section uppercase tracking-wider text-[var(--color-text-muted)] m-0 mb-1">Send feedback</h3>
      <div className="flex items-center justify-between py-2 border-b border-[var(--color-border)]">
        <p className="type-hint text-[var(--color-text-muted)] m-0 mr-3">
          Tell Alakazam Labs, who make K2, about a bug or an idea: the button next to Help, or Send Feedback… in the menu.
        </p>
        <button
          type="button"
          onClick={() => openSendFeedback('Settings')}
          className="px-2 py-0.5 type-control text-[var(--color-text-secondary)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] transition-colors no-drag cursor-pointer flex-shrink-0"
        >
          Send feedback…
        </button>
      </div>
      {s && s.rememberDiagnostics !== null ? (
        <Row
          id="remember"
          label="Start with K2 details ticked"
          hint="Your own choice on this server. You still see what is included and can untick it every time."
          checked={s.rememberDiagnostics === true}
          onChange={(v) => void save({ rememberDiagnostics: v })}
        />
      ) : null}
      {s && s.canEdit ? (
        <>
          <Row
            id="enabled"
            label="Allow sending feedback to K2"
            hint="Off: the button only offers Copy as text. Air-gap always turns it off."
            checked={s.enabled}
            onChange={(v) => void save({ enabled: v })}
          />
          <Row
            id="agents"
            label="Agents may send feedback to K2"
            hint="At most 1 a minute and 3 a day per workspace, 10 a day for the server. You see theirs in the Sent tab."
            checked={s.agents}
            onChange={(v) => void save({ agents: v })}
          />
          <Row
            id="agent-diagnostics"
            label="Agents may include K2 details"
            hint="An agent can't consent for you. On: an agent's feedback may carry the same scrubbed details a person can preview."
            checked={s.agentDiagnostics}
            onChange={(v) => void save({ agentDiagnostics: v })}
          />
        </>
      ) : null}
      {error ? <p className="type-hint text-[var(--color-status-error-soft)] mt-1" role="alert">{error}</p> : null}
    </div>
  )
}
