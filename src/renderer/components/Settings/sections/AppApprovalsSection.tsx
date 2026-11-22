// Settings → Cores → App Approvals (prd-k2-app-runtime-v1 §5.6, 0.46.1 door).
//
// Agents propose, an owner or admin approves in person (0.46.13, Rosson
// 2026-11-15: an Admin login confirms with its own login password; the
// daemon's `canApprove` says whether this login may). Every held App request
// (config, publish run, App key, clean mode, egress, deploy/install caps,
// raw database, overcommit, manage, quota) waits here in plain words:
//   - Approve = a fresh vault owner confirm for THAT request (action
//     `app-approve`, `secretId` = the request id; Touch ID / login password
//     on the Mac, the Connect or vault password elsewhere), then
//     POST /cli/app/approve with the one-use token.
//   - Deny = POST /cli/app/deny with an optional note the requester reads.
// AR43: with no vault password and no Owner Connect login the daemon can't
// take a confirm, so Approve is off and the daemon's hint shows.
// Thin client: no polling; it reloads on open, after a gesture, when the
// window regains focus, and on the daemon's `app_approvals_changed` push
// (0.46.4: a request an agent files while this is open shows at once).
// 0.46.5: every server the person owns, not only the window's
// (`lib/app-approvals-servers.ts`): one block per server, each read,
// approved (with THAT server's owner confirm) and denied on its own server.
// With only the window's server it looks exactly as before.

import React, { useCallback, useEffect, useRef, useState } from 'react'
import { Button, Callout, Input, Surface, Textarea, cx } from '@/components/ui'
import { formatRelativeTime } from '@/lib/format-relative-time'
import { daemonErrorOf, daemonErrorText } from '@/lib/daemon-error'
import { K2_CAPS } from '@/lib/k2-caps.generated'
import type { SettingEntry } from '../searchManifest'
import { vaultConfirmOn } from './vault-api'
import { currentApprovalServers, useAppApprovalsStore } from '@/stores/app-approvals'
import { fetchApprovalsAcross, type ServerApprovals } from '@/lib/app-approvals-servers'
import type { ServerScope } from '@/kessel/server-scope'
import {
  APP_APPROVE_CONFIRM_ACTION,
  approveAppRequest,
  capsAfter,
  cardLine,
  denyAppRequest,
  diffLines,
  expiryText,
  fetchAppApprovals,
  kindLabel,
  resultError,
  type AppRequest,
} from './app-approvals-api'
import { touchIdOrPassword } from '@/lib/desktop-words'
import { useSettingsStore } from '@/stores/settings'
import { SETTING_LIST_ROW_HOVER, SETTING_ROW_HINT, SETTING_ROW_LABEL, SETTING_SERVER_REASON, SettingsGroup, SettingsLoadState } from '../controls/SettingControls'
import { AccessSheet, DIALOG_FIELD, Field } from './AccessDialogs'
import { AppMonogram, StatusDot, type DotTone } from './apps-ui'
import { confirmPasswordLabel, LoginUsernameField, readConfirmIdentity, scopeServerName, type ConfirmIdentity } from './confirm-password-ui'
import { PRODUCT_NAME } from '@/lib/brand'

export const APP_APPROVALS_MANIFEST: SettingEntry[] = [
  {
    id: 'app-approvals.list',
    section: 'app-approvals',
    label: 'App Approvals',
    description: 'Held App requests (deploy caps, egress, quotas, keys): approve in person or deny',
    keywords: ['approvals', 'approve', 'deny', 'held', 'app', 'apps', 'deploy', 'install', 'caps', 'egress', 'quota', 'request'],
  },
]

const DENY_MAX = 500
const NO_CONFIRM_HINT =
  `You can’t approve yet: this server has no way to confirm it’s you. Set a vault password on the Vault page, then come back. An owner or admin can also approve from their own ${PRODUCT_NAME} Connect login, with its password.`
/** A login that may read and deny but not approve (a Member). */
const APPROVERS_ONLY_HINT = 'Only the server’s owner or an admin approves App requests.'

function nowSec(): number {
  return Math.floor(Date.now() / 1000)
}

