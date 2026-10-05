// prd-zen-gardens-v1 G28 (decision 4) — the empty Garden: what a new Garden
// (`k2.blank@1`) shows until its file has widgets.
//
// Centred, in Zen tokens: "This Garden is empty." and "Ask your agents to
// add things to this Garden.", plus **Ask my agent**. That opens a short
// chooser of agents on THIS computer only (`agents.local`): only an agent
// on this computer can edit this computer's `~/.k2/zen`. Picking one turns
// the widget into that agent's conversation (the built-in Conversation,
// same bridge) with the message box pre-filled, not sent
// (`compose.draft`): the person finishes the sentence and sends it. The
// agent then edits `gardens/<id>.toml`.
//
// Leaving the Garden and coming back shows the empty page again (the pick
// is this widget's own state, and its conversation closes on unmount).
//
// The same Ask my agent flow (`ZenAskMyAgent`) serves the nav rail's
// Projects view (Rosson 2026-10-04): "Coming soon — or build a new one
// yourself!", where "build a new one yourself" opens the chooser.
//
// Beside Ask my agent, **Start with the default** (Rosson 2026-10-04)
// turns THIS Garden into Garden 1's texting page, same id, name and place
// (`gardens.useTemplate('texting')`, cap `gardens:template`; the daemon
// keeps the old file in history). It shows only while the Garden really is
// empty (`gardens.empty()`: the blank template with no widgets of its
// own). Nothing is lost, so it asks nothing; if the daemon says the file
// has changes of its own (theme tables), it asks once before replacing
// them. The page then switches live on the daemon's `zen_changed`.
//
// A Garden made with "Start empty and ask my agent" opens the chooser by
// itself the first time it shows (`takeZenGardenAsk`, through
// `ZenAskMyAgent`'s `autoAsk`).

import { useEffect, useRef, useState } from 'react'
import type { ZenAgentRow } from '@/lib/zen/zen-data'
import type { ZenGardenTemplateResult } from '@/lib/zen/zen-bridge'
import { takeZenGardenAsk } from '@/lib/zen/zen-garden-ask'
import type { ZenWidgetProps } from '../zen-registry'
import { ZenConversation } from './ZenConversationWidget'
import { ZenAgentAvatar, ZenWidgetStyles, useZenRows } from './zen-widget-kit'

export const ZEN_GARDEN_EMPTY_TITLE = 'This Garden is empty.'
export const ZEN_GARDEN_EMPTY_ASK = 'Ask your agents to add things to this Garden.'
export const ZEN_NO_LOCAL_AGENTS = 'No agents on this computer yet. Add one from Home.'
export const ZEN_GARDEN_HINT = '(Use the k2-zen skill and k2 zen garden; check with k2 zen validate.)'

/** The draft Ask my agent puts in the message box (never sent). */
export function zenGardenAskDraft(garden: { id: string; name: string }): string {
  return `In my Zen Garden "${garden.name}" (id ${garden.id}), please add: \n${ZEN_GARDEN_HINT}`
}

type Phase = { kind: 'empty' } | { kind: 'choosing'; rows: ZenAgentRow[] | null; error: string | null } | { kind: 'talking'; address: string }

/** Start with the default: idle, sending, asking before it replaces the
 *  file's own changes, or failed. */
type Starting =
  | { kind: 'idle' }
  | { kind: 'busy' }
  | { kind: 'confirm'; message: string }
  | { kind: 'failed'; message: string }

export const ZEN_START_DEFAULT = 'Start with the default'

function Chooser({
  rows,
  error,
  onPick,
  onCancel,
}: {
  rows: ZenAgentRow[] | null
  error: string | null
  onPick(row: ZenAgentRow): void
  onCancel(): void
}): React.JSX.Element {
  return (
    <div
      role="dialog"
      aria-label="Ask my agent"
      data-zen-ask-chooser=""
      className="flex flex-col"
      style={{
        marginTop: 14,
        minWidth: 260,
        maxWidth: 360,
        maxHeight: '50vh',
        padding: 6,
        gap: 2,
        textAlign: 'left',
        background: 'var(--zen-surface-raised)',
        border: '1px solid var(--zen-border)',
        borderRadius: 'var(--zen-radius)',
      }}
    >
      {rows === null && !error && (
        <div style={{ padding: '8px 10px', color: 'var(--zen-text-muted)' }}>Looking for agents…</div>
      )}
      {error && <div style={{ padding: '8px 10px', color: 'var(--zen-danger)' }}>{error}</div>}
      {rows !== null && rows.length === 0 && (
        <div data-zen-no-local-agents="" style={{ padding: '8px 10px', color: 'var(--zen-text-muted)' }}>
          {ZEN_NO_LOCAL_AGENTS}
        </div>
      )}
      {rows !== null && rows.length > 0 && (
        <div className="flex flex-col overflow-y-auto" style={{ gap: 2 }}>
          {rows.map((r) => (
            <button
              key={r.address}
              type="button"
              data-zen-ask-agent={r.address}
              data-zen-soft-button=""
              onClick={() => onPick(r)}
              className="flex w-full items-center gap-3 text-left cursor-pointer disabled:cursor-default"
              style={{ padding: '6px 10px', borderRadius: 'calc(var(--zen-radius) - 4px)', color: 'var(--zen-text)' }}
            >
              <ZenAgentAvatar
                label={r.label}
                url={r.avatarUrl}
                size={26}
                background="var(--zen-bubble-agent)"
                color="var(--zen-bubble-agent-text)"
                fontSize="0.8em"
              />
              <span className="truncate">{r.label}</span>
            </button>
          ))}
        </div>
      )}
      <button
        className="cursor-pointer disabled:cursor-default"
        type="button"
        data-zen-ask-cancel=""
        data-zen-soft-button=""
        onClick={onCancel}
        style={{ alignSelf: 'flex-end', marginTop: 4, padding: '3px 10px', borderRadius: 999, color: 'var(--zen-text-muted)' }}
      >
        Cancel
      </button>
    </div>
  )
}

