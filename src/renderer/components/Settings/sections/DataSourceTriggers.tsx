// Triggers… for one source (contract-sources-v1 §8, §9). Owner: B2.
//
// A trigger wakes one agent on this server when the source changes (hard
// changes only). People add, edit, approve, pause, resume, remove and send
// a test; agents only propose. The list shows the agent's CURRENT handle
// (the trigger keeps the workspace id), "missing workspace" when it's gone,
// and the last fires and suppressions. Re-reads come from the section
// (`sources_changed` bumps on every fire).

import React, { useCallback, useEffect, useState } from 'react'
import type { ServerScope } from '@/kessel/server-scope'
import type { FireRow, TriggerOn, TriggerView } from '@/lib/sources/sources-types'
import {
  addTrigger,
  approveTrigger,
  editTrigger,
  listTriggers,
  pauseTrigger,
  removeTrigger,
  resumeTrigger,
  testTrigger,
  triggerLog,
} from '@/lib/sources/sources-api'
import { useProjectsStore } from '@/stores/projects'
import { ago, btn, btnPrimary, danger, every, input, makerLabel, muted, onLabel, sourcesErrorText } from './data-sources-ui'

type OnKind = TriggerOn['kind']

const ON_CHOICES: Array<{ kind: OnKind; label: string }> = [
  { kind: 'item_went_crit', label: 'An item goes red' },
  { kind: 'went_crit', label: 'The source goes red' },
  { kind: 'went_warn', label: 'The source goes amber' },
  { kind: 'recovered', label: 'The source recovers' },
  { kind: 'stale_for', label: 'No good check for…' },
]

function fireText(f: FireRow): string {
  if (f.outcome === 'suppressed') return `held (${(f.suppressReason ?? '').replace('_', ' ')})`
  if (f.outcome === 'coalesced') return 'sent with the next note'
  if (f.outcome === 'test') return `test: ${f.detail ?? ''}`
  if (f.outcome === 'delivered') return f.woke ? 'woke the agent' : 'delivered'
  if (f.outcome === 'inbox') return 'left in its inbox'
  if (f.outcome === 'undeliverable') return `couldn't deliver (${f.detail ?? 'unknown'})`
  return f.outcome
}

function TriggerFires({ scope, id, rev }: { scope: ServerScope; id: string; rev: number }): React.JSX.Element | null {
  const [fires, setFires] = useState<FireRow[] | null>(null)
  useEffect(() => {
    let live = true
    triggerLog(scope, id, 5)
      .then((f) => live && setFires(f))
      .catch(() => live && setFires([]))
    return () => {
      live = false
    }
  }, [scope, id, rev])
  if (!fires || fires.length === 0) return null
  const now = Math.floor(Date.now() / 1000)
  return (
    <ul className={`text-[10px] ${muted} pl-3`} data-testid="sources-trigger-fires">
      {fires.map((f) => (
        <li key={f.id}>
          {ago(now - f.at)} · {fireText(f)}
          {f.itemKey ? ` · ${f.itemKey}` : ''}
        </li>
      ))}
    </ul>
  )
}

interface FormState {
  kind: OnKind
  staleMins: number
  agent: string
  note: string
  cooldownMins: number
  dailyCap: number
}

const EMPTY_FORM: FormState = { kind: 'item_went_crit', staleMins: 30, agent: '', note: '', cooldownMins: 30, dailyCap: 12 }

function onOf(f: FormState): TriggerOn {
  return f.kind === 'stale_for' ? { kind: 'stale_for', secs: f.staleMins * 60 } : ({ kind: f.kind } as TriggerOn)
}

