// Settings → Org Chart (prd-org-chart-v1 §6.5): the server's switch for the
// Org Chart (off by default), the three night times in the owner's timezone,
// and where an agent's "Manage the org chart" switch lives. The daemon owns
// both settings (`orgChart`, `orgChartNight`) at the Admin floor: an Owner
// or Admin changes them, a Member sees them read-only.

import React, { useEffect, useState } from 'react'
import type { SettingEntry } from '../searchManifest'
import { Button, Input, Toggle } from '@/components/ui'
import { daemonCliGet, daemonCliPost, isHostSwitchedError } from '@/lib/daemon-cli'
import { daemonErrorText } from '@/lib/daemon-error'
import { primaryScope } from '@/kessel/server-scope'
import { useSettingsStore } from '@/stores/settings'
import { fetchViewerRole } from '@/components/Presence/PresenceKickButton'

export const ORG_CHART_SWITCH_LABEL = 'Org Chart'
export const ORG_CHART_SWITCH_LINE =
  'Show an Org Chart tab, group agents into orgs, and give every agent a daily bulletin. Off: no tab, no bulletin, no night runs.'
export const ORG_CHART_MEMBER_LINE = "Only the server's owner or an Admin can change these."
export const ORG_CHART_MANAGE_LINE =
  "Agents change the chart only when the Owner turns on Manage the org chart for that agent. Org heads write their own org's bulletin without it."

export const ORG_CHART_MANIFEST: SettingEntry[] = [
  {
    id: 'org-chart.switch',
    section: 'org-chart',
    label: 'Org Chart',
    description: 'Turn the Org Chart on for this server: the top-bar tab, orgs and heads, and the daily bulletin, with its night run times.',
    keywords: ['org chart', 'organization', 'orgs', 'departments', 'bulletin', 'head', 'night run', 'canvas', 'k2 organization'],
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
  { key: 'lastCallAt', label: 'Last call', hint: "One reminder to a head that hasn't written yet." },
  { key: 'dayStartsAt', label: 'New day', hint: "From here, every agent's bulletin counts as unread for the new day." },
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

const muted = 'text-[var(--color-text-muted)]'

export function OrgChartSection(): React.JSX.Element {
  const on = useSettingsStore((s) => s.orgChart)
  const [night, setNight] = useState<OrgChartNight>(DEFAULT_NIGHT)
  const [savedNight, setSavedNight] = useState<OrgChartNight>(DEFAULT_NIGHT)
  const [timezone, setTimezone] = useState<string>('')
  const [canChange, setCanChange] = useState<boolean | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  useEffect(() => {
    let live = true
    void daemonCliGet<{ orgChart?: boolean; orgChartNight?: unknown; ownerTimezone?: string | null }>(primaryScope(), 'settings/get').then(
      (s) => {
        if (!live) return
        const n = parseNight(s.orgChartNight)
        setNight(n)
        setSavedNight(n)
        setTimezone(s.ownerTimezone ?? '')
        useSettingsStore.setState({ orgChart: s.orgChart === true })
      },
      (e) => {
        if (live && !isHostSwitchedError(e)) setError(daemonErrorText(e))
      },
    )
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
    if (!on) {
      setSpread(null)
      return
    }
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
  }, [on, savedNight])

  const flip = async (): Promise<void> => {
    if (busy || canChange === false) return
    const next = !on
    setBusy(true)
    setError(null)
    useSettingsStore.setState({ orgChart: next }) // optimistic: the tab shows at once
    try {
      await daemonCliPost(primaryScope(), 'settings/update', { orgChart: next })
    } catch (e) {
      useSettingsStore.setState({ orgChart: !next })
      setError(daemonErrorText(e))
    } finally {
      setBusy(false)
    }
  }

  const problem = nightProblem(night)
  const nightDirty = NIGHT_ROWS.some((r) => night[r.key] !== savedNight[r.key])

  const saveNight = async (): Promise<void> => {
    if (problem || !nightDirty) return
    setBusy(true)
    setError(null)
    try {
      await daemonCliPost(primaryScope(), 'settings/update', { orgChartNight: night })
      setSavedNight(night)
    } catch (e) {
      setError(daemonErrorText(e))
    } finally {
      setBusy(false)
    }
  }

  const locked = canChange === false

  return (
    <div className="max-w-2xl space-y-4" data-settings-id="org-chart.switch" data-testid="org-chart-section">
      <div>
        <h2 className="type-title text-[var(--color-text-primary)]">Org Chart</h2>
        <p className={`type-hint ${muted} mt-1 leading-relaxed`}>
          Your agents on one canvas, grouped into orgs, each with a head who keeps a short bulletin. Every agent reads the server's bulletin once a day.
        </p>
        {locked ? (
          <p className={`type-hint ${muted} mt-1`} data-testid="org-chart-member-line">
            {ORG_CHART_MEMBER_LINE}
          </p>
        ) : null}
      </div>

      <div className="border border-[var(--color-border)] p-3">
        <div className="flex items-start gap-3">
          <Toggle
            size="sm"
            checked={on}
            onChange={() => void flip()}
            aria-label={ORG_CHART_SWITCH_LABEL}
            data-testid="org-chart-switch"
            disabled={busy || locked}
            className="mt-0.5"
          />
          <div className="min-w-0 flex-1">
            <div className="type-name text-[var(--color-text-primary)]">{ORG_CHART_SWITCH_LABEL}</div>
            <div className={`type-hint ${muted} mt-0.5 leading-relaxed`}>{ORG_CHART_SWITCH_LINE}</div>
          </div>
        </div>
      </div>

      <div className="space-y-2 border border-[var(--color-border)] p-3" data-settings-id="org-chart.night">
        <div className="type-name text-[var(--color-text-primary)]">The night</div>
        <div className={`type-hint ${muted}`} data-testid="org-chart-timezone">
          Times are in {timezone ? `the owner's timezone, ${timezone}` : "this server's own clock (no owner timezone is set: Settings → General)"}.
        </div>
        {NIGHT_ROWS.map((r) => (
          <label key={r.key} className="flex items-center gap-3">
            <span className="w-24 flex-shrink-0 type-row text-[var(--color-text-secondary)]">{r.label}</span>
            <Input
              type="time"
              value={night[r.key]}
              disabled={locked || busy}
              aria-label={r.label}
              className="w-28"
              onChange={(e) => setNight({ ...night, [r.key]: e.target.value })}
            />
            <span className={`type-hint ${muted}`}>{r.hint}</span>
          </label>
        ))}
        {spread ? (
          <div className={`type-hint ${muted}`} data-testid="org-chart-spread">
            {spread}
          </div>
        ) : null}
        <div className="flex items-center gap-2">
          {problem ?<span className="type-hint text-[var(--color-status-error-text)]">{problem}</span> : null}
          <Button size="sm" className="ml-auto" disabled={locked || busy || !!problem || !nightDirty} onClick={() => void saveNight()}>
            Save times
          </Button>
        </div>
      </div>

      <div className="space-y-1 border border-[var(--color-border)] p-3">
        <div className="type-name text-[var(--color-text-primary)]">Agents on the chart</div>
        <div className={`type-hint ${muted} leading-relaxed`}>{ORG_CHART_MANAGE_LINE}</div>
        <Button size="sm" variant="quiet" onClick={() => useSettingsStore.getState().setSection('projects')}>
          Open Workspaces / Agents
        </Button>
      </div>

      {error ? (
        <div role="alert" className="type-hint text-[var(--color-status-error-soft)]">
          {error}
        </div>
      ) : null}
    </div>
  )
}
