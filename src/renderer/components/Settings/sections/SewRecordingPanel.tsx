// Settings → Chat Hosting → Recording (Sew 0.9.0; K2 PRD
// prd-k2-bobbin-in-apps-v1 K-c, vs-live B22/B23).
//
// Display and gestures only: the daemon owns every rule. It reads
// `GET /cli/sew/recording/policies` first; a Sew without `appRecording`
// shows one line and nothing else. Then one row per App with a Sew
// settings row (`GET /cli/sew/home`) or a bound Space
// (`GET /cli/sew/app-spaces`), each from `GET /cli/app/recording?app=`.
// A change is `POST /cli/app/recording/set`; when the daemon answers
// `vault_confirm_required` (a widening, or `required` / `server_read` on,
// the owner only) the person confirms and the same change goes again. The
// per-Space CUI/FCI lock is the owner's, in person. Never `fetchProjects()`.

import React, { useCallback, useEffect, useState } from 'react'
import { Button, Callout, Segmented, Toggle } from '@/components/ui'
import { httpCodeOf, httpDetailsOf } from '@/lib/daemon-http-error'
import {
  fetchAppRecording,
  fetchSewAppSpaces,
  fetchSewHomeAppIds,
  fetchSewRecordingPolicies,
  isNotBuilt,
  setAppRecording,
  setSpaceRecordingLock,
  sewErrorMessage,
  type AppRecordingChange,
  type AppRecordingView,
  type SewConfirmAction,
  type SewRecordingPolicies,
} from './sew-api'
import { muted, secondary, usePersonConfirm } from './sew-ui'

export const RECORDING_UNAVAILABLE = "This server's chat can't record in Apps yet"

const LEVELS = ['none', 'admin', 'moderator', 'member'] as const
type Level = (typeof LEVELS)[number]
const LEVEL_LABEL: Record<Level, string> = { none: 'Nobody', admin: 'Admins', moderator: 'Moderators', member: 'Members' }

const CONFIRM_CODES = new Set(['vault_confirm_required', 'owner_confirm_required'])

function policyValue(v: unknown): string {
  if (v === null || v === undefined) return '–'
  if (v === true) return 'On'
  if (v === false) return 'Off'
  return String(v)
}

export interface RecordingPanelProps {
  sample: boolean
  canMutate: boolean
  isOwner: boolean
}

