// The agent-reports card (plan-agent-reports-opt-in-app-v1): may this
// server's agents report problems to the company on their own, and what
// may they attach. One card for the setup list's `agent_reports` row
// (Save · Skip), the one-time offer on a server already in use (Save · Ask
// me later) and Settings' "Choose now" (Save · Cancel). Only the owner, an
// Owner or an Admin gets it: the daemon offers it to them only, and the
// settings route refuses anyone else with its own words.
//
// The daemon owns the answer. Save is one POST of `agents`, `agentPlatform`
// and `agentAttach` together; the daemon stamps who and when, and tells
// every window to read the setup list again. The parts come from the
// daemon (`attachParts`, its own labels), so a new part shows by itself.
// The copy says what each choice is for; the names come from the brand.

import { useCallback, useEffect, useId, useState } from 'react'
import { Button, Checkbox, Radio } from '@/components/ui'
import { AccessSheet, DIALOG_LABEL, DIALOG_MUTED } from '@/components/Settings/sections/AccessDialogs'
import { SETTING_ROW_LABEL } from '@/components/Settings/controls/SettingControls'
import { CLI_NAME, COMPANY_NAME, PRODUCT_NAME } from '@/lib/brand'
import type { ServerScope } from '@/kessel/server-scope'
import {
  fetchFeedbackSettings,
  refusalMessage,
  saveAgentReports,
  type AgentReportsAnswer,
  type AttachPart,
} from '@/lib/product-feedback'
import { useFirstRunStore } from '@/stores/first-run'
import type { AgentReportsCardKind } from '@/stores/agent-reports-card'

/** The card's words (v1, purpose only), with the brand's names. */
export function agentReportsCopy(): {
  heading: string
  body: string
  answers: { id: AgentReportsAnswer; label: string; hint: string | null }[]
  attachHeading: string
  attachHint: string
  always: string
  footer: string
  members: string
  notAnswered: string
  rowLine: string
} {
  const product = PRODUCT_NAME
  return {
    heading: `May your agents report ${product} problems?`,
    body: `When an agent hits a problem with ${product}, it can send a short report to ${COMPANY_NAME}. Reports are private: they go only to ${COMPANY_NAME} and are never made public. You see every report in Send feedback, Sent.`,
    answers: [
      {
        id: 'cli',
        label: `Yes, problems with the ${CLI_NAME} commands agents run.`,
        hint: 'For example, a command that failed, an unclear error, or help that does not match.',
      },
      { id: 'all', label: `Yes, anything about ${product}.`, hint: null },
      { id: 'off', label: 'No.', hint: 'Agents are not told about reports, and cannot send them.' },
    ],
    attachHeading: 'What may they attach?',
    attachHint:
      'Every item is off until you tick it. Names, files, screenshots, terminal output, chats, mail, tokens and IP addresses are never sent.',
    always: `Always included: ${product} version and install kind`,
    footer: 'You can change this later in Settings, General, Send feedback.',
    members: 'The owner or an admin chooses this.',
    notAnswered: 'Not answered yet, so agents send no reports.',
    rowLine: 'Not answered yet. Agents do not send reports until you choose.',
  }
}

/** The three answers and, for a yes, the parts they may attach. Controlled;
 *  used by the card and by Settings → General → Send feedback. */
export function AgentReportsChoice({
  answer,
  attach,
  parts,
  disabled = false,
  onAnswer,
  onAttach,
}: {
  answer: AgentReportsAnswer | null
  attach: readonly string[]
  parts: readonly AttachPart[]
  disabled?: boolean
  onAnswer: (a: AgentReportsAnswer) => void
  onAttach: (ids: string[]) => void
}): React.JSX.Element {
  const copy = agentReportsCopy()
  const name = useId()
  const toggle = (id: string, on: boolean): void => {
    // In the daemon's order, whatever order they were ticked in.
    onAttach(parts.map((p) => p.id).filter((p) => (p === id ? on : attach.includes(p))))
  }
  return (
    <div className="space-y-4">
      <div role="radiogroup" aria-label={copy.heading} className="space-y-2" data-testid="agent-reports-answers">
        {copy.answers.map((a) => (
          <label key={a.id} className={`flex items-start gap-2 ${disabled ? 'cursor-default' : 'cursor-pointer'}`}>
            <Radio
              name={name}
              className="mt-0.5"
              checked={answer === a.id}
              disabled={disabled}
              onChange={() => onAnswer(a.id)}
              aria-label={a.label}
              data-testid={`agent-reports-answer-${a.id}`}
            />
            <span>
              <span className={SETTING_ROW_LABEL}>{a.label}</span>
              {a.hint ? <span className={`block ${DIALOG_MUTED}`}>{a.hint}</span> : null}
            </span>
          </label>
        ))}
      </div>
      {answer === 'cli' || answer === 'all' ? (
        <fieldset className="space-y-2" data-testid="agent-reports-attach">
          <legend className={DIALOG_LABEL}>{copy.attachHeading}</legend>
          <p className={DIALOG_MUTED}>{copy.attachHint}</p>
          {parts.map((p) => (
            <label key={p.id} className={`flex items-start gap-2 ${disabled ? 'cursor-default' : 'cursor-pointer'}`}>
              <Checkbox
                className="mt-0.5"
                checked={attach.includes(p.id)}
                disabled={disabled}
                onChange={(e) => toggle(p.id, e.target.checked)}
                aria-label={p.label}
                data-testid={`agent-reports-part-${p.id}`}
              />
              <span className={SETTING_ROW_LABEL}>{p.label}</span>
            </label>
          ))}
          <label className="flex items-start gap-2 cursor-default">
            <Checkbox className="mt-0.5" checked disabled readOnly aria-label={copy.always} data-testid="agent-reports-part-always" />
            <span className={DIALOG_MUTED}>{copy.always}</span>
          </label>
        </fieldset>
      ) : null}
    </div>
  )
}