function TriggerForm({
  initial,
  agents,
  submitLabel,
  onSubmit,
  onCancel,
}: {
  initial: FormState
  agents: Array<{ id: string; label: string }>
  submitLabel: string
  onSubmit: (f: FormState) => Promise<void>
  onCancel: () => void
}): React.JSX.Element {
  const [f, setF] = useState<FormState>(initial)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const set = <K extends keyof FormState>(k: K, v: FormState[K]): void => setF((s) => ({ ...s, [k]: v }))
  return (
    <form
      className="space-y-1.5 border border-[var(--color-border)] p-2 max-w-lg"
      data-testid="sources-trigger-form"
      onSubmit={(e) => {
        e.preventDefault()
        setBusy(true)
        setError(null)
        onSubmit(f)
          .catch((err) => setError(sourcesErrorText(err)))
          .finally(() => setBusy(false))
      }}
    >
      <label className={`block text-[11px] ${muted}`}>
        When
        <select className={`${input} mt-0.5`} value={f.kind} onChange={(e) => set('kind', e.currentTarget.value as OnKind)} data-testid="sources-trigger-on">
          {ON_CHOICES.map((c) => (
            <option key={c.kind} value={c.kind}>
              {c.label}
            </option>
          ))}
        </select>
      </label>
      {f.kind === 'stale_for' && (
        <label className={`block text-[11px] ${muted}`}>
          Minutes with no good check (5 or more)
          <input
            type="number"
            min={5}
            max={10080}
            className={`${input} mt-0.5`}
            value={f.staleMins}
            onChange={(e) => set('staleMins', Number(e.currentTarget.value))}
          />
        </label>
      )}
      <label className={`block text-[11px] ${muted}`}>
        Wake this agent
        <select className={`${input} mt-0.5`} value={f.agent} onChange={(e) => set('agent', e.currentTarget.value)} data-testid="sources-trigger-agent">
          <option value="">Choose…</option>
          {agents.map((a) => (
            <option key={a.id} value={a.id}>
              {a.label}
            </option>
          ))}
        </select>
      </label>
      <label className={`block text-[11px] ${muted}`}>
        Your note to the agent (optional, plain text)
        <input
          className={`${input} mt-0.5`}
          maxLength={500}
          value={f.note}
          placeholder="check the relay first"
          onChange={(e) => set('note', e.currentTarget.value)}
          data-testid="sources-trigger-note"
        />
      </label>
      <div className="flex gap-2">
        <label className={`block text-[11px] ${muted}`}>
          At most once every (minutes)
          <input
            type="number"
            min={5}
            max={1440}
            className={`${input} mt-0.5`}
            value={f.cooldownMins}
            onChange={(e) => set('cooldownMins', Number(e.currentTarget.value))}
          />
        </label>
        <label className={`block text-[11px] ${muted}`}>
          At most per day
          <input
            type="number"
            min={1}
            max={48}
            className={`${input} mt-0.5`}
            value={f.dailyCap}
            onChange={(e) => set('dailyCap', Number(e.currentTarget.value))}
          />
        </label>
      </div>
      <div className="flex gap-1.5">
        <button type="submit" className={btnPrimary} disabled={busy || f.agent === ''} data-testid="sources-trigger-save">
          {submitLabel}
        </button>
        <button type="button" className={btn} onClick={onCancel}>
          Cancel
        </button>
      </div>
      {error && (
        <p className={`text-[11px] ${danger}`} role="alert">
          {error}
        </p>
      )}
    </form>
  )
}

