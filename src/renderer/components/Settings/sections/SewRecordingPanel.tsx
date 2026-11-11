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

import React, { useCallback, useEffect, useId, useState } from 'react'
import { Button, Input, Segmented, Toggle } from '@/components/ui'
import { SettingRow, SettingsGroup, SettingsLoadState } from '../controls/SettingControls'
import { FailedNote } from './AccessDialogs'
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
import { usePersonConfirm } from './sew-ui'

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
    <div className="space-y-6" data-settings-id="sew.recording">
      {ui}
      {error ? <FailedNote what="Couldn’t change the recording settings." detail={error} next="Nothing changed. Try again in a moment." /> : null}
      {notBuilt ? (
        <SettingsLoadState kind="empty" title="This server doesn’t report recording settings yet">
          Update Sew from the Overview tab to set recording in Apps here.
        </SettingsLoadState>
      ) : unavailable ? (
        <SettingsLoadState kind="empty" title={RECORDING_UNAVAILABLE} testId="sew-recording-unavailable">
          Recording in App calls needs Sew 0.9.0 or newer. Update Sew from the Overview tab.
        </SettingsLoadState>
      ) : policies === null && !error ? (
        <SettingsLoadState kind="loading" title="Loading recording settings…" />
      ) : null}
      {!unavailable && policies ? (
        <>
          <SettingsGroup
            title="This server’s rules"
            card
            hint="Everything starts off. Each person in a call still answers their own consent notice, and is never recorded after saying no. The owner or an admin changes these from a terminal on the server."
            testId="sew-recording-policies"
          >
            {policies.policies.map((p) => (
              <div key={p.key} data-testid="sew-recording-policy">
                <SettingRow
                  label={policyLabel(p.key)}
                  hint={<span className="font-mono text-[var(--color-text-muted)]">{p.key}</span>}
                >
                  <span className="type-hint text-[var(--color-text-secondary)] text-right">
                    {p.want === null ? `Chat shows ${policyValue(p.seen)}` : `Set to ${policyValue(p.want)} · ${p.delivered ? 'Delivered' : 'Sending to chat…'}`}
                  </span>
                </SettingRow>
              </div>
            ))}
          </SettingsGroup>
          {apps.length === 0 && Object.keys(rowErrors).length === 0 ? (
            <SettingsGroup
              title="Apps"
              card
              state={
                <SettingsLoadState kind="empty" title="No App uses this server’s chat yet">
                  Apps that use chat show up here with their recording settings.
                </SettingsLoadState>
              }
            />
          ) : null}
          {Object.entries(rowErrors).map(([id, msg]) => (
            <SettingsGroup
              key={id}
              title={id}
              card
              state={
                <SettingsLoadState kind="error" title={`Couldn’t load ${id}’s recording settings`} detail={msg} action={<Button onClick={() => void load()}>Try again</Button>} />
              }
            />
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
        </>
      ) : null}
    </div>
  )
}

/** `recording.audio_max_days` → "Audio max days". */
function policyLabel(key: string): string {
  const last = key.split('.').pop() ?? key
  const words = last.replace(/[_-]+/g, ' ').trim()
  return words ? words[0].toUpperCase() + words.slice(1) : key
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
  const kept = a.audioMaxDays !== null ? `${a.audioMaxDays} days` : a.orgAudioMaxDays !== null ? `The server’s limit (${a.orgAudioMaxDays} days)` : 'The server’s limit'
  const daysN = Number(days)
  const daysOk = /^\d+$/.test(days) && daysN >= 1 && daysN <= 3650 && (a.orgAudioMaxDays === null || daysN <= a.orgAudioMaxDays)
  const daysId = useId()
  return (
    <div data-testid="sew-recording-app">
      <SettingsGroup
        title={a.app.name}
        card
        hint={a.homeClient ? undefined : a.app.serviceId !== a.app.name ? `App ${a.app.serviceId}` : undefined}
        action={
          a.homeClient ? undefined : (
            <span className="type-hint text-[var(--color-text-secondary)]" data-testid="sew-recording-delivered">
              {a.delivered ? 'Delivered' : 'Sending to Sew'}
            </span>
          )
        }
        state={
          a.homeClient ? (
            <SettingsLoadState kind="empty" title="Not for the home client">
              It sees every Space a person is in, so it takes no recording settings.
            </SettingsLoadState>
          ) : undefined
        }
      >
        <SettingRow label="Who may start a recording" hint="Everyone in the call is asked first." stack>
          <Segmented<Level>
            aria-label={`Who may start a recording in ${a.app.name}`}
            value={(LEVELS as readonly string[]).includes(a.startLevel) ? (a.startLevel as Level) : 'none'}
            onChange={(l) => void change(a, { startLevel: l }, false)}
            options={LEVELS.map((l) => ({ value: l, label: LEVEL_LABEL[l] }))}
            disabled={!canMutate || busy}
          />
        </SettingRow>
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
        <SettingRow
          label="Audio kept"
          hint={kept}
          below={
            canMutate ? (
              <form
                className="flex flex-wrap items-center gap-2"
                onSubmit={(e) => {
                  e.preventDefault()
                  if (daysOk && !busy) void change(a, { audioMaxDays: daysN }, false).then(() => setDays(''))
                }}
              >
                <label htmlFor={daysId} className="sr-only">
                  Days to keep audio in {a.app.name}
                </label>
                <Input
                  id={daysId}
                  aria-label={`Days to keep audio in ${a.app.name}`}
                  inputMode="numeric"
                  placeholder="Days"
                  value={days}
                  onChange={(e) => setDays(e.target.value.trim())}
                  className="w-24"
                />
                <Button type="submit" disabled={busy || !daysOk}>
                  Save
                </Button>
                {a.audioMaxDays !== null ? (
                  <Button variant="quiet" disabled={busy} onClick={() => void change(a, { audioMaxDays: null }, false)}>
                    Use the server&apos;s limit
                  </Button>
                ) : null}
              </form>
            ) : undefined
          }
        >
          <span />
        </SettingRow>
        {a.spaces.map((s) => (
          <div key={s.spaceId} data-testid="sew-recording-space">
            <SettingRow
              label={`Space ${s.spaceId}`}
              hint={
                s.locked
                  ? 'Locked: no recording (controlled information)'
                  : `Starts: ${s.startLevel ? LEVEL_LABEL[s.startLevel as Level] ?? s.startLevel : 'the App’s rule'} · Every call: ${s.required === null ? 'the App’s rule' : s.required ? 'on' : 'off'}`
              }
            >
              {isOwner && canMutate ? (
                <Button variant={s.locked ? 'default' : 'quiet'} disabled={busy} onClick={() => void lock(s.spaceId, !s.locked)}>
                  {s.locked ? 'Lift the lock' : 'Lock against recording'}
                </Button>
              ) : null}
            </SettingRow>
          </div>
        ))}
      </SettingsGroup>
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
    <SettingRow label={label.replace("App's", 'App’s')} hint={!on && !isOwner ? 'Only the owner turns this on' : undefined}>
      <Toggle aria-label={label} checked={on} disabled={!may || busy} onChange={onChange} size="sm" />
    </SettingRow>
  )
}