/** What a kind of request asks for, in plain words. */
const KIND_WORDS: Record<string, string> = {
  config: 'Change its settings',
  publish_run: 'Run a published service',
  app_key: 'Use an App key',
  clean_mode: 'Change how it cleans up',
  egress: 'Reach an outside site',
  egress_mode: 'Change its outside access',
  deploy: 'Deploy a new version',
  install: 'Install',
  db_raw: 'Use its database directly',
  overcommit: 'Use more memory than the server has free',
  manage: 'Manage it end to end',
  quota: 'Raise its limits',
}
function kindWords(kind: string): string {
  return KIND_WORDS[kind] ?? kindLabel(kind)
}

function daysLeft(expiresAt: number, now: number): string {
  return expiryText(expiresAt, now).replace(/(\d+)d$/, (_m, n: string) => `${n}d`)
}

export function AppApprovalsSection(): React.JSX.Element {
  const [groups, setGroups] = useState<ServerApprovals[] | null>(null)
  const [tab, setTab] = useState<'waiting' | 'history'>('waiting')
  const showHistory = tab === 'history'
  const [notice, setNotice] = useState<Notice | null>(null)

  const load = useCallback(async () => {
    const r = await fetchApprovalsAcross(
      currentApprovalServers(),
      showHistory ? 'all' : 'pending',
      (state, scope) => fetchAppApprovals(state, scope),
      daemonErrorText,
    )
    setGroups(r)
  }, [showHistory])

  useEffect(() => {
    void load()
    const onFocus = (): void => void load()
    window.addEventListener('focus', onFocus)
    return () => window.removeEventListener('focus', onFocus)
  }, [load])

  // The daemon's push (a request filed, asked again or decided), or the
  // set of owned servers changed.
  const revision = useAppApprovalsStore((s) => s.revision)
  const firstRevision = useRef(revision)
  useEffect(() => {
    if (revision !== firstRevision.current) void load()
  }, [revision, load])

  const multi = (groups?.length ?? 0) > 1
  const pendingOf = (g: ServerApprovals): AppRequest[] => (g.list?.requests ?? []).filter((r) => r.status === 'pending')
  const total = (groups ?? []).reduce((n, g) => n + pendingOf(g).length, 0)

  const tabs: { id: 'waiting' | 'history'; label: string; testId?: string }[] = [
    { id: 'waiting', label: total > 0 ? `Waiting (${total})` : 'Waiting' },
    { id: 'history', label: 'History', testId: 'app-approvals-history-toggle' },
  ]

  return (
    <div className="max-w-[52rem] space-y-6 pb-6" data-testid="app-approvals" data-settings-id="app-approvals.list">
      <header className="space-y-3">
        <div>
          <h2 className="type-title text-[var(--color-text-primary)]">
            App Approvals{total > 0 ? ` (${total})` : ''}
          </h2>
          <p className={SETTING_ROW_HINT}>
            Changes to an App that only the server&rsquo;s owner or an admin may allow wait here: new permissions, outside
            sites, higher limits and more. Agents can only ask. You approve each one in person, with {touchIdOrPassword()}
            (an admin types their own login password). Requests expire after 7 days.
            {multi ? ' Every server you own or admin is listed; each one is approved on its own server.' : ''}
          </p>
        </div>
        <div role="tablist" aria-label="App Approvals" className="flex flex-wrap gap-0.5 border-b border-[var(--color-border)] w-full">
          {tabs.map((t) => {
            const active = tab === t.id
            return (
              <button
                key={t.id}
                role="tab"
                type="button"
                aria-selected={active}
                data-testid={t.testId}
                onClick={() => {
                  setNotice(null)
                  setTab(t.id)
                }}
                data-k2-focus=""
                className={cx(
                  'relative px-3 py-2 type-control transition-colors no-drag cursor-pointer border-b-2 -mb-px outline-none',
                  active
                    ? 'border-[var(--color-accent)] text-[var(--color-text-primary)]'
                    : 'border-transparent text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)]',
                )}
              >
                {t.label}
              </button>
            )
          })}
        </div>
      </header>
      {notice ? (
        <Callout tone={notice.tone} className="type-hint" role={notice.tone === 'error' ? 'alert' : 'status'} data-testid="app-approvals-notice">
          {notice.text}
        </Callout>
      ) : null}
      {groups === null ? (
        <SettingsLoadState kind="loading" title={showHistory ? 'Loading history…' : 'Loading requests…'} testId="app-approvals-loading" />
      ) : null}
      {(groups ?? []).map((g) =>
        showHistory ? (
          <HistoryBlock key={g.server.hostKey} group={g} multi={multi} onRetry={() => void load()} />
        ) : (
          <ServerBlock
            key={g.server.hostKey}
            group={g}
            multi={multi}
            onRetry={() => void load()}
            onDone={async (note) => {
              setNotice(note)
              await load()
            }}
          />
        ),
      )}
    </div>
  )
}