export function RecordingPanel({ sample, canMutate, isOwner }: RecordingPanelProps): React.JSX.Element {
  const { confirm, ui } = usePersonConfirm()
  const [policies, setPolicies] = useState<SewRecordingPolicies | null>(null)
  const [apps, setApps] = useState<AppRecordingView[]>([])
  const [rowErrors, setRowErrors] = useState<Record<string, string>>({})
  const [notBuilt, setNotBuilt] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [days, setDays] = useState<Record<string, string>>({})

  const load = useCallback(async () => {
    if (sample) return
    try {
      const p = await fetchSewRecordingPolicies()
      setPolicies(p)
      setError(null)
      if (!p.flags.appRecording) return
      const [home, spaces] = await Promise.all([fetchSewHomeAppIds(), fetchSewAppSpaces()])
      const ids = [...new Set([...home, ...spaces.map((s) => s.app)])]
      const views: AppRecordingView[] = []
      const errs: Record<string, string> = {}
      for (const id of ids) {
        try {
          views.push(await fetchAppRecording(id))
        } catch (e) {
          errs[id] = sewErrorMessage(e)
        }
      }
      setApps(views)
      setRowErrors(errs)
    } catch (e) {
      if (isNotBuilt(e)) setNotBuilt(true)
      else setError(sewErrorMessage(e))
    }
  }, [sample])

  useEffect(() => {
    void load()
  }, [load])

  /** Send; on a confirm request, the person confirms and it goes again. */
  const withConfirm = async (
    send: (token?: string) => Promise<unknown>,
    fallback: { action: SewConfirmAction; secretId: string },
    label: string,
  ): Promise<void> => {
    setBusy(true)
    setError(null)
    try {
      try {
        await send()
      } catch (e) {
        if (!CONFIRM_CODES.has(httpCodeOf(e) ?? '')) throw e
        const d = httpDetailsOf(e)
        const action = (typeof d?.action === 'string' ? d.action : fallback.action) as SewConfirmAction
        const secretId = typeof d?.serviceId === 'string' ? d.serviceId : fallback.secretId
        const token = await confirm(action, secretId, label)
        if (!token) return
        await send(token)
      }
      await load()
    } catch (e) {
      setError(sewErrorMessage(e))
    } finally {
      setBusy(false)
    }
  }

  const change = (a: AppRecordingView, c: AppRecordingChange, owner: boolean): Promise<void> =>
    withConfirm(
      (token) => setAppRecording(a.app.serviceId, c, token),
      { action: owner ? 'app-recording-owner' : 'app-recording', secretId: a.app.serviceId },
      `recording settings for ${a.app.name}`,
    )

  const lock = (spaceId: string, on: boolean): Promise<void> =>
    withConfirm((token) => setSpaceRecordingLock(spaceId, on, token), { action: 'recording-lock', secretId: spaceId }, `recording lock for Space ${spaceId}`)

  const unavailable = sample || (policies !== null && !policies.flags.appRecording)

  return (
    <div className="max-w-2xl space-y-4" data-settings-id="sew.recording">
      <h3 className="type-heading text-[var(--color-text-primary)]">Recording</h3>
      {ui}
      {notBuilt ? <p className={`type-hint ${muted}`}>This server does not report recording settings yet.</p> : null}
      {error ? <Callout tone="error">{error}</Callout> : null}
      {unavailable ? (
        <p className={`type-row ${secondary}`} data-testid="sew-recording-unavailable">
          {RECORDING_UNAVAILABLE}. It needs Sew 0.9.0 (Upgrade on the Chat / Voice Hosting page).
        </p>
      ) : null}
      {!unavailable && policies ? (
        <>
          <p className={`type-hint ${secondary}`}>
            Everything starts off. Each person in a call still answers their own consent notice, and is never recorded after saying no.
          </p>
          <section className="space-y-1" data-testid="sew-recording-policies">
            <h4 className="type-name text-[var(--color-text-primary)]">This server&apos;s policies</h4>
            <table className="w-full type-hint">
              <thead>
                <tr className={muted}>
                  <th className="text-left font-normal">Policy</th>
                  <th className="text-left font-normal">Asked for</th>
                  <th className="text-left font-normal">Sew shows</th>
                  <th className="text-left font-normal" />
                </tr>
              </thead>
              <tbody>
                {policies.policies.map((p) => (
                  <tr key={p.key} data-testid="sew-recording-policy">
                    <td className="font-mono text-[var(--color-text-primary)]">{p.key}</td>
                    <td>{policyValue(p.want)}</td>
                    <td>{policyValue(p.seen)}</td>
                    <td className={muted}>{p.want === null ? '' : p.delivered ? 'Delivered' : 'Sending to Sew'}</td>
                  </tr>
                ))}
              </tbody>
            </table>
            <p className={`type-hint ${muted}`}>
              The owner or an admin changes these in person: <code className="font-mono">k2 hostchat recording policy set KEY VALUE</code>.
            </p>
          </section>
          <section className="space-y-3">
            <h4 className="type-name text-[var(--color-text-primary)]">Apps</h4>
            {apps.length === 0 && Object.keys(rowErrors).length === 0 ? <p className={`type-hint ${muted}`}>No App uses this server&apos;s chat yet.</p> : null}
            {Object.entries(rowErrors).map(([id, msg]) => (
              <p key={id} className={`type-hint ${muted}`}>
                <span className="font-mono">{id}</span>: {msg}
              </p>
            ))}
            {apps.map((a) => (
              <AppRow
                key={a.app.serviceId}
                a={a}
                busy={busy}
                canMutate={canMutate}
                isOwner={isOwner}
                days={days[a.app.serviceId] ?? ''}
                setDays={(v) => setDays((d) => ({ ...d, [a.app.serviceId]: v }))}
                change={change}
                lock={lock}
              />
            ))}
          </section>
        </>
      ) : null}
    </div>
  )
}

