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
import { SewFailed, usePersonConfirm } from './sew-ui'

export const RECORDING_UNAVAILABLE = "This server's chat can't record in Apps yet"

const LEVELS = ['none', 'admin', 'moderator', 'member'] as const
type Level = (typeof LEVELS)[number]
const LEVEL_LABEL: Record<Level, string> = { none: 'Nobody', admin: 'Admins', moderator: 'Moderators', member: 'Members' }

const CONFIRM_CODES = new Set(['vault_confirm_required', 'owner_confirm_required'])

/** The server-wide recording rules, in plain words (unknown keys are
 *  worded from the key itself, never shown raw). */
const POLICY_LABEL: Record<string, string> = {
  'recording.enabled': 'Recording in App calls',
  'recording.audio_max_days': 'Longest time audio is kept',
  'recording.transcripts': 'Transcripts',
  'recording.required_allowed': 'Apps may record every call',
}

function policyLabel(key: string): string {
  if (POLICY_LABEL[key]) return POLICY_LABEL[key]
  const words = key.replace(/^recording\./, '').replace(/[._-]+/g, ' ').trim()
  return words ? words[0].toUpperCase() + words.slice(1) : 'A recording rule'
}

function policyValue(key: string, v: unknown): string {
  if (v === null || v === undefined) return 'Not set'
  if (v === true) return 'On'
  if (v === false) return 'Off'
  if (typeof v === 'number' && /days/.test(key)) return `${v} day${v === 1 ? '' : 's'}`
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
  /** Which change is in flight (its button says so). */
  const [pending, setPending] = useState<string | null>(null)
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
    key: string,
  ): Promise<void> => {
    setPending(key)
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
      setPending(null)
    }
  }

  const change = (a: AppRecordingView, c: AppRecordingChange, owner: boolean, key = `${a.app.serviceId}:${Object.keys(c)[0] ?? ''}`): Promise<void> =>
    withConfirm(
      (token) => setAppRecording(a.app.serviceId, c, token),
      { action: owner ? 'app-recording-owner' : 'app-recording', secretId: a.app.serviceId },
      `recording settings for ${a.app.name}`,
      key,
    )

  const lock = (spaceId: string, on: boolean): Promise<void> =>
    withConfirm((token) => setSpaceRecordingLock(spaceId, on, token), { action: 'recording-lock', secretId: spaceId }, `recording lock for Space ${spaceId}`, `lock:${spaceId}`)

  const unavailable = sample || (policies !== null && !policies.flags.appRecording)

  return (
    <div className="space-y-6" data-settings-id="sew.recording">
      {ui}
      {error ? <SewFailed what="Couldn’t change the recording settings." detail={error} next="Nothing changed. Try again in a moment." /> : null}
      {notBuilt ? (
        <SettingsLoadState kind="empty" title="This server doesn’t report recording settings yet">
          Update chat from the Overview tab to set recording in Apps here.
        </SettingsLoadState>
      ) : unavailable ? (
        <SettingsLoadState kind="empty" title={RECORDING_UNAVAILABLE} testId="sew-recording-unavailable">
          Recording in App calls needs chat version 0.9.0 or newer. Update chat from the Overview tab.
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
                  hint={p.want === null ? 'Chat’s own setting.' : p.delivered ? undefined : 'Saved here. Chat picks it up in a moment.'}
                >
                  <span className="type-row text-[var(--color-text-primary)]">{policyValue(p.key, p.want === null ? p.seen : p.want)}</span>
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
              title={`The ${id} App`}
              card
              state={
                <SettingsLoadState kind="error" title="Couldn’t load this App’s recording settings" detail={msg} action={<Button onClick={() => void load()}>Try again</Button>}>
                  Its calls keep their current settings. Try again in a moment.
                </SettingsLoadState>
              }
            />
          ))}
          {apps.map((a) => (
            <AppRow
              key={a.app.serviceId}
              a={a}
              pending={pending}
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

function AppRow({
  a,
  pending,
  canMutate,
  isOwner,
  days,
  setDays,
  change,
  lock,
}: {
  a: AppRecordingView
  pending: string | null
  canMutate: boolean
  isOwner: boolean
  days: string
  setDays: (v: string) => void
  change: (a: AppRecordingView, c: AppRecordingChange, owner: boolean, key?: string) => Promise<void>
  lock: (spaceId: string, on: boolean) => Promise<void>
}): React.JSX.Element {
  const kept = a.audioMaxDays !== null ? `${a.audioMaxDays} days` : a.orgAudioMaxDays !== null ? `The server’s limit (${a.orgAudioMaxDays} days)` : 'The server’s limit'
  const daysN = Number(days)
  const daysOk = /^\d+$/.test(days) && daysN >= 1 && daysN <= 3650 && (a.orgAudioMaxDays === null || daysN <= a.orgAudioMaxDays)
  const daysId = useId()
  const busy = pending !== null
  const id = a.app.serviceId
  return (
    <div data-testid="sew-recording-app">
      <SettingsGroup
        title={a.homeClient ? 'The main chat app' : a.app.name}
        card
        action={
          a.homeClient ? undefined : (
            <span className="type-hint text-[var(--color-text-secondary)]" data-testid="sew-recording-delivered">
              {a.delivered ? 'Up to date in chat' : 'Sending to chat…'}
            </span>
          )
        }
        state={
          a.homeClient ? (
            <SettingsLoadState kind="empty" title="No recording settings here">
              The main chat app shows every Space a person is in, so recording is set for each App instead.
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
                  {pending === `${id}:audioMaxDays` && days ? 'Saving…' : 'Save'}
                </Button>
                {a.audioMaxDays !== null ? (
                  <Button variant="quiet" disabled={busy} onClick={() => void change(a, { audioMaxDays: null }, false, `${id}:reset`)}>
                    {pending === `${id}:reset` ? 'Saving…' : 'Use the server’s limit'}
                  </Button>
                ) : null}
              </form>
            ) : undefined
          }
        >
          <span />
        </SettingRow>
        {a.spaces.map((s, i) => (
          <div key={s.spaceId} data-testid="sew-recording-space">
            <SettingRow
              label={a.spaces.length === 1 ? `${a.app.name}’s Space` : `${a.app.name}’s Space ${i + 1}`}
              hint={
                <>
                  {s.locked
                    ? 'Locked: never recorded, because it holds controlled information.'
                    : `Who may start a recording: ${s.startLevel ? LEVEL_LABEL[s.startLevel as Level] ?? s.startLevel : 'as the App says'}. Every call recorded: ${s.required === null ? 'as the App says' : s.required ? 'yes' : 'no'}.`}
                  <span className="block text-[var(--color-text-muted)]">
                    <span className="sr-only">Space ID </span>
                    <span className="font-mono break-all">{s.spaceId}</span>
                  </span>
                </>
              }
            >
              {isOwner && canMutate ? (
                <Button variant={s.locked ? 'default' : 'quiet'} disabled={busy} onClick={() => void lock(s.spaceId, !s.locked)}>
                  {pending === `lock:${s.spaceId}` ? (s.locked ? 'Lifting…' : 'Locking…') : s.locked ? 'Lift the lock' : 'Lock against recording'}
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