function ServerHeading({ g, multi, waiting }: { g: ServerApprovals; multi: boolean; waiting: number | null }): React.JSX.Element | null {
  if (!multi) return null
  return (
    <h3 className="type-heading text-[var(--color-text-primary)] flex items-baseline gap-2">
      {g.server.label}
      {g.server.isWindow ? <span className={SETTING_ROW_HINT}>this window&apos;s server</span> : null}
      {waiting === 0 ? <span className={SETTING_ROW_HINT}>nothing waiting</span> : null}
    </h3>
  )
}

/** One server's pending requests (with its own confirm hint and error). */
function ServerBlock({
  group: g,
  multi,
  onDone,
  onRetry,
}: {
  group: ServerApprovals
  multi: boolean
  onDone: (notice: Notice) => Promise<void>
  onRetry: () => void
}): React.JSX.Element {
  const ownerConfirm = g.list?.ownerConfirm ?? null
  // A Member login reads and denies; only an owner or admin approves.
  const canApprove = g.list?.canApprove !== false
  const canConfirm = canApprove && ownerConfirm?.available === true
  const pending = (g.list?.requests ?? []).filter((r) => r.status === 'pending')
  return (
    <section className="space-y-3" data-testid={`app-approvals-server-${g.server.hostKey}`}>
      <ServerHeading g={g} multi={multi} waiting={g.list ? pending.length : null} />
      {g.list && !canApprove && pending.length > 0 ? (
        <p className={SETTING_ROW_HINT} data-testid="app-approvals-approvers-only">
          {APPROVERS_ONLY_HINT}
        </p>
      ) : null}
      {canApprove && ownerConfirm && !canConfirm && (pending.length > 0 || !multi) ? (
        <Callout tone="warn" className="type-hint flex items-center justify-between gap-4">
          <span data-testid="app-approvals-no-confirm">{NO_CONFIRM_HINT}</span>
          <Button onClick={() => useSettingsStore.getState().openSettings('vault')} data-testid="app-approvals-open-vault">
            Open Vault
          </Button>
        </Callout>
      ) : null}
      {g.error ? (
        <SettingsLoadState
          kind="error"
          title={multi ? `${PRODUCT_NAME} couldn’t read the requests on ${g.server.label}` : `${PRODUCT_NAME} couldn’t read the waiting requests`}
          detail={g.error}
          testId={`app-approvals-error-${g.server.hostKey}`}
          action={<Button onClick={onRetry}>Try again</Button>}
        >
          Nothing was approved or denied. Try again in a moment.
        </SettingsLoadState>
      ) : g.list && pending.length === 0 && !multi ? (
        <SettingsLoadState kind="empty" title="Nothing is waiting" testId="app-approvals-empty">
          When an agent asks for a change only you can allow, it shows up here.
        </SettingsLoadState>
      ) : null}
      {pending.map((r) => (
        <RequestCard
          key={r.id}
          request={r}
          scope={g.server.scope}
          canApprove={canApprove}
          canConfirm={canConfirm}
          methods={ownerConfirm?.methods ?? []}
          identity={{ server: ownerConfirm?.server ?? null, login: ownerConfirm?.login ?? null }}
          onDone={onDone}
        />
      ))}
    </section>
  )
}

/** One server's decided requests. */
function HistoryBlock({ group: g, multi, onRetry }: { group: ServerApprovals; multi: boolean; onRetry: () => void }): React.JSX.Element | null {
  if (g.error) {
    return (
      <SettingsLoadState
        kind="error"
        title={multi ? `${PRODUCT_NAME} couldn’t read the history on ${g.server.label}` : `${PRODUCT_NAME} couldn’t read the history`}
        detail={g.error}
        action={<Button onClick={onRetry}>Try again</Button>}
      >
        Try again in a moment.
      </SettingsLoadState>
    )
  }
  if (!g.list) return null
  const history = g.list.requests.filter((r) => r.status !== 'pending')
  return (
    <SettingsGroup
      card
      title={multi ? g.server.label : 'Decided requests'}
      hint="The last requests approved, denied or left to expire."
      testId={multi ? `app-approvals-history-${g.server.hostKey}` : 'app-approvals-history'}
      state={
        history.length === 0 ? (
          <SettingsLoadState kind="empty" title="No decided requests yet">
            What you approve or deny shows up here.
          </SettingsLoadState>
        ) : undefined
      }
    >
      {history.map((r) => (
        <HistoryRow key={r.id} request={r} />
      ))}
    </SettingsGroup>
  )
}

