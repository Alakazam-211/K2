// Settings → Apps → Add an App (prd-one-click-suite-apps-v1 R1–R5, vs-live
// A5/A9): Hearth and Meet in one step. Its own pane on the Apps page, picked
// from the list on the left; the look is the page's: a pane header, then
// card groups. A thin client over the daemon:
//
// - The catalog card (R2): one row per entry: name, publisher, the version
//   and "Verified by K2" (only when the server says a pinned, published
//   version is verified), the address it takes; whether this box can run it
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
import { Button, Callout, Input, Spinner } from '@/components/ui'
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
import { AppMonogram, LinkButton, PaneHeader, StatusDot, serverWords, type DotTone } from './apps-ui'
import {
  STEP_LABELS,
  checkAddress,
  fetchCatalog,
  fetchJob,
  isActive,
  plainText,
  retryJob,
  startInstall,
  stepLabel,
  waitText,
  type AddressCheck,
  type CatalogEntry,
  type CatalogReason,
  type CatalogJob,
} from './app-catalog-api'
import { approveWithConfirm } from './AppHandoverSheet'
import { APP_APPROVE_CONFIRM_ACTION } from './app-approvals-api'
import { vaultApi } from './vault-api'
import { isAppsOff } from './apps-server-api'
import { PRODUCT_NAME } from '@/lib/brand'

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

/** The reasons every entry shares (two or more entries): the pane says each
 *  once, above the card, instead of under every row. */
export function sharedReasons(entries: CatalogEntry[]): CatalogReason[] {
  if (entries.length < 2) return []
  return entries[0].reasons.filter((r) => entries.every((e) => e.reasons.some((x) => x.code === r.code)))
}

/** `https://chat.acme.example` → `chat.acme.example`. */
export function hostOf(url: string | null | undefined): string | null {
  if (!url) return null
  return url.replace(/^[a-z]+:\/\//i, '').replace(/[/?#].*$/, '') || null
}

/** A reason in the page's words: not-published in our own sentence, the
 *  rest the server's, with its names in the page's words. */
function reasonWords(r: CatalogReason): string {
  if (r.code === NOT_PINNED) return `Its first version comes with a ${PRODUCT_NAME} update.`
  const t = serverWords(r.text).trim().replace(/[.\s]+$/, '')
  return `${t.charAt(0).toUpperCase()}${t.slice(1)}.`
}

/** "Hearth", "Hearth and Meet", "Hearth, Meet and Lens". */
function namesOf(entries: CatalogEntry[]): string {
  const n = entries.map((e) => e.name)
  return n.length < 2 ? (n[0] ?? '') : `${n.slice(0, -1).join(', ')} and ${n[n.length - 1]}`
}

/** The code the pane gives an address check that couldn't run at all. */
const CHECK_FAILED = 'check_failed'

/** A server's reason, ready for "The server said: …." */
function trimReason(text: string): string {
  return serverWords(text).trim().replace(/[.\s]+$/, '')
}

/** What follows the App's own label in its address (`.acme.example`), when
 *  the server has a public address at all. */
export function addressSuffix(a: { label: string; url: string | null }): string | null {
  const host = hostOf(a.url)
  return host && host.startsWith(`${a.label}.`) ? host.slice(a.label.length) : null
}

/** One line of what deploying approves: what the App may then do, in the
 *  server's plain words, with the name it goes by after it. */
export function approvalWords(a: { kind: string; name: string; why?: string }): string {
  if (a.why) return `${a.why} (${a.name})`
  return `${a.kind === 'boxService' ? 'Server service' : 'Permission'} ${a.name}`
}

/** Under the address: checking, free, taken (with the server's suggestion),
 *  or a check that couldn't run. Read out politely as it changes. */
function AddressCheckLine({ check, onUse }: { check: AddressCheck | null; onUse: (label: string) => void }): React.JSX.Element {
  const suggestion = check?.error?.suggestion
  return (
    <div aria-live="polite">
      {check == null ? (
        <p className={`${SETTING_ROW_HINT} flex items-center gap-1.5`} data-testid="catalog-address-checking">
          <Spinner />
          Checking the address…
        </p>
      ) : check.error?.code === CHECK_FAILED ? (
        <div data-testid="catalog-address-unchecked">
          <p className="type-hint text-[var(--color-status-warn-text)]">Couldn’t check the address. You can still deploy: the server checks it then.</p>
          <p className={SETTING_SERVER_REASON}>The server said: {trimReason(check.error.hint)}.</p>
        </div>
      ) : check.error ? (
        <p className="type-hint text-[var(--color-status-error-soft)] flex flex-wrap items-center gap-x-1.5" data-testid="catalog-address-problem">
          <span className="inline-flex items-center gap-1.5">
            <StatusDot tone="error" />
            {trimReason(check.error.text ?? check.error.hint)}.
          </span>
          {suggestion ? (
            <LinkButton onClick={() => onUse(suggestion)} testId="catalog-use-suggestion">
              Use {suggestion}
            </LinkButton>
          ) : null}
        </p>
      ) : check.address ? (
        <p className={`${SETTING_ROW_HINT} flex flex-wrap items-center gap-x-1.5`} data-testid="catalog-address-ok">
          <span className="inline-flex items-center gap-1.5">
            <StatusDot tone="ok" />
            {hostOf(check.address.url) ?? check.address.label} is free
          </span>
          {check.tier?.hint ? <span>· {check.tier.hint}</span> : null}
        </p>
      ) : (
        <span />
      )}
    </div>
  )
}

/** "Verified by K2" with its check mark. */
function VerifiedMark({ children }: { children: React.ReactNode }): React.JSX.Element {
  return (
    <span className="inline-flex items-center gap-1" data-testid="catalog-verified">
      <svg aria-hidden="true" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth={2} className="w-3 h-3 flex-shrink-0 text-[var(--color-status-ok-soft)]">
        <path d="M3.5 8.5 6.5 11.5 12.5 4.5" />
      </svg>
      {children}
    </span>
  )
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
            {serverWords(l)}
          </li>
        ))}
      </ul>
    </div>
  )
}