/** What the idle panel shows around the Ask my agent trigger. */
export interface ZenAskIntro {
  /** `ask` opens the chooser. `phase`: idle, the chooser is open (drawn
   *  under the intro), or the picked agent's conversation is opening. */
  (ask: () => void, phase: 'empty' | 'choosing' | 'talking'): React.ReactNode
}

/**
 * Ask my agent: an idle panel (`intro`), then a chooser of agents on this
 * computer, then that agent's conversation in place with `draft` in its
 * box (never sent). `kind` is the panel's `data-zen-widget`.
 */
export function ZenAskMyAgent({
  bridge,
  decl,
  kind,
  intro,
  draft,
  autoAsk,
}: ZenWidgetProps & {
  kind: string
  intro: ZenAskIntro
  draft(garden: { id: string; name: string }): string
  /** Checked once when the panel mounts: true opens the chooser by itself. */
  autoAsk?(): boolean
}): React.JSX.Element {
  const [phase, setPhase] = useState<Phase>({ kind: 'empty' })
  const rows = useZenRows(bridge)
  const drafted = useRef<string | null>(null)
  const draftRef = useRef(draft)
  draftRef.current = draft
  const talking = phase.kind === 'talking' ? phase.address : null
  const row = talking ? (rows.find((r) => r.address === talking) ?? null) : null

  // The conversation closes with the widget (leaving the Garden).
  useEffect(
    () => () => {
      try {
        bridge.call('conversation.close')
      } catch (err) {
        console.warn('[zen] closing the Ask my agent conversation failed:', err)
      }
    },
    [bridge],
  )

  // Once the conversation is on screen, fill its box (never send).
  useEffect(() => {
    if (!row || drafted.current === row.address) return
    drafted.current = row.address
    const garden = bridge.gardens.current()
    if (!garden) {
      console.warn('[zen] Ask my agent: no current Garden to name in the draft')
      return
    }
    bridge.call('compose.draft', row.address, draftRef.current(garden))
  }, [bridge, row])

  const ask = (): void => {
    setPhase({ kind: 'choosing', rows: null, error: null })
    void Promise.resolve()
      .then(() => bridge.call('agents.local') as Promise<ZenAgentRow[]>)
      .then((list) => setPhase((p) => (p.kind === 'choosing' ? { kind: 'choosing', rows: list, error: null } : p)))
      .catch((err: unknown) =>
        setPhase((p) =>
          p.kind === 'choosing'
            ? { kind: 'choosing', rows: [], error: `Couldn’t list agents: ${err instanceof Error ? err.message : String(err)}` }
            : p,
        ),
      )
  }

  const askRef = useRef(ask)
  askRef.current = ask
  const autoAskRef = useRef(autoAsk)
  useEffect(() => {
    if (autoAskRef.current?.()) askRef.current()
  }, [])

  const pick = (r: ZenAgentRow): void => {
    setPhase({ kind: 'talking', address: r.address })
    void Promise.resolve()
      .then(() => bridge.call('conversation.open', r.address))
      .catch((err: unknown) => console.warn('[zen] Ask my agent: open failed:', err))
  }

  if (row) {
    return (
      <div className="flex h-full min-h-0 w-full flex-col" data-zen-widget={kind} data-zen-widget-id={decl.id} data-zen-asking={row.address}>
        <ZenWidgetStyles />
        <ZenConversation bridge={bridge} row={row} />
      </div>
    )
  }
  return (
    <div
      className="flex h-full min-h-0 w-full flex-col items-center justify-center text-center"
      data-zen-widget={kind}
      data-zen-widget-id={decl.id}
      style={{ padding: 24, color: 'var(--zen-text)' }}
    >
      <ZenWidgetStyles />
      {intro(ask, phase.kind)}
      {phase.kind === 'choosing' && (
        <Chooser rows={phase.rows} error={phase.error} onPick={pick} onCancel={() => setPhase({ kind: 'empty' })} />
      )}
    </div>
  )
}

