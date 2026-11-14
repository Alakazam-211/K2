// Settings → Apps → Add an App (prd-one-click-suite-apps-v1 R1–R5, vs-live
// A5/A9): Hearth and Meet in one step. Its own pane on the Apps page, picked
// from the list on the left; the look is the page's: a pane header, then
// card groups. A thin client over the daemon:
//
// - The catalog card (R2): one row per entry: name, publisher, the version
//   and "Verified by K2", the address it takes; whether this box can run it
//   now and, if not, why. Until Sew publishes the Seeds, no version is pinned
//   in this K2, and the pane says so in words instead of offering a Deploy
//   that would fail.
// - The deploy card (R3): what it will do, the address (prefilled with the
//   family label, checked live against the daemon as you type), who looks
//   after it, and what deploying approves.
// - Deploy starts the daemon job and the card ticks its steps (R4). The job
//   is the daemon's: closing Settings or the app never stops it, and a job
//   in flight shows on its row when Settings opens again.
// - A wait says why. When the job waits for the owner's approval, the owner
//   approves the filed request here with the same fresh vault confirm App
//   Approvals uses (A9).
//
// Nothing here decides anything: the address check is a hint, and the
// daemon repeats every check at deploy.

import React, { useCallback, useEffect, useRef, useState } from 'react'
import { Button, Callout, Input } from '@/components/ui'
import { daemonErrorOf, daemonErrorText } from '@/lib/daemon-error'
import { DaemonHttpError } from '@/lib/daemon-http-error'
import { useConnectHostStore } from '@/stores/connect-host'
import { useProjectsStore } from '@/stores/projects'
import { fetchViewerRole } from '@/components/Presence/PresenceKickButton'
import {
  SETTING_LIST_ROW_HOVER,
  SETTING_ROW_HINT,
  SETTING_ROW_LABEL,
  SETTING_SERVER_REASON,
  SettingDropdown,
  SettingRow,
  SettingsGroup,
  SettingsLoadState,
} from '../controls/SettingControls'
import { AppMonogram, PaneHeader, StatusDot, type DotTone } from './apps-ui'
import {
  STEP_LABELS,
  checkAddress,
  fetchCatalog,
  fetchJob,
  isActive,
  retryJob,
  startInstall,
  waitText,
  type AddressCheck,
  type CatalogEntry,
  type CatalogJob,
} from './app-catalog-api'
import { approveWithConfirm } from './AppHandoverSheet'
import { APP_APPROVE_CONFIRM_ACTION } from './app-approvals-api'
import { vaultApi } from './vault-api'
import { isAppsOff } from './apps-server-api'

const POLL_MS = 1500
const CHECK_DEBOUNCE_MS = 350
/** The daemon's reason when no version of an entry is pinned in this K2. */
export const NOT_PINNED = 'catalog_not_pinned'

type Load =
  | { kind: 'loading' }
  | { kind: 'off' }
  | { kind: 'old' }
  | { kind: 'ready'; entries: CatalogEntry[] }
  | { kind: 'error'; text: string }

/** A server whose K2 predates the catalog answers its route with a 404. */
function isRouteMissing(e: unknown): boolean {
  if (e instanceof DaemonHttpError && e.status === 404) return true
  return daemonErrorOf(e)?.code === 'not_found'
}

/** No entry has a pinned version: Sew hasn't published the Seeds yet. */
export function nothingPublished(entries: CatalogEntry[]): boolean {
  return entries.length > 0 && entries.every((e) => e.reasons.some((r) => r.code === NOT_PINNED))
}

function entryState(e: CatalogEntry): { tone: DotTone; words: string } {
  if (e.jobs.some(isActive)) return { tone: 'warn', words: 'Deploying' }
  if (e.alreadyDeployed) return { tone: 'ok', words: `Deployed as ${e.alreadyDeployed}` }
  if (e.reasons.some((r) => r.code === NOT_PINNED)) return { tone: 'idle', words: 'Not published yet' }
  if (!e.canDeploy) return { tone: 'warn', words: 'Can’t deploy here yet' }
  return { tone: 'ok', words: 'Ready to deploy' }
}