/** A step's state in words and colour. The dot is never the only sign. */
function stepLook(state: string, needsOwner: boolean): { tone: DotTone; words: string | null; text: string } {
  switch (state) {
    case 'done':
      return { tone: 'ok', words: 'Done', text: 'text-[var(--color-text-secondary)]' }
    case 'running':
      return { tone: 'warn', words: 'In progress', text: 'text-[var(--color-text-secondary)]' }
    case 'waiting':
      return { tone: 'warn', words: needsOwner ? 'Needs approval' : 'Waiting', text: 'text-[var(--color-status-warn-text)]' }
    case 'failed':
      return { tone: 'error', words: 'Failed', text: 'text-[var(--color-status-error-soft)]' }
    default:
      return { tone: 'idle', words: null, text: '' }
  }
}

function StepList({ job }: { job: CatalogJob }): React.JSX.Element {
  const needsOwner = job.waiting?.reason === 'waiting_approval'
  return (
    <ol className="space-y-1.5 max-w-sm" aria-label="Steps" data-testid="catalog-steps">
      {STEP_LABELS.map(([id, label]) => {
        const state = job.steps.find((x) => x.step === id)?.state ?? 'todo'
        const look = stepLook(state, needsOwner)
        return (
          <li key={id} className="flex items-center gap-2.5" data-step={id} data-state={state}>
            {state === 'running' ? <Spinner /> : <StatusDot tone={look.tone} className="mx-[3px]" />}
            <span className={`type-hint flex-1 min-w-0 ${state === 'todo' ? 'text-[var(--color-text-muted)]' : 'text-[var(--color-text-primary)]'}`}>{label}</span>
            {look.words ? <span className={`type-hint flex-shrink-0 ${look.text}`}>{look.words}</span> : null}
          </li>
        )
      })}
    </ol>
  )
}