export function DataSourceTriggers({ scope, source, rev }: { scope: ServerScope; source: string; rev: number }): React.JSX.Element {
  const [rows, setRows] = useState<TriggerView[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [note, setNote] = useState<string | null>(null)
  const [adding, setAdding] = useState(false)
  const [editing, setEditing] = useState<string | null>(null)
  const projects = useProjectsStore((s) => s.projects)
  const agents = projects.map((p) => ({ id: p.id, label: p.handle || p.name }))

  const load = useCallback(() => {
    listTriggers(scope, source)
      .then((r) => {
        setRows(r)
        setError(null)
      })
      .catch((e) => setError(sourcesErrorText(e)))
  }, [scope, source])

  useEffect(load, [load, rev])

  const act = async (fn: () => Promise<unknown>, done?: string): Promise<void> => {
    setError(null)
    setNote(null)
    try {
      await fn()
      if (done) setNote(done)
      load()
    } catch (e) {
      setError(sourcesErrorText(e))
    }
  }

  return (
    <div className="space-y-2" data-testid="sources-triggers">
      <p className={`text-[11px] ${muted}`}>
        A trigger wakes an agent on this server with a short note when this source changes. It never fires while the source
        is muted, paused or simulated, and at most once per problem.
      </p>
      {rows && rows.length === 0 && <p className={`text-[11px] ${muted}`}>No triggers yet.</p>}
      {(rows ?? []).map((t) => (
        <div key={t.id} className="border border-[var(--color-border)] p-1.5 space-y-1" data-testid="sources-trigger-row">
          <div className="flex flex-wrap items-center gap-1.5 text-[11px]">
            <span className="text-[var(--color-text-primary)]">
              When it {onLabel(t.on)}, wake{' '}
              {t.agent.missing ? <span className={danger}>missing workspace</span> : <strong>{t.agent.handle}</strong>}
            </span>
            <span className={muted}>
              · {t.status === 'proposed' ? `proposed by ${makerLabel(t.madeBy, t.agent.handle)}` : t.status}
              {' · '}at most every {every(t.cooldownSecs)}, {t.dailyCap}/day · {t.firesToday} today
            </span>
          </div>
          {t.note && <p className={`text-[10px] ${muted}`}>Note: {t.note}</p>}
          {t.status === 'undeliverable' && (
            <p className={`text-[10px] ${danger}`}>Its workspace is gone. Edit it to wake another agent, or remove it.</p>
          )}
          <div className="flex flex-wrap gap-1">
            {t.status === 'proposed' && (
              <button type="button" className={btn} onClick={() => void act(() => approveTrigger(scope, t.id))} data-testid="sources-trigger-approve">
                Approve
              </button>
            )}
            {(t.status === 'active' || t.status === 'undeliverable') && (
              <button type="button" className={btn} onClick={() => void act(() => pauseTrigger(scope, t.id))}>
                Pause
              </button>
            )}
            {t.status === 'paused' && (
              <button type="button" className={btn} onClick={() => void act(() => resumeTrigger(scope, t.id))}>
                Resume
              </button>
            )}
            <button type="button" className={btn} onClick={() => setEditing(editing === t.id ? null : t.id)}>
              Edit
            </button>
            <button
              type="button"
              className={btn}
              disabled={t.agent.missing}
              onClick={() =>
                void act(async () => {
                  const r = await testTrigger(scope, t.id)
                  setNote(
                    r.outcome === 'delivered'
                      ? `Test sent${r.woke ? ' (woke the agent)' : ''}.`
                      : r.outcome === 'inbox'
                        ? 'Test left in the agent’s inbox.'
                        : 'The test couldn’t be delivered.',
                  )
                })
              }
              data-testid="sources-trigger-test"
            >
              Send test
            </button>
            <button type="button" className={btn} onClick={() => void act(() => removeTrigger(scope, t.id), 'Trigger removed.')}>
              Remove
            </button>
          </div>
          {editing === t.id && (
            <TriggerForm
              initial={{
                kind: t.on.kind,
                staleMins: t.on.kind === 'stale_for' ? Math.round(t.on.secs / 60) : 30,
                agent: t.agent.projectId,
                note: t.note ?? '',
                cooldownMins: Math.round(t.cooldownSecs / 60),
                dailyCap: t.dailyCap,
              }}
              agents={agents}
              submitLabel="Save"
              onCancel={() => setEditing(null)}
              onSubmit={async (f) => {
                await editTrigger(scope, t.id, {
                  on: onOf(f),
                  agent: f.agent,
                  note: f.note.trim() === '' ? (null as unknown as string) : f.note.trim(),
                  cooldownSecs: f.cooldownMins * 60,
                  dailyCap: f.dailyCap,
                })
                setEditing(null)
                load()
              }}
            />
          )}
          <TriggerFires scope={scope} id={t.id} rev={rev} />
        </div>
      ))}
      {adding ? (
        <TriggerForm
          initial={EMPTY_FORM}
          agents={agents}
          submitLabel="Add trigger"
          onCancel={() => setAdding(false)}
          onSubmit={async (f) => {
            await addTrigger(scope, {
              source,
              on: onOf(f),
              agent: f.agent,
              ...(f.note.trim() ? { note: f.note.trim() } : {}),
              cooldownSecs: f.cooldownMins * 60,
              dailyCap: f.dailyCap,
            })
            setAdding(false)
            load()
          }}
        />
      ) : (
        <button type="button" className={btn} onClick={() => setAdding(true)} data-testid="sources-trigger-add">
          Add a trigger
        </button>
      )}
      {note && <p className={`text-[11px] ${muted}`}>{note}</p>}
      {error && (
        <p className={`text-[11px] ${danger}`} role="alert">
          {error}
        </p>
      )}
    </div>
  )
}