const SECONDARY: Record<AgentReportsCardKind, string> = { row: 'Skip', offer: 'Ask me later', settings: 'Cancel' }

/** The card in its sheet. `onClose` runs once it is done with (saved,
 *  skipped, asked later, or closed). */
export function AgentReportsCard({
  scope,
  kind,
  onClose,
}: {
  scope: ServerScope
  kind: AgentReportsCardKind
  onClose: () => void
}): React.JSX.Element {
  const copy = agentReportsCopy()
  const [loaded, setLoaded] = useState(false)
  const [canEdit, setCanEdit] = useState(true)
  const [parts, setParts] = useState<AttachPart[]>([])
  const [answer, setAnswer] = useState<AgentReportsAnswer | null>(null)
  const [attach, setAttach] = useState<string[]>([])
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    let live = true
    fetchFeedbackSettings(scope)
      .then((s) => {
        if (!live) return
        setCanEdit(s.canEdit)
        setParts(s.agentReports?.parts ?? [])
        setAnswer(s.agentReports?.answer ?? null)
        setAttach(s.agentReports?.attach ?? [])
        setLoaded(true)
      })
      .catch((e: unknown) => {
        if (!live) return
        setError(refusalMessage(e))
        setLoaded(true)
      })
    return () => {
      live = false
    }
  }, [scope])

  const run = useCallback(
    async (f: () => Promise<unknown>): Promise<void> => {
      setBusy(true)
      setError(null)
      try {
        await f()
        void useFirstRunStore.getState().load(scope)
        onClose()
      } catch (e) {
        setError(refusalMessage(e))
      } finally {
        setBusy(false)
      }
    },
    [scope, onClose],
  )

  const save = (): void => {
    if (!answer) return
    void run(() => saveAgentReports(scope, answer, answer === 'off' ? [] : attach))
  }
  const later = (): void => {
    if (kind === 'row') {
      void run(async () => {
        await useFirstRunStore.getState().skip('agent_reports', scope)
        const err = useFirstRunStore.getState().entries[scope.id]?.error
        if (err) throw new Error(err)
      })
    } else if (kind === 'offer') {
      void run(async () => {
        await useFirstRunStore.getState().dismissAgentReports(scope)
        const err = useFirstRunStore.getState().entries[scope.id]?.error
        if (err) throw new Error(err)
      })
    } else {
      onClose()
    }
  }
  // Closing the offer (×, Esc) is "Ask me later": it isn't offered again.
  const dismiss = kind === 'offer' ? later : onClose

  const footer = (
    <>
      <Button variant="ghost" size="md" onClick={later} disabled={busy} data-testid="agent-reports-later">
        {SECONDARY[kind]}
      </Button>
      {canEdit ? (
        <Button
          variant="accent"
          size="md"
          onClick={save}
          disabled={busy || !loaded || answer === null}
          data-testid="agent-reports-save"
        >
          {busy ? 'Saving…' : 'Save'}
        </Button>
      ) : null}
    </>
  )

  return (
    <AccessSheet title={copy.heading} intro={copy.body} onClose={dismiss} busy={busy} testId={`agent-reports-card-${kind}`} footer={footer}>
      <div className="space-y-4">
        {canEdit ? (
          <AgentReportsChoice
            answer={answer}
            attach={attach}
            parts={parts}
            disabled={busy || !loaded}
            onAnswer={setAnswer}
            onAttach={setAttach}
          />
        ) : (
          <p className={DIALOG_MUTED} data-testid="agent-reports-members">
            {copy.members}
          </p>
        )}
        <p className={DIALOG_MUTED}>{copy.footer}</p>
        {error ? (
          <p className="type-hint text-[var(--color-status-error-soft)]" role="alert" data-testid="agent-reports-error">
            {error}
          </p>
        ) : null}
      </div>
    </AccessSheet>
  )
}