function JobRows({ job, isOwner, onChanged }: { job: CatalogJob; isOwner: boolean; onChanged: (j: CatalogJob) => void }): React.JSX.Element {
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<{ what: string; why: string } | null>(null)
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
      setError({ what: 'Couldn’t approve it.', why: daemonErrorText(e) })
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
      setError({ what: 'Couldn’t start it again.', why: daemonErrorText(e) })
    } finally {
      setBusy(false)
    }
  }

  const host = hostOf(job.address.url)
  return (
    <div className="px-4 py-3 space-y-3" data-testid="catalog-job" data-state={job.state}>
      <StepList job={job} />
      {job.state === 'done' ? (
        <Callout tone="ok" className="type-hint" role="status" data-testid="catalog-done">
          <p>
            {job.app} is live at{' '}
            {job.address.url ? (
              <a href={job.address.url} target="_blank" rel="noreferrer" data-k2-focus="" className="relative underline underline-offset-2">
                {host}
              </a>
            ) : (
              job.address.label
            )}
            .
          </p>
          {(job.nextSteps ?? []).map((n) => (
            <p key={n.text} className="mt-1">
              Next: {serverWords(n.text)}
            </p>
          ))}
        </Callout>
      ) : null}
      {job.state === 'waiting' ? (
        <Callout tone="warn" className="type-hint" role="status" data-testid="catalog-waiting">
          <p>{waitText(job)}</p>
          {requests.length > 0 && !isOwner ? <p className="mt-1">The owner can approve it here or in App Approvals.</p> : null}
          {job.waiting?.reason !== 'waiting_approval' && job.waiting?.reason !== 'waiting_sew_rights' && plainText(job.waiting) ? (
            <p className={SETTING_SERVER_REASON}>The server said: {trimReason(plainText(job.waiting) ?? '')}.</p>
          ) : null}
          {requests.length > 0 && isOwner ? (
            <Button className="mt-2" variant="accent" disabled={busy} onClick={() => void approve()} data-testid="catalog-approve">
              {busy ? 'Approving…' : 'Approve'}
            </Button>
          ) : null}
        </Callout>
      ) : null}
      {job.state === 'failed' ? (
        <Callout tone="error" className="type-hint" role="alert" data-testid="catalog-failed">
          <p>
            The deploy stopped at {stepLabel(job.step)}.{' '}
            {job.error?.retry ? 'You can try again from that step.' : 'Close this, fix what the server says, and deploy again.'}
          </p>
          {plainText(job.error) ? <p className={SETTING_SERVER_REASON}>The server said: {trimReason(plainText(job.error) ?? '')}.</p> : null}
          {job.error?.retry ? (
            <Button className="mt-2" disabled={busy} onClick={() => void retry()} data-testid="catalog-retry">
              {busy ? 'Starting again…' : 'Try again'}
            </Button>
          ) : null}
        </Callout>
      ) : null}
      {error ? (
        <div role="alert">
          <p className="type-hint text-[var(--color-status-error-soft)]">{error.what}</p>
          <p className={SETTING_SERVER_REASON}>The server said: {trimReason(error.why)}.</p>
        </div>
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

  // The live address check (R13): debounced, a hint only. A new address or
  // agent clears the last answer at once, so a stale "is free" never shows
  // while the next check runs; an answer for an older value is dropped.
  const checkSeq = useRef(0)
  useEffect(() => {
    if (job) return
    const n = ++checkSeq.current
    setCheck(null)
    const t = setTimeout(() => {
      void checkAddress(entry.entry, address, workspace)
        .then((c) => {
          if (alive.current && checkSeq.current === n) setCheck(c)
        })
        .catch((e) => {
          if (alive.current && checkSeq.current === n) setCheck({ ok: false, app: '', error: { code: CHECK_FAILED, hint: daemonErrorText(e) } })
        })
    }, CHECK_DEBOUNCE_MS)
    return () => clearTimeout(t)
  }, [entry.entry, address, workspace, job])

  // Opening the card puts the keyboard in its first field.
  const addressRef = useRef<HTMLInputElement | null>(null)
  useEffect(() => {
    addressRef.current?.focus()
  }, [])

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

  const suffix = addressSuffix(entry.defaultAddress)
  // The server said the address won't do: Deploy waits for a better one. A
  // check that couldn't run at all is no answer, so Deploy stays open (the
  // server checks again when it deploys).
  const refused = check?.ok === false && check.error?.code !== CHECK_FAILED
  return (
    <SettingsGroup
      card
      title={job ? `Deploying ${entry.name}` : `Deploy ${entry.name}`}
      hint={
        job
          ? 'The deploy runs on the server: closing Settings doesn’t stop it, and its progress shows here again when you come back.'
          : 'Check the address and who looks after it, then deploy. The server checks everything again when it deploys.'
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
            hint="Where people open it. You can add your own domain on the App’s page after it deploys."
            below={<AddressCheckLine check={check} onUse={setAddress} />}
          >
            <span className="flex flex-wrap items-center gap-x-2 gap-y-1 min-w-0">
              <Input
                ref={addressRef}
                id={`catalog-address-${entry.entry}`}
                data-testid="catalog-address"
                value={address}
                onChange={(e) => setAddress(e.target.value)}
                className="w-40"
              />
              {suffix ? <span className="type-hint text-[var(--color-text-secondary)] break-all">{suffix}</span> : null}
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
          {entry.approves.length > 0 ? <ListRow label="What you approve by deploying" lines={entry.approves.map(approvalWords)} /> : null}
          <div className="px-4 py-3">
            <Button variant="accent" disabled={busy || !workspace || refused} onClick={() => void deploy()} data-testid="catalog-deploy">
              {busy ? 'Starting…' : 'Deploy'}
            </Button>
            {error ? (
              <div role="alert" className="mt-2" data-testid="catalog-error">
                <p className="type-hint text-[var(--color-status-error-soft)]">Couldn’t start the deploy.</p>
                <p className={SETTING_SERVER_REASON}>The server said: {trimReason(error)}.</p>
              </div>
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
  const openers = useRef(new Map<string, HTMLButtonElement>())

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
  const shared = sharedReasons(entries)

  const state =
    load.kind === 'loading' ? (
      <SettingsLoadState kind="loading" title="Reading this server’s catalog…" />
    ) : load.kind === 'off' ? (
      <SettingsLoadState kind="empty" title="Apps (beta) are off on this server" testId="catalog-apps-off">
        Turn them on in This server first; then Hearth and Meet can be added here.
      </SettingsLoadState>
    ) : load.kind === 'old' ? (
      <SettingsLoadState kind="empty" title={`This server’s ${PRODUCT_NAME} can’t add Apps in one step yet`} testId="catalog-server-old">
        Update {PRODUCT_NAME} on this server to add Hearth and Meet from here.
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
        This {PRODUCT_NAME} has no Apps to deploy in one step.
      </SettingsLoadState>
    ) : undefined

  return (
    <div className="space-y-6" data-testid="apps-catalog">
      <PaneHeader
        title="Add an App"
        testId="catalog-header"
        hint={`Apps from known publishers, set up on this server in one step. ${PRODUCT_NAME} checks every version before it installs it.`}
      />
      {shared.length > 0 ? (
        <Callout tone="info" className="type-hint space-y-1" role="status" data-testid={nothingPublished(entries) ? 'catalog-not-published' : 'catalog-shared-reasons'}>
          {shared.map((r) => (
            <p key={r.code}>
              {r.code === NOT_PINNED
                ? `${namesOf(entries)} aren’t published yet. Their first versions come with a ${PRODUCT_NAME} update; until then there’s nothing to add here.`
                : reasonWords(r)}
            </p>
          ))}
        </Callout>
      ) : null}
      <SettingsGroup card title="Suite Apps" hint="From known publishers. After it deploys, each one is an ordinary App you can manage on its own page." state={state}>
        {entries.map((e) => {
          const running = e.jobs.find(isActive) ?? null
          const st = entryState(e)
          const shown = open === e.entry
          const own = e.reasons.filter((r) => !shared.some((s) => s.code === r.code))
          return (
            <div key={e.entry} className={`px-4 py-3 ${SETTING_LIST_ROW_HOVER}`} data-testid={`catalog-entry-${e.entry}`}>
              <div className="flex items-start gap-3">
                <AppMonogram name={e.name} />
                <div className="min-w-0 flex-1">
                  <div className="type-name text-[var(--color-text-primary)]">{e.name}</div>
                  <p className={SETTING_ROW_HINT}>{e.summary}</p>
                  <p className={`${SETTING_ROW_HINT} flex flex-wrap items-center gap-x-1.5`} data-testid="catalog-publisher">
                    <span>By {e.publisher.name}</span>
                    {e.version ? <span>· {e.version}</span> : null}
                    {e.verified ? (
                      <>
                        <span aria-hidden="true">·</span>
                        <VerifiedMark>{e.verified}</VerifiedMark>
                      </>
                    ) : null}
                  </p>
                  <p className={`${SETTING_ROW_HINT} flex flex-wrap items-center gap-x-1.5`} data-testid="catalog-state">
                    <span className="inline-flex items-center gap-1.5">
                      <StatusDot tone={st.tone} />
                      {st.words}
                    </span>
                    <span>· {hostOf(e.defaultAddress.url) ?? e.defaultAddress.label}</span>
                  </p>
                  {own.map((r) => (
                    <p key={r.code} className={SETTING_SERVER_REASON} data-testid="catalog-reason" data-code={r.code}>
                      {reasonWords(r)}
                    </p>
                  ))}
                </div>
                {isStaff ? (
                  <div className="flex-shrink-0">
                    <Button
                      ref={(el) => {
                        if (el) openers.current.set(e.entry, el)
                        else openers.current.delete(e.entry)
                      }}
                      variant={shown ? 'default' : running ? 'default' : 'accent'}
                      disabled={!running && !e.canDeploy}
                      aria-label={running ? `Show ${e.name}’s progress` : `Add ${e.name}`}
                      aria-expanded={shown}
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
            // Back to the row's button, so the keyboard doesn't fall to the top of Settings.
            const back = openers.current.get(opened.entry)
            setOpen(null)
            back?.focus()
            void refresh()
          }}
        />
      ) : null}
    </div>
  )
}
