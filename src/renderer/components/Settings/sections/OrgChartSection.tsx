// Settings → Org Chart (prd-org-chart-v1 §6.5): the server's switch for the
// Org Chart (off by default), the three night times in the owner's timezone,
// and where an agent's "Manage the org chart" switch lives. The daemon owns
// both settings (`orgChart`, `orgChartNight`) at the Admin floor: an Owner
// or Admin changes them, a Member sees them read-only.
//
// Card groups, top to bottom: the switch; The night (three times, each a
// row with its hint under the label, then how the run is paced and Save);
// Agents on the chart (a link to Workspaces / Agents).

import React, { useCallback, useEffect, useState } from 'react'
import type { SettingEntry } from '../searchManifest'
import {
  SETTING_ROW_HINT,
  SETTING_ROW_LABEL,
  SettingRow,
  SettingSwitchRow,
  SettingsGroup,
  SettingsLoadState,
} from '../controls/SettingControls'
import { DENIED_LINE, RowSaveError, RowSaveNote, useRowSave } from './row-save'
import { BetaTag, Button, Input, Tag, Toggle } from '@/components/ui'
import { daemonCliGet, daemonCliPost, isHostSwitchedError } from '@/lib/daemon-cli'
import { daemonErrorText } from '@/lib/daemon-error'
import { primaryScope } from '@/kessel/server-scope'
import { useSettingsStore } from '@/stores/settings'
import { fetchViewerRole } from '@/components/Presence/PresenceKickButton'
import { SettingsLink } from './ComputeBits'
import { CLI_NAME } from '@/lib/brand'

export const ORG_CHART_SWITCH_LABEL = 'Org Chart'
export const ORG_CHART_SWITCH_LINE =
  'Adds an Org Chart tab, groups agents into orgs and gives every agent a daily bulletin. Off: no tab, no bulletin and no night runs.'
export const ORG_CHART_MEMBER_LINE = DENIED_LINE
export const ORG_CHART_MANAGE_LINE =
  'An agent changes the chart only when its Manage the org chart switch is on, in Workspaces / Agents. Org heads write their own org’s bulletin without it.'

export const ORG_CHART_MANIFEST: SettingEntry[] = [
  {
    id: 'org-chart.switch',
    section: 'org-chart',
    label: 'Org Chart',
    description: 'Turn the Org Chart on for this server: the top-bar tab, orgs and heads, and the daily bulletin, with its night run times.',
    keywords: ['org chart', 'organization', 'orgs', 'departments', 'bulletin', 'head', 'night run', 'canvas', `${CLI_NAME} organization`],
  },
]

export interface OrgChartNight {
  runAt: string
  lastCallAt: string
  dayStartsAt: string
}

export const DEFAULT_NIGHT: OrgChartNight = { runAt: '01:00', lastCallAt: '04:30', dayStartsAt: '06:00' }

const NIGHT_ROWS: { key: keyof OrgChartNight; label: string; hint: string }[] = [
  { key: 'runAt', label: 'Night run', hint: 'Each org head is woken to update its bulletin.' },
  { key: 'lastCallAt', label: 'Last call', hint: 'One reminder to a head that hasn’t written yet.' },
  { key: 'dayStartsAt', label: 'New day', hint: 'Every agent’s bulletin counts as unread from here.' },
]

/** Minutes after 22:00 on the night's clock (22:00 → 0, 07:00 → 540), or
 *  null outside 22:00–07:00. */
function nightMinute(hhmm: string): number | null {
  const m = /^(\d{2}):(\d{2})$/.exec(hhmm)
  if (!m) return null
  const t = Number(m[1]) * 60 + Number(m[2])
  if (t >= 22 * 60) return t - 22 * 60
  if (t <= 7 * 60) return t + 2 * 60
  return null
}

/** The daemon's rule (PRD Q2): all three fall in order within 22:00–07:00. */
export function nightProblem(n: OrgChartNight): string | null {
  const ms = NIGHT_ROWS.map((r) => nightMinute(n[r.key]))
  const off = ms.findIndex((m) => m === null)
  if (off >= 0) return `${NIGHT_ROWS[off]!.label} must fall between 22:00 and 07:00.`
  if (!(ms[0]! < ms[1]! && ms[1]! < ms[2]!)) return 'The times go in order: night run, then last call, then the new day.'
  return null
}

function parseNight(v: unknown): OrgChartNight {
  if (!v || typeof v !== 'object') return DEFAULT_NIGHT
  const o = v as Record<string, unknown>
  const pick = (k: keyof OrgChartNight): string => (typeof o[k] === 'string' ? (o[k] as string) : DEFAULT_NIGHT[k])
  // The daemon reads back `dayStartsAt`; an older one said `editionAt`.
  const dayStartsAt = typeof o.dayStartsAt === 'string' ? o.dayStartsAt : typeof o.editionAt === 'string' ? o.editionAt : DEFAULT_NIGHT.dayStartsAt
  return { runAt: pick('runAt'), lastCallAt: pick('lastCallAt'), dayStartsAt }
}

