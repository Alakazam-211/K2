// Settings → General → "Send feedback" (prd-k2-product-feedback-v1 PF8,
// PF10a, PF19, PF27). Daemon-owned, on the window's server:
//   - "Remember including K2 details": this person's own remembered choice
//     (only "start with the box ticked"; agents never inherit it);
//   - for the owner and admins: sending on/off;
//   - agent reports (plan-agent-reports-opt-in-app-v1): the answer, who
//     answered and when, the three answers and the parts agents may attach
//     (the daemon's `attachParts` labels). "Not answered yet" with Choose
//     now (the agent-reports card) until someone answers. Each change is one
//     POST of all three switches. A Member sees it read-only. An older
//     daemon (no `agentsAsked` / `attachParts`) gets its old two switches.
// Hidden on a server without the feature.

import { useCallback, useEffect, useRef, useState } from 'react'
import { Toggle } from '@/components/ui'
import { primaryScope } from '@/kessel/server-scope'
import {
  PRODUCT_FEEDBACK_FEATURE,
  fetchFeedbackSettings,
  refusalMessage,
  saveAgentReports,
  saveFeedbackSettings,
  type AgentReports,
  type AgentReportsAnswer,
  type FeedbackSettings,
} from '@/lib/product-feedback'
import { openSendFeedback } from '@/stores/send-feedback'
import { COMPANY_NAME, PRODUCT_NAME } from '@/lib/brand'
import { openAgentReportsCard, useAgentReportsCardStore } from '@/stores/agent-reports-card'
import { AgentReportsChoice, agentReportsCopy } from '@/components/AgentReports/AgentReportsCard'

/** Who answered and when, for the Settings line. */
export function answeredLine(r: Pick<AgentReports, 'askedAt' | 'askedBy'>): string {
  const who = !r.askedBy || r.askedBy === 'owner' ? 'the owner' : r.askedBy
  if (r.askedAt === null) return `Answered by ${who}.`
  const when = new Date(r.askedAt).toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' })
  return `Answered by ${who} on ${when}.`
}

/** Agent reports on a daemon that has the opt-in. */
function AgentReportsRows({
  r,
  canEdit,
  onSave,
}: {
  r: AgentReports
  canEdit: boolean
  onSave: (answer: AgentReportsAnswer, attach: string[]) => void
}): React.JSX.Element {
  const copy = agentReportsCopy()
  return (
    <div className="py-2 border-b border-[var(--color-border)]" data-settings-id="general.send-feedback.agent-reports" data-testid="send-feedback-agent-reports">
      <span className="type-row text-[var(--color-text-secondary)]">{copy.heading}</span>
      {r.answer === null ? (
        <div className="flex items-center justify-between mt-0.5">
          <p className="type-hint text-[var(--color-text-muted)] m-0 mr-3" data-testid="agent-reports-not-answered">
            {copy.notAnswered}
            {canEdit ? null : ` ${copy.members}`}
          </p>
          {canEdit ? (
            <button
              type="button"
              onClick={() => openAgentReportsCard('settings')}
              data-testid="agent-reports-choose-now"
              className="px-2 py-0.5 type-control text-[var(--color-text-secondary)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] transition-colors no-drag cursor-pointer flex-shrink-0"
            >
              Choose now
            </button>
          ) : null}
        </div>
      ) : (
        <>
          <p className="type-hint text-[var(--color-text-muted)] mt-0.5 mb-2" data-testid="agent-reports-answered">
            {answeredLine(r)}
            {canEdit ? null : ` ${copy.members}`}
          </p>
          <AgentReportsChoice
            answer={r.answer}
            attach={r.attach}
            parts={r.parts}
            disabled={!canEdit}
            onAnswer={(a) => onSave(a, a === 'off' ? [] : r.attach)}
            onAttach={(ids) => r.answer && onSave(r.answer, ids)}
          />
        </>
      )}
    </div>
  )
}

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

  const saveAnswer = useCallback(
    async (answer: AgentReportsAnswer, attach: string[]) => {
      try {
        setS(await saveAgentReports(scope, answer, attach))
        setError(null)
      } catch (e) {
        setError(refusalMessage(e))
      }
    },
    [scope],
  )

  // Choose now's card closed: read the answer it may have saved.
  const cardOpen = useAgentReportsCardStore((st) => st.openAs === 'settings')
  const wasOpen = useRef(false)
  useEffect(() => {
    if (wasOpen.current && !cardOpen && supported) {
      fetchFeedbackSettings(scope)
        .then(setS)
        .catch((e) => setError(refusalMessage(e)))
    }
    wasOpen.current = cardOpen
  }, [cardOpen, scope, supported])

  if (!supported) return null
  return (
    <div data-settings-group="send-feedback" data-testid="general-send-feedback-group">
      <h3 className="type-section uppercase tracking-wider text-[var(--color-text-muted)] m-0 mb-1">Send feedback</h3>
      <div className="flex items-center justify-between py-2 border-b border-[var(--color-border)]">
        <p className="type-hint text-[var(--color-text-muted)] m-0 mr-3">
          Tell {COMPANY_NAME}, who make {PRODUCT_NAME}, about a bug or an idea: the button next to Help, or Send Feedback… in the menu.
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
          label={`Start with ${PRODUCT_NAME} details ticked`}
          hint="Your own choice on this server. You still see what is included and can untick it every time."
          checked={s.rememberDiagnostics === true}
          onChange={(v) => void save({ rememberDiagnostics: v })}
        />
      ) : null}
      {s && s.canEdit ? (
        <Row
          id="enabled"
          label="Allow sending feedback to K2"
          hint="Off: the button only offers Copy as text. Air-gap always turns it off."
          checked={s.enabled}
          onChange={(v) => void save({ enabled: v })}
        />
      ) : null}
      {s && s.agentReports ? (
        <AgentReportsRows r={s.agentReports} canEdit={s.canEdit} onSave={(a, ids) => void saveAnswer(a, ids)} />
      ) : null}
      {s && s.canEdit && !s.agentReports ? (
        <>
          <Row
            id="enabled"
            label={`Allow sending feedback to ${PRODUCT_NAME}`}
            hint="Off: the button only offers Copy as text. Air-gap always turns it off."
            checked={s.enabled}
            onChange={(v) => void save({ enabled: v })}
          />
          <Row
            id="agents"
            label={`Agents may send feedback to ${PRODUCT_NAME}`}
            hint="At most 1 a minute and 3 a day per workspace, 10 a day for the server. You see theirs in the Sent tab."
            checked={s.agents}
            onChange={(v) => void save({ agents: v })}
          />
          <Row
            id="agent-diagnostics"
            label={`Agents may include ${PRODUCT_NAME} details`}
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