/** What the last gesture did. An approval whose apply failed leaves the
 *  pending list (it is decided), so its error shows here and in History. */
type Notice = { tone: 'ok' | 'error'; text: string }

type ConfirmStep = { step: 'review' } | { step: 'starting' } | { step: 'mac' } | { step: 'password'; method: string; started: boolean }

/** One change line in plain words: "New outside site: api.example.com". */
export function plainChange(line: string): string {
  const host = /^\+ host (.+)$/.exec(line)
  if (host) return `New outside site: ${host[1].replace(/^https?:\/\//, '').replace(/:443$/, '')}`
  const cap = /^\+ cap (.+)$/.exec(line)
  if (cap) {
    const label = (K2_CAPS as Record<string, { label: string } | undefined>)[cap[1]]?.label
    return `New permission: ${label ? label.replace(/\s*\(.*\)\s*$/, '') : cap[1]}`
  }
  const mode = /^Egress mode → (.+)$/.exec(line)
  if (mode) return `Outside access: ${mode[1]}`
  return line
}

/** What approving did, in plain words. */
function approvedWords(r: AppRequest): string {
  const hosts = Array.isArray(r.hosts) && r.hosts.length ? r.hosts.map((h) => h.replace(/:443$/, '')) : null
  if (hosts) return `Approved: ${r.appLabel} can now reach ${hosts.join(', ')}.`
  const what = r.summary.trim().replace(/[.\s]+$/, '')
  return `Approved: ${r.appLabel} can now ${what.charAt(0).toLowerCase()}${what.slice(1)}.`
}

/** How the owner confirmed, in words. */
const CONFIRM_WORDS: Record<string, string> = {
  'vault-password': 'the vault password',
  'connect-password': 'their sign-in password',
  'local-auth': 'Touch ID or the Mac password',
}

/** The change itself: what it adds, one line each, in a quiet box. */
function ChangeLines({ r }: { r: AppRequest }): React.JSX.Element | null {
  const lines = diffLines(r)
  const after = capsAfter(r)
  if (lines.length === 0 && after.length === 0) return null
  return (
    <div className="space-y-1">
      {lines.length > 0 ? (
        <ul
          data-k2-chamfer="surface"
          className="type-hint list-none space-y-0.5 border border-[var(--color-border)] bg-[var(--color-bg)] rounded-[var(--radius-field)] px-3 py-2 text-[var(--color-text-primary)]"
          data-testid="app-request-diff"
        >
          {lines.map((l, i) => (
            <li key={i}>{plainChange(l)}</li>
          ))}
        </ul>
      ) : null}
      {after.length > 0 ? <p className={SETTING_ROW_HINT}>After approval it holds: {after.join(', ')}</p> : null}
    </div>
  )
}