/** The daemon's `organization/night` answer, the parts this page reads. */
interface NightSpread {
  runAt?: unknown
  spreadEndsAt?: unknown
  spreadMinutes?: unknown
  governor?: { slots?: unknown } | null
}

/** "Night run spreads over 180 min (01:00–04:00), 2 at a time on this
 *  server." from the daemon (which owns the rule), or null. Read only. */
export function spreadLine(v: NightSpread | null | undefined): string | null {
  if (!v || typeof v.spreadMinutes !== 'number' || typeof v.runAt !== 'string' || typeof v.spreadEndsAt !== 'string') return null
  const slots = typeof v.governor?.slots === 'number' ? v.governor.slots : null
  const pace = slots === null ? '' : `, ${slots} at a time on this server`
  return `Night run spreads over ${v.spreadMinutes} min (${v.runAt}–${v.spreadEndsAt})${pace}.`
}

const PAGE = 'w-full max-w-[52rem] space-y-6 pb-6'

function Header({ readOnly }: { readOnly: boolean }): React.JSX.Element {
  return (
    <header className="min-w-0">
      <h2 className="type-title text-[var(--color-text-primary)] flex items-center gap-2">
        Org Chart <BetaTag />
        {readOnly && <Tag tone="muted">View only</Tag>}
      </h2>
      <p className={`${SETTING_ROW_HINT} mt-1`}>
        Your agents on one canvas, grouped into orgs. Each org has a head who keeps a short bulletin, and every agent reads
        the server’s bulletin once a day.
      </p>
      {readOnly && (
        <p className="type-hint text-[var(--color-text-primary)] mt-1" data-testid="org-chart-member-line">
          {ORG_CHART_MEMBER_LINE}
        </p>
      )}
    </header>
  )
}

/** A time a Member can see but not change: plain text on the control's edge. */
function ReadOnlyTime({ value, label }: { value: string; label: string }): React.JSX.Element {
  return (
    <span aria-label={label} data-readonly-value="" className="block text-right type-row text-[var(--color-text-primary)] tabular-nums">
      {value}
    </span>
  )
}

interface Loaded {
  timezone: string
}