/** A card row: a label and a short list under it. */
function ListRow({ label, lines }: { label: string; lines: string[] }): React.JSX.Element {
  return (
    <div className="px-4 py-3">
      <div className={SETTING_ROW_LABEL}>{label}</div>
      <ul className="list-disc ml-5 mt-1 space-y-0.5">
        {lines.map((l) => (
          <li key={l} className="type-hint text-[var(--color-text-secondary)]">
            {l}
          </li>
        ))}
      </ul>
    </div>
  )
}

function StepList({ job }: { job: CatalogJob }): React.JSX.Element {
  return (
    <ol className="space-y-1" data-testid="catalog-steps">
      {STEP_LABELS.map(([id, label]) => {
        const s = job.steps.find((x) => x.step === id)
        const state = s?.state ?? 'todo'
        const tone: DotTone = state === 'done' ? 'ok' : state === 'failed' ? 'error' : state === 'todo' ? 'idle' : 'warn'
        return (
          <li key={id} className="flex items-start gap-2" data-step={id} data-state={state}>
            <StatusDot tone={tone} className="mt-1.5" />
            <span className={state === 'todo' ? 'type-hint text-[var(--color-text-muted)]' : 'type-hint text-[var(--color-text-primary)]'}>
              {label}
              {s?.note ? <span className="ml-2 text-[var(--color-text-secondary)]">{s.note}</span> : null}
            </span>
          </li>
        )
      })}
    </ol>
  )
}

function JobRows({ job, isOwner, onChanged }: { job: CatalogJob; isOwner: boolean; onChanged: (j: CatalogJob) => void }): React.JSX.Element {
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const requests = job.waiting?.reason === 'waiting_approval' ? job.waiting.requests : []

  const approve = async (): Promise<void> => {
    setBusy(true)
    setError(null)
    try {
      for (const r of requests) {
        const { methods } = await vaultApi.confirmStart(APP_APPROVE_CONFIRM_ACTION, r)
        if (!methods.includes('local-auth')) {
          throw new Error(JSON.stringify({ error: { code: 'confirm_elsewhere', hint: 'Approve it in App Approvals with your password.' } }))
        }
        await approveWithConfirm(r, `deploy ${job.app} (${job.entry})`, 'local-auth')
      }
      onChanged(await fetchJob(job.id))
    } catch (e) {
      setError(daemonErrorText(e))
    } finally {
      setBusy(false)
    }
  }

  const retry = async (): Promise<void> => {
    setBusy(true)
    setError(null)
    try {
      onChanged(await retryJob(job.id))
    } catch (e) {
      setError(daemonErrorText(e))
    } finally {
      setBusy(false)
    }
  }

  const where = job.address.url ?? `${job.address.label}.<this server>.k2.dev`
  return (
    <div className="px-4 py-3 space-y-3" data-testid="catalog-job" data-state={job.state}>
      <StepList job={job} />
      {job.state === 'done' ? (
        <Callout tone="ok" className="type-hint" role="status" data-testid="catalog-done">
          <p>
            {job.app} is live at{' '}
            {job.address.url ? (
              <a href={job.address.url} target="_blank" rel="noreferrer" className="underline">
                {job.address.url}
              </a>
            ) : (
              where
            )}
          </p>
          {(job.next ?? []).map((line) => (
            <p key={line} className="mt-1">
              Next: {line}
            </p>
          ))}
        </Callout>
      ) : null}
      {job.state === 'waiting' ? (
        <Callout tone="info" className="type-hint" role="status" data-testid="catalog-waiting">
          <p>{waitText(job)}</p>
          {job.waiting?.question ? <p className="mt-1">Open question {job.waiting.question} for the owner.</p> : null}
          {requests.length > 0 && isOwner ? (
            <Button className="mt-2" variant="accent" disabled={busy} onClick={() => void approve()} data-testid="catalog-approve">
              Approve
            </Button>
          ) : null}
        </Callout>
      ) : null}
      {job.state === 'failed' ? (
        <Callout tone="error" className="type-hint" role="alert" data-testid="catalog-failed">
          <p>
            {job.error?.code}: {job.error?.hint}
          </p>
          {job.error?.retry ? (
            <Button className="mt-2" disabled={busy} onClick={() => void retry()} data-testid="catalog-retry">
              Retry
            </Button>
          ) : null}
        </Callout>
      ) : null}
      {error ? (
        <p role="alert" className="type-hint text-[var(--color-status-error-soft)]">
          {error}
        </p>
      ) : null}
    </div>
  )
}

