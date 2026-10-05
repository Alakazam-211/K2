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

import { useEffect, useRef, useState } from 'react'
import type { ZenAgentRow } from '@/lib/zen/zen-data'
import type { ZenWidgetProps } from '../zen-registry'
import { ZenConversation } from './ZenConversationWidget'
import { ZenWidgetStyles, initials, useZenRows } from './zen-widget-kit'

export const ZEN_GARDEN_EMPTY_TITLE = 'This Garden is empty.'
export const ZEN_GARDEN_EMPTY_ASK = 'Ask your agents to add things to this Garden.'
export const ZEN_NO_LOCAL_AGENTS = 'No agents on this computer yet. Add one from Home.'
export const ZEN_GARDEN_HINT = '(Use the k2-zen skill and k2 zen garden; check with k2 zen validate.)'

/** The draft Ask my agent puts in the message box (never sent). */
export function zenGardenAskDraft(garden: { id: string; name: string }): string {
  return `In my Zen Garden "${garden.name}" (id ${garden.id}), please add: \n${ZEN_GARDEN_HINT}`
}

type Phase = { kind: 'empty' } | { kind: 'choosing'; rows: ZenAgentRow[] | null; error: string | null } | { kind: 'talking'; address: string }

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
              className="flex w-full items-center gap-3 text-left"
              style={{ padding: '6px 10px', borderRadius: 'calc(var(--zen-radius) - 4px)', color: 'var(--zen-text)' }}
            >
              <span
                aria-hidden
                className="flex flex-shrink-0 items-center justify-center"
                style={{
                  width: 26,
                  height: 26,
                  borderRadius: 999,
                  background: 'var(--zen-bubble-agent)',
                  color: 'var(--zen-bubble-agent-text)',
                  fontWeight: 600,
                  fontSize: '0.8em',
                }}
              >
                {initials(r.label)}
              </span>
              <span className="truncate">{r.label}</span>
            </button>
          ))}
        </div>
      )}
      <button
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

export function ZenGardenEmptyWidget({ bridge, decl }: ZenWidgetProps): React.JSX.Element {
  const [phase, setPhase] = useState<Phase>({ kind: 'empty' })
  const rows = useZenRows(bridge)
  const drafted = useRef<string | null>(null)
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
    bridge.call('compose.draft', row.address, zenGardenAskDraft(garden))
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

  const pick = (r: ZenAgentRow): void => {
    setPhase({ kind: 'talking', address: r.address })
    void Promise.resolve()
      .then(() => bridge.call('conversation.open', r.address))
      .catch((err: unknown) => console.warn('[zen] Ask my agent: open failed:', err))
  }

  if (row) {
    return (
      <div className="flex h-full min-h-0 w-full flex-col" data-zen-widget="garden-empty" data-zen-widget-id={decl.id} data-zen-asking={row.address}>
        <ZenWidgetStyles />
        <ZenConversation bridge={bridge} row={row} />
      </div>
    )
  }
  return (
    <div
      className="flex h-full min-h-0 w-full flex-col items-center justify-center text-center"
      data-zen-widget="garden-empty"
      data-zen-widget-id={decl.id}
      style={{ padding: 24, color: 'var(--zen-text)' }}
    >
      <ZenWidgetStyles />
      <div data-zen-garden-empty-title="" style={{ fontSize: '1.25em', fontWeight: 600 }}>
        {ZEN_GARDEN_EMPTY_TITLE}
      </div>
      <div data-zen-garden-empty-ask="" style={{ marginTop: 6, color: 'var(--zen-text-muted)' }}>
        {ZEN_GARDEN_EMPTY_ASK}
      </div>
      {phase.kind === 'choosing' ? (
        <Chooser rows={phase.rows} error={phase.error} onPick={pick} onCancel={() => setPhase({ kind: 'empty' })} />
      ) : (
        <button
          type="button"
          data-zen-ask-my-agent=""
          disabled={phase.kind === 'talking'}
          onClick={ask}
          style={{
            marginTop: 16,
            padding: '8px 18px',
            borderRadius: 999,
            background: 'var(--zen-accent)',
            color: 'var(--zen-accent-text)',
            fontWeight: 600,
          }}
        >
          Ask my agent
        </button>
      )}
    </div>
  )
}