export function OrgChartSection(): React.JSX.Element {
  const on = useSettingsStore((s) => s.orgChart)
  const [loaded, setLoaded] = useState<Loaded | null>(null)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [night, setNight] = useState<OrgChartNight>(DEFAULT_NIGHT)
  const [savedNight, setSavedNight] = useState<OrgChartNight>(DEFAULT_NIGHT)
  const [canChange, setCanChange] = useState<boolean | null>(null)
  const [busy, setBusy] = useState(false)
  const switchSave = useRowSave()
  const nightSave = useRowSave()

  const load = useCallback((): (() => void) => {
    let live = true
    setLoadError(null)
    void daemonCliGet<{ orgChart?: boolean; orgChartNight?: unknown; ownerTimezone?: string | null }>(primaryScope(), 'settings/get').then(
      (s) => {
        if (!live) return
        const n = parseNight(s.orgChartNight)
        setNight(n)
        setSavedNight(n)
        setLoaded({ timezone: s.ownerTimezone ?? '' })
        useSettingsStore.setState({ orgChart: s.orgChart === true })
      },
      (e) => {
        if (live && !isHostSwitchedError(e)) setLoadError(daemonErrorText(e))
      },
    )
    return () => {
      live = false
    }
  }, [])

  useEffect(() => load(), [load])

  useEffect(() => {
    let live = true
    void fetchViewerRole().then((role) => {
      if (live) setCanChange(role === 'owner' || role === 'admin')
    })
    return () => {
      live = false
    }
  }, [])

  // How the night run is paced (read only; the daemon answers only while
  // the Org Chart is on).
  const [spread, setSpread] = useState<string | null>(null)
  useEffect(() => {
    // Hidden while it is (re-)read, so a saved change never sits beside the
    // old pacing.
    setSpread(null)
    if (!on || !loaded) return
    let live = true
    void daemonCliGet<NightSpread>(primaryScope(), 'organization/night').then(
      (v) => {
        if (live) setSpread(spreadLine(v))
      },
      () => {
        if (live) setSpread(null)
      },
    )
    return () => {
      live = false
    }
  }, [on, loaded, savedNight])

  const locked = canChange === false

  const flip = async (): Promise<void> => {
    if (busy || locked) return
    const next = !on
    setBusy(true)
    switchSave.saving()
    useSettingsStore.setState({ orgChart: next }) // optimistic: the tab shows at once
    try {
      await daemonCliPost(primaryScope(), 'settings/update', { orgChart: next })
      switchSave.saved()
    } catch (e) {
      useSettingsStore.setState({ orgChart: !next })
      switchSave.failed(e)
    } finally {
      setBusy(false)
    }
  }

  const problem = nightProblem(night)
  const nightDirty = NIGHT_ROWS.some((r) => night[r.key] !== savedNight[r.key])

  const saveNight = async (): Promise<void> => {
    if (problem || !nightDirty) return
    setBusy(true)
    nightSave.saving()
    try {
      await daemonCliPost(primaryScope(), 'settings/update', { orgChartNight: night })
      setSavedNight(night)
      nightSave.saved()
    } catch (e) {
      nightSave.failed(e)
    } finally {
      setBusy(false)
    }
  }

  const undoNight = (): void => {
    setNight(savedNight)
    nightSave.reset()
  }

  if (!loaded) {
    return (
      <div className={PAGE} data-settings-id="org-chart.switch" data-testid="org-chart-section">
        <Header readOnly={false} />
        {loadError ? (
          <SettingsLoadState
            kind="error"
            title="Couldn’t load the Org Chart settings"
            detail={loadError}
            action={<Button onClick={() => load()}>Try again</Button>}
            testId="org-chart-load-error"
          >
            Nothing changed on this server; this page just couldn’t read it.
          </SettingsLoadState>
        ) : (
          <SettingsLoadState kind="loading" title="Loading the Org Chart settings…" testId="org-chart-loading" />
        )}
      </div>
    )
  }

  const saving = nightSave.state === 'saving'
  const denied = typeof nightSave.state === 'object' && nightSave.state.denied
  const timezone = loaded.timezone
  const nightHint = timezone ? (
    <span data-testid="org-chart-timezone">Times are in the owner’s timezone, {timezone}. All three fall between 22:00 and 07:00.</span>
  ) : (
    <span data-testid="org-chart-timezone">
      Times use this server’s own clock until you set your timezone.{' '}
      {!locked && <SettingsLink to="general">Set your timezone</SettingsLink>}
    </span>
  )

  return (
    <div className={PAGE} data-settings-id="org-chart.switch" data-testid="org-chart-section">
      <Header readOnly={locked} />

      <SettingsGroup card testId="org-chart-switch-card">
        <SettingSwitchRow
          control={
            <>
              <RowSaveNote state={switchSave.state} />
              <Toggle
                checked={on}
                onChange={() => void flip()}
                aria-label={ORG_CHART_SWITCH_LABEL}
                data-testid="org-chart-switch"
                disabled={busy || locked}
              />
            </>
          }
        >
          <div className={SETTING_ROW_LABEL}>{ORG_CHART_SWITCH_LABEL}</div>
          <div className={SETTING_ROW_HINT}>{ORG_CHART_SWITCH_LINE}</div>
          <RowSaveError state={switchSave.state} testId="org-chart-switch-error" />
        </SettingSwitchRow>
      </SettingsGroup>

      <SettingsGroup title="The night" hint={nightHint} card settingId="org-chart.night" testId="org-chart-night">
        {NIGHT_ROWS.map((r) => (
          <SettingRow key={r.key} label={r.label} hint={r.hint} field>
            {locked ? (
              <ReadOnlyTime value={night[r.key]} label={r.label} />
            ) : (
              <Input
                type="time"
                value={night[r.key]}
                disabled={busy}
                aria-label={r.label}
                className="w-full"
                onChange={(e) => {
                  setNight({ ...night, [r.key]: e.target.value })
                  nightSave.reset()
                }}
              />
            )}
          </SettingRow>
        ))}
        {(spread || problem || nightDirty || nightSave.state !== 'idle') && (
          <div className="px-4 py-3" data-testid="org-chart-night-footer">
            <div className="flex items-center justify-between gap-6">
              <div className="min-w-0">
                {saving ? (
                  <p className={SETTING_ROW_HINT}>Saving the new times…</p>
                ) : problem ? (
                  <p role="alert" className="type-hint text-[var(--color-status-error-soft)] leading-relaxed" data-testid="org-chart-night-problem">
                    {problem}
                  </p>
                ) : nightDirty ? (
                  <p className={SETTING_ROW_HINT}>Unsaved changes.</p>
                ) : spread ? (
                  <p className={SETTING_ROW_HINT} data-testid="org-chart-spread">
                    {spread}
                  </p>
                ) : null}
                <RowSaveError state={nightSave.state} testId="org-chart-night-error" />
              </div>
              <div className="flex items-center gap-2 flex-shrink-0">
                {nightSave.state === 'saved' && <RowSaveNote state={nightSave.state} />}
                {nightDirty && !locked && (
                  <>
                    <Button variant="quiet" disabled={busy} onClick={undoNight}>
                      Undo changes
                    </Button>
                    {!denied && (
                      <Button variant="accent" disabled={busy || !!problem} onClick={() => void saveNight()}>
                        {saving ? 'Saving…' : 'Save times'}
                      </Button>
                    )}
                  </>
                )}
              </div>
            </div>
          </div>
        )}
      </SettingsGroup>

      <SettingsGroup title="Agents on the chart" card testId="org-chart-agents">
        <SettingRow label="Who can change the chart" hint={ORG_CHART_MANAGE_LINE}>
          <Button variant="quiet" onClick={() => useSettingsStore.getState().setSection('projects')}>
            Open Workspaces / Agents
          </Button>
        </SettingRow>
      </SettingsGroup>
    </div>
  )
}