function DeployCard({
  entry,
  initialJob,
  isOwner,
  onClose,
}: {
  entry: CatalogEntry
  initialJob: CatalogJob | null
  isOwner: boolean
  onClose: () => void
}): React.JSX.Element {
  const projects = useProjectsStore((s) => s.projects)
  const activeProjectId = useProjectsStore((s) => s.activeProjectId)
  const agents = projects.filter((p) => p.path).sort((a, b) => a.name.localeCompare(b.name))
  const [workspace, setWorkspace] = useState<string | null>(activeProjectId ?? agents[0]?.id ?? null)
  const [address, setAddress] = useState(entry.defaultAddress.label)
  const [check, setCheck] = useState<AddressCheck | null>(null)
  const [job, setJob] = useState<CatalogJob | null>(initialJob)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const alive = useRef(true)

  useEffect(() => {
    alive.current = true
    return () => {
      alive.current = false
    }
  }, [])

  // The live address check (R13): debounced, a hint only.
  useEffect(() => {
    if (job) return
    const t = setTimeout(() => {
      void checkAddress(entry.entry, address, workspace)
        .then((c) => {
          if (alive.current) setCheck(c)
        })
        .catch((e) => {
          if (alive.current) setCheck({ ok: false, app: '', error: { code: 'check_failed', hint: daemonErrorText(e) } })
        })
    }, CHECK_DEBOUNCE_MS)
    return () => clearTimeout(t)
  }, [entry.entry, address, workspace, job])

  // While the job runs, follow it; the daemon owns it either way. A missed
  // poll is retried on the next tick (the job lives on in the daemon).
  useEffect(() => {
    if (!job || !isActive(job)) return
    const t = setInterval(() => {
      void fetchJob(job.id)
        .then((j) => {
          if (alive.current) setJob(j)
        })
        .catch(() => {})
    }, POLL_MS)
    return () => clearInterval(t)
  }, [job])

  const deploy = async (): Promise<void> => {
    setBusy(true)
    setError(null)
    try {
      setJob(await startInstall(entry.entry, address, workspace))
    } catch (e) {
      setError(daemonErrorText(e))
    } finally {
      setBusy(false)
    }
  }

  const suggestion = check?.error?.suggestion
  return (
    <SettingsGroup
      card
      title={job ? `Deploying ${entry.name}` : `Deploy ${entry.name}`}
      hint={
        job
          ? 'The deploy runs on the server: closing Settings doesn’t stop it, and its progress shows here again when you come back.'
          : 'Check the address and who looks after it, then deploy. The server repeats every check when it deploys.'
      }
      action={
        <Button variant="ghost" onClick={onClose} data-testid="catalog-close">
          {job ? 'Close' : 'Cancel'}
        </Button>
      }
      testId="catalog-sheet"
    >
      <ListRow label="What it will do" lines={entry.willDo} />
      {job ? (
        <JobRows job={job} isOwner={isOwner} onChanged={setJob} />
      ) : (
        <>
          <SettingRow
            label={<label htmlFor={`catalog-address-${entry.entry}`}>Address</label>}
            hint="Your own domain goes on after it deploys, on the App’s Domains."
            below={
              check?.error ? (
                <p className="type-hint text-[var(--color-status-error-soft)]" data-testid="catalog-address-problem">
                  {check.error.hint}
                  {suggestion ? (
                    <>
                      {' '}
                      <button type="button" className="underline" onClick={() => setAddress(suggestion)} data-testid="catalog-use-suggestion">
                        Use {suggestion}
                      </button>
                    </>
                  ) : null}
                </p>
              ) : check?.address ? (
                <p className="type-hint text-[var(--color-text-secondary)]" data-testid="catalog-address-ok">
                  {check.address.url ?? check.address.label} is free{check.tier?.hint ? ` · ${check.tier.hint}` : ''}
                </p>
              ) : undefined
            }
          >
            <span className="flex items-center gap-1">
              <Input
                id={`catalog-address-${entry.entry}`}
                data-testid="catalog-address"
                value={address}
                onChange={(e) => setAddress(e.target.value)}
                className="w-40"
              />
              <span className="type-hint text-[var(--color-text-secondary)]">.&lt;this server&gt;.k2.dev</span>
            </span>
          </SettingRow>
          <SettingRow label="Looked after by" hint="The agent whose App this becomes.">
            <SettingDropdown
              ariaLabel="Looked after by"
              value={workspace ?? ''}
              placeholder="Pick an agent"
              options={agents.map((p) => ({ value: p.id, label: p.name }))}
              onChange={(next) => setWorkspace(next || null)}
              menuAlign="right"
            />
          </SettingRow>
          {entry.approves.length > 0 ? (
            <ListRow
              label="What you approve by deploying"
              lines={entry.approves.map((a) => `${a.kind === 'boxService' ? `Box service ${a.name}` : `Cap ${a.name}`}${a.why ? `: ${a.why}` : ''}`)}
            />
          ) : null}
          <div className="px-4 py-3 flex flex-wrap items-center gap-2">
            <Button variant="accent" disabled={busy || !workspace || check?.ok === false} onClick={() => void deploy()} data-testid="catalog-deploy">
              {busy ? 'Starting…' : 'Deploy'}
            </Button>
            {error ? (
              <p role="alert" className="type-hint text-[var(--color-status-error-soft)]" data-testid="catalog-error">
                {error}
              </p>
            ) : null}
          </div>
        </>
      )}
    </SettingsGroup>
  )
}

