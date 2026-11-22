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
//
// The two server switches (prd-home-call-recording-v1 §3, HR-11): "Recording
// on this server" (`recording.enabled`) and "Record calls in DMs"
// (`recording.dm_enabled`), both off until a person turns them on. Each goes
// to `POST /cli/sew/recording/policy`; the daemon takes an Owner or Admin in
// person, with a fresh confirm (`recording-policy`, bound to this install),
// and refuses an agent `person_only`. Neither is ever turned on by K2 itself.

import React, { useCallback, useEffect, useId, useState } from 'react'
import { Button, Input, Segmented, Toggle } from '@/components/ui'
import { SettingRow, SettingsGroup, SettingsLoadState } from '../controls/SettingControls'
import { httpCodeOf, httpDetailsOf } from '@/lib/daemon-http-error'
import {
  setSewRecordingPolicy,
  fetchAppRecording,
  fetchSewAppSpaces,
  fetchSewHomeAppIds,
  fetchSewRecordingPolicies,
  isNotBuilt,
  setAppRecording,
  setSpaceRecordingLock,
  sewErrorMessage,
  sewFailure,
  type SewFailure,
  type AppRecordingChange,
  type AppRecordingView,
  type SewConfirmAction,
  type SewRecordingPolicies,
} from './sew-api'
import { SewFailed, usePersonConfirm } from './sew-ui'
import { CLI_NAME } from '@/lib/brand'

export const RECORDING_UNAVAILABLE = "This server's chat can't record in Apps yet"

const LEVELS = ['none', 'admin', 'moderator', 'member'] as const
type Level = (typeof LEVELS)[number]
const LEVEL_LABEL: Record<Level, string> = { none: 'Nobody', admin: 'Admins', moderator: 'Moderators', member: 'Members' }

const CONFIRM_CODES = new Set(['vault_confirm_required', 'owner_confirm_required'])

/** The two switches a person turns on here (HR-11): their keys, labels and
 *  hints. Everything else stays a read-only row. */
export const RECORDING_SWITCHES = [
  {
    key: 'recording.enabled',
    label: 'Recording on this server',
    hint: 'Lets people record calls in voice rooms, Apps and (with the next switch) DMs. Everyone in a call is told first. Audio is stored on this server, sealed with its keys: it is not end-to-end encrypted on disk. You are responsible for the recording laws where people are (for example, two-party consent).',
  },
  {
    key: 'recording.dm_enabled',
    label: 'Record calls in DMs',
    hint: 'Either person in a DM call may press Record or Stop; the other person is told and may leave. The transcript is kept in the DM, which only the two people read (chat’s compliance tools can still reach it). The audio is kept 30 days, or less if this server keeps audio for less.',
  },
] as const

/** The org rule that would also record every DM call (HR-15). */
export const REQUIRED_ALSO_DMS = 'Also records every DM call while “Record calls in DMs” is on.'

/** The server-wide recording rules, in plain words (unknown keys are
 *  worded from the key itself, never shown raw). */
const POLICY_LABEL: Record<string, string> = {
  'recording.enabled': 'Recording on this server',
  'recording.dm_enabled': 'Record calls in DMs',
  'recording.required': 'Record every call with two or more people',
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
  /** This install's id: the policy confirm is bound to it. */
  installId?: string | null
}

/** A policy's value as the person sees it: what K2 asked for, else what Sew
 *  shows; never there = off (both switches start off). */
export function policyOn(p: { want: unknown; seen: unknown } | undefined): boolean {
  if (!p) return false
  return (p.want ?? p.seen) === true
}

export function RecordingPanel({ sample, canMutate, isOwner, installId = null }: RecordingPanelProps): React.JSX.Element {
  const { confirm, ui } = usePersonConfirm()
  const [policies, setPolicies] = useState<SewRecordingPolicies | null>(null)
  const [apps, setApps] = useState<AppRecordingView[]>([])
  const [rowErrors, setRowErrors] = useState<Record<string, string>>({})
  const [notBuilt, setNotBuilt] = useState(false)
  const [error, setError] = useState<SewFailure | null>(null)
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
      else setError(sewFailure(e))
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
      setError(sewFailure(e))
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

  const setPolicy = (key: string, on: boolean, label: string): Promise<void> =>
    withConfirm(
      (token) => setSewRecordingPolicy({ [key]: on }, token),
      { action: 'recording-policy', secretId: installId || 'none' },
      `${label}: ${on ? 'on' : 'off'}`,
      `policy:${key}`,
    )

  const lock = (spaceId: string, on: boolean): Promise<void> =>
    withConfirm((token) => setSpaceRecordingLock(spaceId, on, token), { action: 'recording-lock', secretId: spaceId }, `recording lock for Space ${spaceId}`, `lock:${spaceId}`)

  const unavailable = sample || (policies !== null && !policies.flags.appRecording)

  return (
    <div className="space-y-6" data-settings-id="sew.recording">
      {ui}
      {error ? <SewFailed what="Couldn’t change the recording settings." failure={error} next="Nothing changed. Try again in a moment." /> : null}
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
            title="Recording"
            card
            hint="Both start off. The owner or an admin turns them on here, with a confirm (Touch ID, your login or the vault password). Agents never turn them on."
            testId="sew-recording-switches"
          >
            {RECORDING_SWITCHES.map((sw) => {
              const row = policies.policies.find((p) => p.key === sw.key)
              const on = policyOn(row)
              const busy = pending !== null
              const dmNeedsServer = sw.key === 'recording.dm_enabled' && !on && !policyOn(policies.policies.find((p) => p.key === 'recording.enabled'))
              return (
                <div key={sw.key} data-testid="sew-recording-switch" data-key={sw.key}>
                  <SettingRow
                    label={sw.label}
                    hint={
                      <>
                        {sw.hint}
                        {row && !row.delivered ? <span className="block">Saved here. Chat picks it up in a moment.</span> : null}
                        {!canMutate ? <span className="block">Only the owner or an admin changes this.</span> : null}
                        {dmNeedsServer ? <span className="block">Turn on recording on this server first.</span> : null}
                      </>
                    }
                  >
                    <Toggle
                      aria-label={sw.label}
                      checked={on}
                      disabled={!canMutate || busy || dmNeedsServer}
                      onChange={(v) => void setPolicy(sw.key, v, sw.label)}
                      size="sm"
                    />
                  </SettingRow>
                </div>
              )
            })}
          </SettingsGroup>
          <SettingsGroup
            title="This server’s rules"
            card
            hint={`Everything starts off. Each person in a call still answers their own consent notice, and is never recorded after saying no. The owner or an admin changes the rest from a terminal on the server (${CLI_NAME} hostchat recording policy set).`}
            testId="sew-recording-policies"
          >
            {policies.policies.filter((p) => !RECORDING_SWITCHES.some((sw) => sw.key === p.key)).map((p) => (
              <div key={p.key} data-testid="sew-recording-policy">
                <SettingRow
                  label={policyLabel(p.key)}
                  hint={
                    <>
                      {p.want === null ? 'Chat’s own setting.' : p.delivered ? null : 'Saved here. Chat picks it up in a moment.'}
                      {p.key === 'recording.required' ? <span className="block">{REQUIRED_ALSO_DMS}</span> : null}
                    </>
                  }
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
              title={`The ${id[0] ? id[0].toUpperCase() + id.slice(1) : id} App`}
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
              label={a.spaces.length === 1 ? `The ${a.app.name} App’s Space` : `The ${a.app.name} App’s Space ${i + 1}`}
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