function RequestCard({
  request: r,
  scope,
  canApprove = true,
  canConfirm,
  methods = [],
  identity,
  onDone,
}: {
  request: AppRequest
  /** The server this request is held on: read, confirmed, approved and
   *  denied there. */
  scope: ServerScope
  /** This login may approve here (an owner or admin); a Member only
   *  reads and denies, so no Approve button. */
  canApprove?: boolean
  canConfirm: boolean
  /** How the server confirms it's the owner (the list's ownerConfirm): a
   *  password method with no Touch ID puts the password in the dialog at
   *  once, so approving is one more press. */
  methods?: string[]
  /** Who confirms where (the list's ownerConfirm), for the password
   *  prompt; `vault/confirm/start` refreshes it. */
  identity?: ConfirmIdentity
  onDone: (notice: Notice) => Promise<void>
}): React.JSX.Element {
  const [dialog, setDialog] = useState<'approve' | 'deny' | null>(null)
  const [confirm, setConfirm] = useState<ConfirmStep>({ step: 'review' })
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  /** The password didn't match: say so plainly and keep the field. */
  const [badPassword, setBadPassword] = useState(false)
  const [note, setNote] = useState('')
  const [who, setWho] = useState<ConfirmIdentity>(identity ?? {})
  const pwFirst = methods.includes('local-auth') ? undefined : methods.find((m) => m === 'connect-password' || m === 'vault-password')
  const firstStep = (): ConfirmStep => (pwFirst ? { step: 'password', method: pwFirst, started: false } : { step: 'review' })
  const pwRef = useRef<HTMLInputElement>(null)
  const [hasPw, setHasPw] = useState(false)
  const now = nowSec()

  const close = (): void => {
    setDialog(null)
    setConfirm({ step: 'review' })
    setError(null)
    setBadPassword(false)
    setNote('')
    setHasPw(false)
  }

  const finish = async (method: string, password?: string): Promise<void> => {
    const { token } = await vaultConfirmOn(scope).finish({
      action: APP_APPROVE_CONFIRM_ACTION,
      secretId: r.id,
      method,
      label: `${r.appLabel}: ${r.summary}`.slice(0, 64),
      ...(password !== undefined ? { password } : {}),
    })
    const out = await approveAppRequest(r.id, token, scope)
    close()
    if (out.error) {
      // Recorded as approved, but applying it failed: the daemon says why.
      await onDone({
        tone: 'error',
        text: `Approved, but it didn’t apply: ${(out.error.hint || out.error.code).replace(/[.\s]+$/, '')}. ${r.appLabel} stays as it was; its agent can ask again.`,
      })
      return
    }
    await onDone({ tone: 'ok', text: approvedWords(r) })
  }

  const approve = async (): Promise<void> => {
    setError(null)
    setConfirm({ step: 'starting' })
    try {
      const started = await vaultConfirmOn(scope).start(APP_APPROVE_CONFIRM_ACTION, r.id)
      const { methods } = started
      const named = readConfirmIdentity(started)
      if (named.login || named.server) setWho((w) => ({ server: named.server ?? w.server, login: named.login ?? w.login }))
      if (methods.includes('local-auth')) {
        setConfirm({ step: 'mac' })
        await finish('local-auth')
        return
      }
      const pw = methods.find((m) => m === 'connect-password' || m === 'vault-password')
      if (pw) {
        setConfirm({ step: 'password', method: pw, started: true })
        return
      }
      setConfirm({ step: 'review' })
      setError(NO_CONFIRM_HINT)
    } catch (e) {
      setConfirm({ step: 'review' })
      setError(daemonErrorText(e))
    }
  }

  const submitPassword = async (method: string, started: boolean): Promise<void> => {
    const el = pwRef.current
    if (!el) return
    const pw = el.value
    el.value = ''
    setHasPw(false)
    setError(null)
    setBadPassword(false)
    try {
      if (!started) await vaultConfirmOn(scope).start(APP_APPROVE_CONFIRM_ACTION, r.id)
      await finish(method, pw)
    } catch (e) {
      setConfirm({ step: 'password', method, started: false })
      if (daemonErrorOf(e)?.code === 'confirm_failed') setBadPassword(true)
      else setError(daemonErrorText(e))
    }
  }

  const deny = async (): Promise<void> => {
    setBusy(true)
    setError(null)
    try {
      await denyAppRequest(r.id, note.trim() || undefined, scope)
      close()
      await onDone({
        tone: 'ok',
        text: `Denied: ${r.appLabel} stays as it is. ${note.trim() ? `${r.requestedBy} can see why.` : `${r.requestedBy} can ask again later.`}`,
      })
    } catch (e) {
      setError(daemonErrorText(e))
    } finally {
      setBusy(false)
    }
  }

  const working = confirm.step === 'starting' || confirm.step === 'mac'
  return (
    <Surface chamfer material="tile" className="px-4 py-3 space-y-2" data-testid={`app-request-${r.id}`}>
      <div className="flex items-start gap-3">
        <AppMonogram name={r.appLabel} />
        <div className="min-w-0 flex-1 space-y-1">
          <div className="flex items-baseline gap-x-2 flex-wrap">
            <span className="type-name text-[var(--color-text-primary)]">{r.appLabel}</span>
            <span className={SETTING_ROW_HINT} data-kind={r.kind}>
              {kindWords(r.kind)}
              <span className="sr-only"> ({kindLabel(r.kind)})</span>
            </span>
          </div>
          <div className="type-row text-[var(--color-text-primary)]">{r.summary}</div>
          <ChangeLines r={r} />
          {r.reason ? <p className={SETTING_ROW_HINT}>Why: {r.reason}</p> : null}
          <p className={SETTING_ROW_HINT}>
            asked by {r.requestedBy}
            {r.workspaceHandle ? ` for ${r.workspaceHandle}` : ''} · {formatRelativeTime(r.requestedAt, now)} · {daysLeft(r.expiresAt, now)}
          </p>
          {cardLine(r.card) ? (
            <p className={SETTING_ROW_HINT} data-testid="app-request-card">
              {cardLine(r.card)}
            </p>
          ) : null}
          {r.reasons.length > 0 ? (
            <p className={SETTING_SERVER_REASON} data-testid="app-request-reasons">
              Held because: {r.reasons.join('; ')}
            </p>
          ) : null}
        </div>
        <div className="flex items-center gap-2 flex-shrink-0">
          <Button
            variant="danger-quiet"
            onClick={() => {
              setError(null)
              setDialog('deny')
            }}
            data-testid={`app-deny-${r.id}`}
          >
            Deny…
          </Button>
          {canApprove ? (
            <Button
              variant="accent"
              disabled={!canConfirm}
              title={canConfirm ? undefined : 'No way to confirm on this server yet'}
              onClick={() => {
                setError(null)
                setBadPassword(false)
                setConfirm(firstStep())
                setDialog('approve')
              }}
              data-testid={`app-approve-${r.id}`}
            >
              Approve…
            </Button>
          ) : null}
        </div>
      </div>

      {dialog === 'approve' ? (
        <AccessSheet
          title={`Approve this request from ${r.requestedBy}?`}
          intro={`${r.appLabel}: ${r.summary}. It applies as soon as you confirm it’s you.`}
          onClose={close}
          busy={working}
          testId={`app-approve-dialog-${r.id}`}
          footer={
            <>
              <Button variant="ghost" size="md" onClick={close} disabled={working}>
                Cancel
              </Button>
              {confirm.step === 'password' ? (
                <Button type="submit" form={`approve-pw-${r.id}`} variant="accent" size="md" disabled={!hasPw} data-testid={`app-approve-submit-${r.id}`}>
                  Approve
                </Button>
              ) : (
                <Button
                  variant="accent"
                  size="md"
                  autoFocus
                  disabled={working}
                  onClick={() => void approve()}
                  data-testid={`app-approve-confirm-${r.id}`}
                >
                  {confirm.step === 'mac' ? 'Confirm on your Mac…' : confirm.step === 'starting' ? 'Asking…' : 'Approve'}
                </Button>
              )}
            </>
          }
        >
          <ChangeLines r={r} />
          {r.reason ? <p className={SETTING_ROW_HINT}>Why: {r.reason}</p> : null}
          {confirm.step === 'review' ? (
            <p className={SETTING_ROW_HINT}>You confirm it&rsquo;s you with {touchIdOrPassword()} next.</p>
          ) : null}
          {confirm.step === 'mac' ? <p className={SETTING_ROW_LABEL}>Confirm on your Mac to approve.</p> : null}
          {confirm.step === 'password' ? (
            <form
              id={`approve-pw-${r.id}`}
              onSubmit={(e) => {
                e.preventDefault()
                void submitPassword(confirm.method, confirm.started)
              }}
            >
              <LoginUsernameField method={confirm.method} who={who} testId="app-approve-username" />
              <Field
                label={confirmPasswordLabel(confirm.method, who, scopeServerName(scope))}
                htmlFor={`approve-pw-input-${r.id}`}
                hint={badPassword ? undefined : `${PRODUCT_NAME} uses it once, to confirm it’s you.`}
                problem={badPassword ? 'That password didn’t match. Try again.' : null}
              >
                <Input
                  id={`approve-pw-input-${r.id}`}
                  ref={pwRef}
                  type="password"
                  autoFocus
                  autoComplete="current-password"
                  data-testid="app-approve-password"
                  onChange={(e) => setHasPw(e.currentTarget.value !== '')}
                  className={DIALOG_FIELD}
                />
              </Field>
            </form>
          ) : null}
          {error ? (
            <Callout tone="error" role="alert" className="type-hint space-y-0.5">
              <p className="font-medium">Couldn&rsquo;t approve it. Nothing changed.</p>
              <p data-testid={`app-request-error-${r.id}`}>{error}</p>
            </Callout>
          ) : null}
        </AccessSheet>
      ) : null}

      {dialog === 'deny' ? (
        <AccessSheet
          title={`Deny this request from ${r.requestedBy}?`}
          intro={`${r.appLabel}: ${r.summary}. Nothing changes on the App; ${r.requestedBy} can ask again later.`}
          onClose={close}
          busy={busy}
          testId={`app-deny-dialog-${r.id}`}
          footer={
            <>
              <Button variant="ghost" size="md" onClick={close} disabled={busy} autoFocus>
                Cancel
              </Button>
              <Button variant="danger-muted" size="md" disabled={busy} onClick={() => void deny()} data-testid={`app-deny-send-${r.id}`}>
                {busy ? 'Denying…' : 'Deny'}
              </Button>
            </>
          }
        >
          <Field label="Why (optional)" htmlFor={`deny-note-${r.id}`} hint={`${r.requestedBy} reads this. ${note.length}/${DENY_MAX}`}>
            <Textarea
              id={`deny-note-${r.id}`}
              value={note}
              maxLength={DENY_MAX}
              rows={3}
              onChange={(e) => setNote(e.target.value.slice(0, DENY_MAX))}
              aria-label="Why deny"
              className={DIALOG_FIELD}
              data-testid={`app-deny-note-${r.id}`}
            />
          </Field>
          {error ? (
            <Callout tone="error" role="alert" className="type-hint space-y-0.5">
              <p className="font-medium">Couldn&rsquo;t deny it. It&rsquo;s still waiting.</p>
              <p data-testid={`app-request-error-${r.id}`}>{error}</p>
            </Callout>
          ) : null}
        </AccessSheet>
      ) : null}
    </Surface>
  )
}