export function AppsCatalogSection(): React.JSX.Element {
  const activeHost = useConnectHostStore((s) => s.activeHost)
  const [load, setLoad] = useState<Load>({ kind: 'loading' })
  const [open, setOpen] = useState<string | null>(null)
  const [role, setRole] = useState<string | null>(null)
  const mounted = useRef(true)

  const refresh = useCallback(async (): Promise<void> => {
    try {
      const entries = await fetchCatalog()
      if (mounted.current) setLoad({ kind: 'ready', entries })
    } catch (e) {
      if (!mounted.current) return
      setLoad(isAppsOff(e) ? { kind: 'off' } : isRouteMissing(e) ? { kind: 'old' } : { kind: 'error', text: daemonErrorText(e) })
    }
  }, [])

  useEffect(() => {
    mounted.current = true
    setLoad({ kind: 'loading' })
    setOpen(null)
    void refresh()
    void fetchViewerRole().then((r) => {
      if (mounted.current) setRole(r)
    })
    const onFocus = (): void => void refresh()
    window.addEventListener('focus', onFocus)
    return () => {
      mounted.current = false
      window.removeEventListener('focus', onFocus)
    }
  }, [refresh, activeHost])

  const isOwner = role === 'owner'
  const isStaff = role === 'owner' || role === 'admin'
  const entries = load.kind === 'ready' ? load.entries : []
  const opened = entries.find((e) => e.entry === open) ?? null

  const state =
    load.kind === 'loading' ? (
      <SettingsLoadState kind="loading" title="Reading this server’s catalog…" />
    ) : load.kind === 'off' ? (
      <SettingsLoadState kind="empty" title="Apps (beta) are off on this server" testId="catalog-apps-off">
        Turn them on in This server first; then Hearth and Meet can be added here.
      </SettingsLoadState>
    ) : load.kind === 'old' ? (
      <SettingsLoadState kind="empty" title="This server’s K2 can’t add Apps in one step yet" testId="catalog-server-old">
        Update K2 on this server to add Hearth and Meet from here.
      </SettingsLoadState>
    ) : load.kind === 'error' ? (
      <SettingsLoadState
        kind="error"
        title="Couldn’t read this server’s catalog"
        detail={load.text}
        action={
          <Button variant="default" onClick={() => void refresh()}>
            Try again
          </Button>
        }
      />
    ) : entries.length === 0 ? (
      <SettingsLoadState kind="empty" title="Nothing to add yet">
        This K2 has no Apps to deploy in one step.
      </SettingsLoadState>
    ) : undefined

  return (
    <div className="space-y-6" data-testid="apps-catalog">
      <PaneHeader
        title="Add an App"
        testId="catalog-header"
        hint="Apps K2 deploys in one step: download, check, install, address, deploy and approve, as one job on this server. Each version is pinned inside this K2 release and verified by K2."
      />
      {load.kind === 'ready' && nothingPublished(entries) ? (
        <Callout tone="info" className="type-hint" role="status" data-testid="catalog-not-published">
          Hearth and Meet aren’t published yet. Their first versions arrive with a K2 update once Sew publishes them, so there is
          nothing to deploy here until then.
        </Callout>
      ) : null}
      <SettingsGroup card title="Suite Apps" hint="From known publishers. After it deploys, each one is an ordinary App you can manage on its own page." state={state}>
        {entries.map((e) => {
          const running = e.jobs.find(isActive) ?? null
          const st = entryState(e)
          const shown = open === e.entry
          return (
            <div key={e.entry} className={`px-4 py-3 ${SETTING_LIST_ROW_HOVER}`} data-testid={`catalog-entry-${e.entry}`}>
              <div className="flex items-start justify-between gap-6">
                <div className="flex items-start gap-3 min-w-0 flex-1">
                  <AppMonogram name={e.name} />
                  <div className="min-w-0">
                    <div className="type-name text-[var(--color-text-primary)]">
                      {e.name}
                      <span className="ml-2 type-hint text-[var(--color-text-secondary)]">
                        by {e.publisher.name}
                        {e.version ? ` · ${e.version}` : ''} · {e.verified}
                      </span>
                    </div>
                    <p className={SETTING_ROW_HINT}>{e.summary}</p>
                    <p className={`${SETTING_ROW_HINT} flex items-start gap-1.5`} data-testid="catalog-state">
                      <StatusDot tone={st.tone} className="mt-1" />
                      <span>
                        {st.words} · {e.defaultAddress.url ?? `${e.defaultAddress.label}.<this server>.k2.dev`}
                      </span>
                    </p>
                    {e.reasons.map((r) => (
                      <p key={r.code} className={SETTING_SERVER_REASON} data-testid="catalog-reason" data-code={r.code}>
                        {r.text}
                      </p>
                    ))}
                  </div>
                </div>
                {isStaff ? (
                  <div className="flex-shrink-0">
                    <Button
                      variant={shown ? 'default' : running ? 'default' : 'accent'}
                      disabled={!running && !e.canDeploy}
                      aria-label={running ? `Show ${e.name}’s progress` : `Add ${e.name}`}
                      onClick={() => setOpen(shown ? null : e.entry)}
                      data-testid={`catalog-open-${e.entry}`}
                    >
                      {running ? 'Show progress' : 'Add…'}
                    </Button>
                  </div>
                ) : null}
              </div>
            </div>
          )
        })}
      </SettingsGroup>
      {load.kind === 'ready' && !isStaff && role != null ? (
        <p className={SETTING_ROW_HINT} data-testid="catalog-staff-only">
          Only the owner or an Admin of this server can add an App.
        </p>
      ) : null}
      {opened ? (
        <DeployCard
          key={opened.entry}
          entry={opened}
          initialJob={opened.jobs.find(isActive) ?? null}
          isOwner={isOwner}
          onClose={() => {
            setOpen(null)
            void refresh()
          }}
        />
      ) : null}
    </div>
  )
}