function AppRow({
  a,
  busy,
  canMutate,
  isOwner,
  days,
  setDays,
  change,
  lock,
}: {
  a: AppRecordingView
  busy: boolean
  canMutate: boolean
  isOwner: boolean
  days: string
  setDays: (v: string) => void
  change: (a: AppRecordingView, c: AppRecordingChange, owner: boolean) => Promise<void>
  lock: (spaceId: string, on: boolean) => Promise<void>
}): React.JSX.Element {
  const kept = a.audioMaxDays !== null ? `${a.audioMaxDays} days` : a.orgAudioMaxDays !== null ? `The server's limit (${a.orgAudioMaxDays} days)` : "The server's limit"
  const daysN = Number(days)
  const daysOk = /^\d+$/.test(days) && daysN >= 1 && daysN <= 3650 && (a.orgAudioMaxDays === null || daysN <= a.orgAudioMaxDays)
  return (
    <div className="space-y-2 border-t border-[var(--color-border)] pt-2" data-testid="sew-recording-app">
      <div className="flex items-center gap-2">
        <span className="type-name text-[var(--color-text-primary)]">{a.app.name}</span>
        <span className={`font-mono type-hint ${muted}`}>{a.app.serviceId}</span>
        <span className="flex-1" />
        {a.homeClient ? null : (
          <span className={`type-hint ${muted}`} data-testid="sew-recording-delivered">
            {a.delivered ? 'Delivered' : 'Sending to Sew'}
          </span>
        )}
      </div>
      {a.homeClient ? (
        <p className={`type-hint ${muted}`}>Not for the home client: it sees every Space a person is in, so it takes no recording settings.</p>
      ) : (
        <>
          <div className="flex flex-wrap items-center gap-2 type-row">
            <span className={`w-48 ${secondary}`}>Who may start a recording</span>
            <Segmented<Level>
              aria-label={`Who may start a recording in ${a.app.name}`}
              value={(LEVELS as readonly string[]).includes(a.startLevel) ? (a.startLevel as Level) : 'none'}
              onChange={(l) => void change(a, { startLevel: l }, false)}
              options={LEVELS.map((l) => ({ value: l, label: LEVEL_LABEL[l] }))}
              disabled={!canMutate || busy}
            />
          </div>
          <OwnerSwitch
            label="Record every call with two or more people"
            on={a.required}
            busy={busy}
            canMutate={canMutate}
            isOwner={isOwner}
            onChange={(v) => void change(a, { required: v }, v)}
          />
          <OwnerSwitch
            label="Let the App's server read transcripts"
            on={a.serverRead}
            busy={busy}
            canMutate={canMutate}
            isOwner={isOwner}
            onChange={(v) => void change(a, { serverRead: v }, v)}
          />
          <div className="flex flex-wrap items-center gap-2 type-row">
            <span className={`w-48 ${secondary}`}>Audio kept</span>
            <span className="text-[var(--color-text-primary)]">{kept}</span>
            {canMutate ? (
              <>
                <input
                  aria-label={`Days to keep audio in ${a.app.name}`}
                  inputMode="numeric"
                  placeholder="days"
                  value={days}
                  onChange={(e) => setDays(e.target.value.trim())}
                  className="w-20 border border-[var(--color-border)] bg-transparent px-2 py-0.5 type-control font-mono"
                />
                <Button disabled={busy || !daysOk} onClick={() => void change(a, { audioMaxDays: daysN }, false).then(() => setDays(''))}>
                  Save
                </Button>
                {a.audioMaxDays !== null ? (
                  <Button disabled={busy} onClick={() => void change(a, { audioMaxDays: null }, false)}>
                    Use the server&apos;s limit
                  </Button>
                ) : null}
              </>
            ) : null}
          </div>
          {a.spaces.length > 0 ? (
            <ul className="space-y-1">
              {a.spaces.map((s) => (
                <li key={s.spaceId} className="flex flex-wrap items-center gap-2 type-hint" data-testid="sew-recording-space">
                  <span className={`font-mono ${muted}`}>Space {s.spaceId}</span>
                  <span className={secondary}>
                    {s.locked
                      ? 'Locked: no recording (CUI/FCI)'
                      : `starts: ${s.startLevel ?? "the App's rule"} · required: ${s.required === null ? "the App's rule" : s.required ? 'on' : 'off'}`}
                  </span>
                  {isOwner && canMutate ? (
                    <Button className="ml-auto" disabled={busy} onClick={() => void lock(s.spaceId, !s.locked)}>
                      {s.locked ? 'Lift the lock' : 'Lock against recording'}
                    </Button>
                  ) : null}
                </li>
              ))}
            </ul>
          ) : null}
        </>
      )}
    </div>
  )
}

/** `required` / `server_read`: only the owner turns one on; anyone who may
 *  change settings turns it off. */
function OwnerSwitch({
  label,
  on,
  busy,
  canMutate,
  isOwner,
  onChange,
}: {
  label: string
  on: boolean
  busy: boolean
  canMutate: boolean
  isOwner: boolean
  onChange: (v: boolean) => void
}): React.JSX.Element {
  const may = canMutate && (on || isOwner)
  return (
    <div className="flex flex-wrap items-center gap-2 type-row">
      <span className={`w-48 ${secondary}`}>{label}</span>
      <Toggle aria-label={label} checked={on} disabled={!may || busy} onChange={onChange} size="sm" />
      {!on && !isOwner ? <span className={`type-hint ${muted}`}>Only the owner turns this on</span> : null}
    </div>
  )
}