const STATUS_WORDS: Record<string, string> = {
  approved: 'Approved',
  denied: 'Denied',
  withdrawn: 'Withdrawn',
  expired: 'Expired',
  failed: 'Approved, failed to apply',
}
const STATUS_DOT: Record<string, DotTone> = { approved: 'ok', denied: 'idle', withdrawn: 'idle', expired: 'idle', failed: 'error' }

function HistoryRow({ request: r }: { request: AppRequest }): React.JSX.Element {
  const failed = r.status === 'failed' ? resultError(r) : null
  const now = nowSec()
  return (
    <div className={`px-4 py-3 ${SETTING_LIST_ROW_HOVER}`} data-testid={`app-history-${r.id}`}>
      <div className="flex items-start gap-3">
        <AppMonogram name={r.appLabel} />
        <div className="min-w-0 flex-1">
          <div className="flex items-baseline gap-x-2 flex-wrap">
            <span className="type-name text-[var(--color-text-primary)]">{r.appLabel}</span>
            <span className="type-row text-[var(--color-text-primary)]">{r.summary}</span>
          </div>
          <p className={`${SETTING_ROW_HINT} flex flex-wrap items-center gap-x-1.5`}>
            <StatusDot tone={STATUS_DOT[r.status] ?? 'idle'} />
            <span className="text-[var(--color-text-primary)]">{STATUS_WORDS[r.status] ?? r.status}</span>
            {r.decidedBy || r.confirmVia ? (
              <span>
                {[r.decidedBy ? `by ${r.decidedBy}` : null, r.confirmVia ? `with ${CONFIRM_WORDS[r.confirmVia] ?? r.confirmVia}` : null]
                  .filter(Boolean)
                  .join(' ')}
              </span>
            ) : null}
            {r.decidedAt ? <span>· {formatRelativeTime(r.decidedAt, now)}</span> : null}
            <span>· asked by {r.requestedBy}</span>
          </p>
          {r.decisionReason ? <p className={SETTING_ROW_HINT}>Note: {r.decisionReason}</p> : null}
          {failed ? <p className="type-hint text-[var(--color-status-error-soft)] mt-0.5">{failed}</p> : null}
        </div>
      </div>
    </div>
  )
}