export function ZenGardenEmptyWidget(props: ZenWidgetProps): React.JSX.Element {
  const { bridge } = props
  const [starting, setStarting] = useState<Starting>({ kind: 'idle' })
  const canStart = bridge.caps.has('gardens:template') && bridge.call('gardens.empty') === true

  const startDefault = (force: boolean): void => {
    setStarting({ kind: 'busy' })
    void Promise.resolve()
      .then(() => bridge.call('gardens.useTemplate', 'texting', { force }) as Promise<ZenGardenTemplateResult>)
      // Changed: the daemon's zen_changed re-reads the page, which unmounts
      // this widget; until then the button stays busy.
      .then((r) => {
        if (!r.changed) setStarting({ kind: 'idle' })
      })
      .catch((err: unknown) => {
        const code = err instanceof Error && 'code' in err ? err.code : null
        const message = err instanceof Error ? err.message : String(err)
        setStarting(code === 'has_changes' && !force ? { kind: 'confirm', message } : { kind: 'failed', message })
      })
  }

  return (
    <ZenAskMyAgent
      {...props}
      kind="garden-empty"
      draft={zenGardenAskDraft}
      autoAsk={() => takeZenGardenAsk(bridge.gardens.current()?.id)}
      intro={(ask, phase) => (
        <>
          <div data-zen-garden-empty-title="" style={{ fontSize: '1.25em', fontWeight: 600 }}>
            {ZEN_GARDEN_EMPTY_TITLE}
          </div>
          <div data-zen-garden-empty-ask="" style={{ marginTop: 6, color: 'var(--zen-text-muted)' }}>
            {ZEN_GARDEN_EMPTY_ASK}
          </div>
          {phase !== 'choosing' && starting.kind === 'confirm' && (
            <div
              role="alertdialog"
              aria-label={ZEN_START_DEFAULT}
              data-zen-start-default-confirm=""
              className="flex flex-col items-center"
              style={{ marginTop: 16, maxWidth: 360, gap: 10 }}
            >
              <div style={{ color: 'var(--zen-text-muted)' }}>{starting.message}</div>
              <div className="flex items-center" style={{ gap: 8 }}>
                <button
                  className="cursor-pointer"
                  type="button"
                  data-zen-start-default-replace=""
                  onClick={() => startDefault(true)}
                  style={{ padding: '8px 18px', borderRadius: 999, background: 'var(--zen-accent)', color: 'var(--zen-accent-text)', fontWeight: 600 }}
                >
                  {ZEN_START_DEFAULT}
                </button>
                <button
                  className="cursor-pointer"
                  type="button"
                  data-zen-start-default-cancel=""
                  data-zen-soft-button=""
                  onClick={() => setStarting({ kind: 'idle' })}
                  style={{ padding: '8px 14px', borderRadius: 999, color: 'var(--zen-text-muted)' }}
                >
                  Cancel
                </button>
              </div>
            </div>
          )}
          {phase !== 'choosing' && starting.kind !== 'confirm' && (
            <div className="flex flex-wrap items-center justify-center" style={{ marginTop: 16, gap: 8 }}>
              <button
                className="cursor-pointer disabled:cursor-default"
                type="button"
                data-zen-ask-my-agent=""
                disabled={phase === 'talking' || starting.kind === 'busy'}
                onClick={ask}
                style={{
                  padding: '8px 18px',
                  borderRadius: 999,
                  background: 'var(--zen-accent)',
                  color: 'var(--zen-accent-text)',
                  fontWeight: 600,
                }}
              >
                Ask my agent
              </button>
              {canStart && (
                <button
                  className="cursor-pointer disabled:cursor-default"
                  type="button"
                  data-zen-start-default=""
                  disabled={phase === 'talking' || starting.kind === 'busy'}
                  onClick={() => startDefault(false)}
                  title="Turn this Garden into Garden 1’s layout: your agents beside a conversation"
                  style={{
                    padding: '7px 17px',
                    borderRadius: 999,
                    color: 'var(--zen-text)',
                    background: 'var(--zen-surface)',
                    border: '1px solid var(--zen-border)',
                    fontWeight: 600,
                  }}
                >
                  {starting.kind === 'busy' ? 'Starting…' : ZEN_START_DEFAULT}
                </button>
              )}
            </div>
          )}
          {starting.kind === 'failed' && (
            <div role="alert" data-zen-start-default-error="" style={{ marginTop: 10, color: 'var(--zen-danger)' }}>
              Couldn’t start with the default: {starting.message}
            </div>
          )}
        </>
      )}
    />
  )
}
