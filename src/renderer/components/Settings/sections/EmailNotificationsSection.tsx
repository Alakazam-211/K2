// Settings → Cores → Email Notifications (prd-email-notifications-part2-v1
// §4 N8). The owner's view of the mail K2 sends from senders (resets,
// codes, App email, agent notices): the senders and their pauses, the
// workspaces whose agents may email people, the addresses K2 stopped
// sending to, and the recent log. Every rule lives in the daemon; this
// page only reads `/cli/notifications/email/*` and sends the owner's
// gestures, each behind a confirm: resume a paused sender (with a vault
// confirm bound to its address), remove a sender, turn a workspace's
// notices off, take an address off the stop list. Adding senders and
// relays stays on the CLI for now.
//
// The Settings page anatomy (as Access Audit): a heading and its hint, then
// card groups; a load, refusal or unreadable answer shows the shared load
// state in the page's place, never a crash.

import React, { useCallback, useEffect, useId, useMemo, useRef, useState } from 'react'
import type { ServerScope } from '@/kessel/server-scope'
import { Button, Callout, Input, OverflowMenu, SearchField, Tag } from '@/components/ui'
import { useProjectsStore } from '@/stores/projects'
import { useSettingsStore } from '@/stores/settings'
import type { SettingEntry } from '../searchManifest'
import {
  SETTING_LIST_ROW_HOVER,
  SETTING_ROW_HINT,
  SETTING_ROW_LABEL,
  SettingRow,
  SettingsGroup,
  SettingsLoadState,
} from '../controls/SettingControls'
import { AccessSheet, DIALOG_MUTED, Field } from './AccessDialogs'
import { localWhen } from '../shared/local-time'
import {
  emailNotifyApiOn,
  logErrorText,
  logStatusText,
  logWhatText,
  notifyErrorCode,
  notifyErrorMessage,
  notifyFailure,
  notifyLoadProblem,
  offReasonText,
  ownerText,
  pathText,
  pauseText,
  senderStateText,
  suppressionReasonText,
  suppressionScopeText,
  type NotifyAction,
  type NotifyAgent,
  type NotifyFailure,
  type NotifyLoadProblem,
  type NotifyLogItem,
  type NotifySender,
  type NotifyStatus,
  type NotifySuppression,
} from './email-notify-api'
import { PRODUCT_NAME } from '@/lib/brand'

export const EMAIL_NOTIFICATIONS_MANIFEST: SettingEntry[] = [
  { id: 'email-notifications.senders', section: 'email-notifications', label: 'Notification Senders', description: `The addresses ${PRODUCT_NAME} sends resets, codes, App email and agent notices from`, keywords: ['sender', 'noreply', 'system sender', 'relay', 'paused', 'resume', 'bounce'] },
  { id: 'email-notifications.agents', section: 'email-notifications', label: 'Agent Email Notices', description: 'Which workspaces may email the people on this server', keywords: ['agent', 'notice', 'notify', 'workspace', 'email'] },
  { id: 'email-notifications.suppressions', section: 'email-notifications', label: 'Stopped Addresses', description: `Addresses ${PRODUCT_NAME} stopped sending to after a bounce, a spam report or an unsubscribe`, keywords: ['suppression', 'suppressed', 'bounce', 'complaint', 'spam', 'unsubscribe'] },
]

const CONTENT_WIDTH = 'max-w-[52rem]'
const LOG_LIMIT = 30
/** The width of a row's "…" menu, kept free on a row without one so every
 *  row's tags end at the same edge. */
const MENU_SLOT = 'w-8 flex-shrink-0'

type EmailNotifyApi = ReturnType<typeof emailNotifyApiOn>
/** One group's data: loading, read, or failed with the server's reason. */
type Part<T> = { kind: 'loading' } | { kind: 'ok'; data: T } | { kind: 'error'; detail: string }
/** The confirmation under a group after its action landed. */
type Note = { group: 'senders' | 'agents' | 'stopped'; text: string }
type Ask =
  | { kind: 'resume'; sender: NotifySender }
  | { kind: 'remove-sender'; sender: NotifySender }
  | { kind: 'agent-off'; agent: NotifyAgent; name: string }
  | { kind: 'unsuppress'; address: string; count: number }

function when(unix: number | null | undefined): string {
  return typeof unix === 'number' && unix > 0 ? localWhen(unix * 1000) : '—'
}

function plural(n: number, one: string, many: string): string {
  return `${n} ${n === 1 ? one : many}`
}

/** A list row: label and one-line hint on the left, the row's state and
 *  actions on one right edge, the shared hover band. */
function Row({
  label,
  hint,
  right,
  below,
  testId,
}: {
  label: React.ReactNode
  hint?: React.ReactNode
  right?: React.ReactNode
  below?: React.ReactNode
  testId?: string
}): React.JSX.Element {
  return (
    <div className={`px-4 py-3 ${SETTING_LIST_ROW_HOVER}`} data-testid={testId}>
      <div className="flex items-start justify-between gap-6">
        <div className="flex-1 min-w-0">
          <div className={`${SETTING_ROW_LABEL} break-words`}>{label}</div>
          {hint && <div className={SETTING_ROW_HINT}>{hint}</div>}
        </div>
        {right && <div className="flex items-center gap-2 flex-shrink-0">{right}</div>}
      </div>
      {below && <div className="mt-2">{below}</div>}
    </div>
  )
}

/** What an action did, at the top of its card. */
function DoneNote({ text }: { text: string }): React.JSX.Element {
  return (
    <div className="px-4 py-3">
      <Callout tone="ok" role="status" className="type-row" data-testid="notify-done">
        {text}
      </Callout>
    </div>
  )
}

/** A failed action: what failed, the next step that fits the server's
 *  reason, and the server's own words quietly when they add something. */
function FailureNote({ what, failure }: { what: string; failure: NotifyFailure }): React.JSX.Element {
  return (
    <Callout tone="error" role="alert" className="type-hint space-y-0.5" data-testid="dialog-error">
      <p className="font-medium">{what}</p>
      <p>{failure.next}</p>
      {failure.detail && <p>The server said: {failure.detail.trim().replace(/[.\s]+$/, '')}.</p>}
    </Callout>
  )
}

/** A confirm-first action in the shared dialog frame: Cancel has the focus;
 *  while the write runs the dialog holds and the button says so; a refusal
 *  says what failed and the next step that fits it (Try again only where
 *  trying again can help, or the fix itself, e.g. turning notices off). */
function ConfirmAction({
  title,
  children,
  confirmLabel,
  busyLabel,
  destructive = false,
  failed,
  action,
  run,
  onDone,
  onClose,
  fixLabel,
  onFix,
  workspace,
  testId,
}: {
  title: string
  children: React.ReactNode
  confirmLabel: string
  busyLabel: string
  destructive?: boolean
  /** "Couldn't …" for the failure note. */
  failed: string
  action: NotifyAction
  run: () => Promise<string>
  onDone: (note: string) => void
  onClose: () => void
  /** The fix a failure offers, named for the page (a workspace's name). */
  fixLabel?: (fix: NonNullable<NotifyFailure['fix']>) => string
  onFix?: (fix: NonNullable<NotifyFailure['fix']>) => void
  /** A workspace's name for an id in the server's words. */
  workspace?: (id: string) => string
  testId: string
}): React.JSX.Element {
  const [busy, setBusy] = useState(false)
  const [failure, setFailure] = useState<NotifyFailure | null>(null)
  const go = async (): Promise<void> => {
    setBusy(true)
    setFailure(null)
    try {
      onDone(await run())
    } catch (e) {
      setFailure(notifyFailure(e, action, workspace))
      setBusy(false)
    }
  }
  const fix = failure?.fix && onFix && fixLabel ? failure.fix : null
  const primary = !failure ? confirmLabel : failure.retry
  return (
    <AccessSheet
      title={title}
      onClose={onClose}
      busy={busy}
      testId={testId}
      footer={
        <>
          <Button variant="ghost" size="md" onClick={onClose} disabled={busy} autoFocus>
            {failure && !primary ? 'Close' : 'Cancel'}
          </Button>
          {fix && (
            <Button variant="accent" size="md" onClick={() => onFix?.(fix)} data-testid={`${testId}-fix`}>
              {fixLabel?.(fix)}
            </Button>
          )}
          {(busy || primary) && (
            <Button variant={destructive ? 'danger-muted' : 'accent'} size="md" disabled={busy} onClick={() => void go()} data-testid={`${testId}-confirm`}>
              {busy ? busyLabel : primary}
            </Button>
          )}
        </>
      }
    >
      <div className={`${DIALOG_MUTED} space-y-2`}>{children}</div>
      {failure && <FailureNote what={failed} failure={failure} />}
    </AccessSheet>
  )
}

/** Resume a paused sender: the owner confirms it's them (Touch ID or their
 *  login where the server offers it, else the vault password), bound to
 *  the sender's address. */
function ResumeDialog({
  sender,
  api,
  onDone,
  onClose,
}: {
  sender: NotifySender
  api: EmailNotifyApi
  onDone: (note: string) => void
  onClose: () => void
}): React.JSX.Element {
  const [askPassword, setAskPassword] = useState(false)
  const [password, setPassword] = useState('')
  const [busy, setBusy] = useState(false)
  const [failure, setFailure] = useState<NotifyFailure | null>(null)
  const formId = useId()
  const fieldId = useId()
  const resume = async (): Promise<void> => {
    setBusy(true)
    setFailure(null)
    try {
      await api.resume(sender, askPassword ? password : undefined)
      onDone(`${sender.address} is sending again. ${PRODUCT_NAME} counts its bounces from zero from now on.`)
    } catch (e) {
      setBusy(false)
      // No Touch ID on that server: ask for the vault password, quietly.
      if (!askPassword && notifyErrorCode(e) === 'password_needed') {
        setAskPassword(true)
        return
      }
      setFailure(notifyFailure(e, 'resume'))
    }
  }
  const primary = !failure ? 'Resume sending' : failure.retry
  return (
    <AccessSheet
      title={`Resume ${sender.address}?`}
      onClose={onClose}
      busy={busy}
      testId="notify-resume-dialog"
      footer={
        <>
          <Button variant="ghost" size="md" onClick={onClose} disabled={busy} autoFocus={!askPassword}>
            {failure && !primary ? 'Close' : 'Cancel'}
          </Button>
          {(busy || primary) && (
            <Button
              type="submit"
              form={formId}
              variant="accent"
              size="md"
              disabled={busy || (askPassword && !password)}
              data-testid="notify-resume-confirm"
            >
              {busy ? 'Resuming…' : primary}
            </Button>
          )}
        </>
      }
    >
      <form
        id={formId}
        className="space-y-4"
        onSubmit={(e) => {
          e.preventDefault()
          if (primary) void resume()
        }}
      >
        <div className={`${DIALOG_MUTED} space-y-2`}>
          {sender.paused && <p>{PRODUCT_NAME} paused it because {pauseText(sender.paused)}.</p>}
          <p>
            Once resumed, its email goes out again. If whatever made it bounce isn’t fixed, {PRODUCT_NAME} pauses it again. You confirm it’s
            you before it resumes.
          </p>
        </div>
        {askPassword && (
          <Field label="Vault password" htmlFor={fieldId} hint="This server confirms with its vault password.">
            <Input
              id={fieldId}
              type="password"
              autoFocus
              className="w-full type-control"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              autoComplete="current-password"
            />
          </Field>
        )}
        {failure && <FailureNote what={`Couldn’t resume ${sender.address}`} failure={failure} />}
      </form>
    </AccessSheet>
  )
}

function SenderRow({
  s,
  system = false,
  onResume,
  onRemove,
}: {
  s: NotifySender
  system?: boolean
  onResume: () => void
  onRemove: () => void
}): React.JSX.Element {
  const st = senderStateText(s.state)
  const what = system ? `${PRODUCT_NAME}’s own email: password resets and sign-in codes` : ownerText(s)
  const route = s.envelope ? `${pathText(s.path)}, bounces to ${s.envelope}` : pathText(s.path)
  return (
    <Row
      testId={`notify-sender-${s.id}`}
      label={s.address}
      hint={`${what} · Sends ${route}`}
      right={
        <>
          {s.paused ? <Tag data-case="as-written" tone="warn">Paused</Tag> : <Tag data-case="as-written" tone={st.tone}>{st.text}</Tag>}
          {s.paused && !system && (
            <Button onClick={onResume} data-testid={`notify-resume-${s.id}`}>
              Resume…
            </Button>
          )}
          {system ? (
            <span aria-hidden="true" className={MENU_SLOT} />
          ) : (
            <OverflowMenu
              label={`More actions for ${s.address}`}
              triggerProps={{ 'data-testid': `notify-more-${s.id}` }}
              items={[{ id: 'remove', label: 'Remove sender…', hint: 'It stops sending at once', danger: true, onSelect: onRemove }]}
            />
          )}
        </>
      }
      below={
        s.paused ? (
          <p className="type-hint text-[var(--color-status-warn-amber-soft)] leading-relaxed" data-testid={`notify-paused-${s.id}`}>
            Paused {when(s.paused.pausedAt)}: {pauseText(s.paused)}. Nothing it sends goes out until you resume it.
          </p>
        ) : s.state === 'needs_dmarc' ? (
          <div className="flex items-start justify-between gap-6" data-testid={`notify-needs-dns-${s.id}`}>
            <p className={`${SETTING_ROW_HINT} mt-0`}>
              Its domain needs a DMARC record: a DNS text record named _dmarc.{s.domain} that tells inboxes its mail is real. Add it
              where {s.domain}’s DNS is managed. For a domain this server hosts, Email Hosting shows the exact record.
            </p>
            <Button className="flex-shrink-0" onClick={() => useSettingsStore.getState().openSettings('email-hosting')}>
              Open Email Hosting
            </Button>
          </div>
        ) : s.state === 'needs_test' ? (
          <p className={`${SETTING_ROW_HINT} mt-0`} data-testid={`notify-needs-test-${s.id}`}>
            It sends once a test email to the owner goes through. The owner sends one from a terminal on the server.
          </p>
        ) : undefined
      }
    />
  )
}

/** `scope`: the server this page shows. */
export function EmailNotificationsSection({ scope }: { scope: ServerScope }): React.JSX.Element {
  const api = useMemo(() => emailNotifyApiOn(scope), [scope])
  const projects = useProjectsStore((st) => st.projects)
  const [status, setStatus] = useState<NotifyStatus | null>(null)
  const [problem, setProblem] = useState<NotifyLoadProblem | null>(null)
  const [agents, setAgents] = useState<Part<NotifyAgent[]>>({ kind: 'loading' })
  const [sups, setSups] = useState<Part<NotifySuppression[]>>({ kind: 'loading' })
  const [log, setLog] = useState<Part<NotifyLogItem[]>>({ kind: 'loading' })
  const [search, setSearch] = useState('')
  /** The address the stop list shows, or '' for the whole list. */
  const [searched, setSearchedState] = useState('')
  const searchedRef = useRef('')
  const setSearched = (q: string): void => {
    searchedRef.current = q
    setSearchedState(q)
  }
  const [note, setNote] = useState<Note | null>(null)
  const [ask, setAsk] = useState<Ask | null>(null)
  const [busy, setBusy] = useState(false)

  const partOf = <T,>(r: PromiseSettledResult<T>): Part<T> =>
    r.status === 'fulfilled' ? { kind: 'ok', data: r.value } : { kind: 'error', detail: notifyErrorMessage(r.reason) }

  const loadSups = useCallback(
    async (address: string): Promise<void> => {
      setSups({ kind: 'loading' })
      const [r] = await Promise.allSettled([api.suppressions(address)])
      setSups(partOf(r))
    },
    [api],
  )

  const load = useCallback(async (): Promise<void> => {
    setBusy(true)
    const [st, ag, su, lg] = await Promise.allSettled([api.status(), api.agents(), api.suppressions(searchedRef.current), api.log(LOG_LIMIT)])
    setBusy(false)
    if (st.status === 'rejected') {
      setProblem(notifyLoadProblem(st.reason))
      return
    }
    setProblem(null)
    setStatus(st.value)
    setAgents(partOf(ag))
    setSups(partOf(su))
    setLog(partOf(lg))
  }, [api])

  useEffect(() => {
    void load()
  }, [load])

  const workspaceName = useCallback(
    (key: string): string => projects.find((p) => p.id === key || p.handle === key || p.name === key)?.name ?? key,
    [projects],
  )

  const done = (group: Note['group']) => (text: string): void => {
    setAsk(null)
    setNote({ group, text })
    void load()
  }

  const tryAgain = (
    <Button onClick={() => void load()} disabled={busy}>
      {busy ? 'Trying…' : 'Try again'}
    </Button>
  )

  const header = (
    <header className="min-w-0" data-settings-id="email-notifications.senders">
      <h2 className="type-title text-[var(--color-text-primary)]">Email Notifications</h2>
      <p className={SETTING_ROW_HINT}>
        The addresses this server sends password resets, sign-in codes, App email and agent notices from, and the addresses it
        has stopped sending to.
      </p>
    </header>
  )

  if (problem || !status) {
    const gate = !problem ? (
      <SettingsLoadState kind="loading" title="Loading email notifications…" testId="email-notifications-loading">
        Senders, stopped addresses and recent email on this server.
      </SettingsLoadState>
    ) : problem.kind === 'not-allowed' ? (
      <SettingsLoadState kind="empty" title="Only the owner and Admins can see this" testId="email-notifications-not-allowed">
        This server’s owner and Admins manage the email it sends. Ask one of them if an email didn’t arrive.
      </SettingsLoadState>
    ) : problem.kind === 'old-server' ? (
      <SettingsLoadState kind="empty" title="This server can’t show email notifications yet" testId="email-notifications-old-server">
        Update {PRODUCT_NAME} on this server, then come back here.
      </SettingsLoadState>
    ) : problem.kind === 'unreadable' ? (
      <SettingsLoadState kind="error" title="Email notifications couldn’t load" action={tryAgain} testId="email-notifications-error">
        This server sent an answer {PRODUCT_NAME} couldn’t read, so there’s nothing to show yet. Try again; if it keeps happening, update {PRODUCT_NAME}
        on that server.
      </SettingsLoadState>
    ) : (
      <SettingsLoadState
        kind="error"
        title="Email notifications couldn’t load"
        detail={problem.detail}
        action={tryAgain}
        testId="email-notifications-error"
      >
        {PRODUCT_NAME} couldn’t read this server’s email settings, so there’s nothing to show yet. Nothing is lost; try again in a moment.
      </SettingsLoadState>
    )
    return (
      <div data-settings-id="email-notifications" className={`w-full ${CONTENT_WIDTH} space-y-6 pb-6`}>
        {header}
        {gate}
      </div>
    )
  }

  const others = status.senders.filter((s) => s.id !== status.systemSender?.id)
  const senderAddress = (id: string | undefined): string | null => status.senders.find((s) => s.id === id)?.address ?? null
  const partState = (p: Part<unknown>, what: string, loading: string): React.ReactNode =>
    p.kind === 'loading' ? (
      <SettingsLoadState kind="loading" title={loading} />
    ) : p.kind === 'error' ? (
      <SettingsLoadState kind="error" title={`${what} couldn’t load`} detail={p.detail} action={tryAgain}>
        The rest of the page is up to date. Try again in a moment.
      </SettingsLoadState>
    ) : undefined

  const agentList = agents.kind === 'ok' ? agents.data : []
  const supList = sups.kind === 'ok' ? sups.data : []
  const logList = log.kind === 'ok' ? log.data : []

  return (
    <div data-settings-id="email-notifications" data-testid="email-notifications" className={`w-full ${CONTENT_WIDTH} space-y-6 pb-6`}>
      {header}

      {status.airgap && (
        <Callout tone="warn" role="status" className="type-row" data-testid="notify-airgap">
          Air-gap is on, so no email leaves this server until it’s turned off.
        </Callout>
      )}

      <SettingsGroup card title="Sending" hint="How this server sends email. These change from a terminal on the server." testId="notify-sending">
        <SettingRow
          label="Email from this server"
          hint={status.boxSwitch === 'on' ? 'The senders below can send.' : 'Nothing is sent from any sender until it’s turned on.'}
        >
          <Tag data-case="as-written" tone={status.boxSwitch === 'on' ? 'ok' : 'muted'}>{status.boxSwitch === 'on' ? 'On' : 'Off'}</Tag>
        </SettingRow>
        <SettingRow
          label="Mail goes out through"
          hint="This server’s own mail, or a mail relay: another mail service, such as your email provider, that sends for a domain this server doesn’t host."
        >
          <span className="type-row text-[var(--color-text-secondary)] text-right">
            {[status.hostedMail ? 'This server’s own mail' : '', status.relays.length ? plural(status.relays.length, 'mail relay', 'mail relays') : '']
              .filter(Boolean)
              .join(' and ') || 'Nothing set up yet'}
          </span>
        </SettingRow>
        <SettingRow label="Sent email kept for" hint="Recent email below drops older entries after this.">
          <span className="type-row text-[var(--color-text-secondary)]">{plural(status.settings.logRetentionDays, 'day', 'days')}</span>
        </SettingRow>
      </SettingsGroup>

      <SettingsGroup
        card
        title="Senders"
        hint={`The addresses this server sends from. ${PRODUCT_NAME} pauses one by itself when too many of its emails bounce or are reported as spam. The one for ${PRODUCT_NAME}’s own email is never paused: you get a warning instead.`}
        testId="notify-senders"
      >
        {note?.group === 'senders' && <DoneNote text={note.text} />}
        {status.systemSender ? (
          <SenderRow s={status.systemSender} system onResume={() => {}} onRemove={() => {}} />
        ) : (
          <Row
            testId="notify-no-system-sender"
            label={`No address for ${PRODUCT_NAME}’s own email`}
            hint="Password resets and sign-in codes can’t be emailed until the owner sets one from a terminal on the server."
            right={<Tag data-case="as-written" tone="warn">Not set</Tag>}
          />
        )}
        {others.map((s) => (
          <SenderRow
            key={s.id}
            s={s}
            onResume={() => setAsk({ kind: 'resume', sender: s })}
            onRemove={() => setAsk({ kind: 'remove-sender', sender: s })}
          />
        ))}
        {others.length === 0 && (
          <Row
            testId="notify-no-senders"
            label="No other senders"
            hint="Apps and agent notices need a sender of their own. The owner adds one from a terminal on the server."
          />
        )}
      </SettingsGroup>

      <SettingsGroup
        card
        title="Agent email notices"
        hint="Workspaces whose agents may email the people on this server. Turning one off stops its notices at once; turning it back on needs the owner, from a terminal."
        state={partState(agents, 'Agent email notices', 'Loading agent email notices…') ??
          (agentList.length === 0 && note?.group !== 'agents' ? (
            <SettingsLoadState kind="empty" title="No workspace can email people" testId="notify-agents-empty">
              The owner turns notices on for a workspace from a terminal on the server.
            </SettingsLoadState>
          ) : undefined)}
        settingId="email-notifications.agents"
        testId="notify-agents"
      >
        {note?.group === 'agents' && <DoneNote text={note.text} />}
        {agentList.map((a) => {
          const name = workspaceName(a.workspace)
          return (
            <Row
              key={a.workspace}
              testId={`notify-agent-${a.workspace}`}
              label={name}
              hint={
                a.on
                  ? `Sends from ${a.sender ?? 'a sender that was removed'} · On since ${when(a.grantedAt)}`
                  : `${offReasonText(a.offReason)} · Was sending from ${a.sender ?? 'a sender that was removed'}`
              }
              right={
                <>
                  <Tag data-case="as-written" tone={a.on ? 'ok' : 'muted'}>{a.on ? 'On' : 'Off'}</Tag>
                  {a.on && (
                    <Button onClick={() => setAsk({ kind: 'agent-off', agent: a, name })} data-testid={`notify-agent-off-${a.workspace}`}>
                      Turn off…
                    </Button>
                  )}
                </>
              }
            />
          )
        })}
      </SettingsGroup>

      <SettingsGroup
        card
        title="Stopped addresses"
        hint={`${PRODUCT_NAME} stops sending to an address after it bounces (for every sender), or after a spam report or an unsubscribe (for that sender or App). Addresses are kept scrambled, so the list shows only their domain.`}
        settingId="email-notifications.suppressions"
        testId="notify-stopped"
      >
        {note?.group === 'stopped' && <DoneNote text={note.text} />}
        <SettingRow stack label="Find an address" hint="Type the full address to see why mail to it stopped, or to let mail go to it again.">
          <form
            role="search"
            aria-label="Find a stopped address"
            className="flex items-center gap-2 w-full"
            onSubmit={(e) => {
              e.preventDefault()
              const q = search.trim()
              setSearched(q)
              setNote(null)
              void loadSups(q)
            }}
          >
            <SearchField
              aria-label="Address"
              className="flex-1 min-w-0"
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              onClear={() => {
                if (searched) {
                  setSearched('')
                  void loadSups('')
                }
              }}
              placeholder="someone@example.com"
            />
            <Button type="submit" data-testid="notify-find">
              Find
            </Button>
          </form>
        </SettingRow>
        {sups.kind === 'loading' ? (
          <div className="px-4 py-3 type-hint text-[var(--color-text-secondary)]" role="status">
            Looking…
          </div>
        ) : sups.kind === 'error' ? (
          <Row
            testId="notify-stopped-error"
            label={<span className="text-[var(--color-status-error-soft)]">The stop list couldn’t load</span>}
            hint={<>Try again in a moment.<span className="block text-[var(--color-text-muted)] mt-1">The server said: {sups.detail.replace(/[.\s]+$/, '')}.</span></>}
            right={<Button onClick={() => void loadSups(searched)}>Try again</Button>}
          />
        ) : supList.length === 0 ? (
          <Row
            testId="notify-stopped-empty"
            label={searched ? `${searched} isn’t on the stop list` : 'No stopped addresses'}
            hint={searched ? `Mail to it isn’t stopped by ${PRODUCT_NAME}.` : 'Every address can get mail from this server’s senders.'}
            right={
              searched ? (
                <Button
                  onClick={() => {
                    setSearch('')
                    setSearched('')
                    void loadSups('')
                  }}
                >
                  Show the whole list
                </Button>
              ) : undefined
            }
          />
        ) : (
          <>
            {supList.map((x) => (
              <Row
                key={`${x.addressSha256}-${x.scope}-${x.scopeRef ?? ''}`}
                testId={`notify-stopped-${x.addressSha256.slice(0, 8)}-${x.scope}`}
                label={searched || `An address at ${x.domain}`}
                hint={`${suppressionReasonText(x.reason)} · ${suppressionScopeText(x, status.senders)}`}
                right={<span className="type-hint text-[var(--color-text-secondary)] tabular-nums whitespace-nowrap">{when(x.createdAt)}</span>}
              />
            ))}
            {searched && (
              <Row
                testId="notify-unsuppress-row"
                label={`Let mail go to ${searched} again`}
                hint={`Takes it off ${supList.length === 1 ? 'the list' : `all ${supList.length} lists`}. If it bounces again, ${PRODUCT_NAME} stops again.`}
                right={
                  <Button variant="danger-quiet" onClick={() => setAsk({ kind: 'unsuppress', address: searched, count: supList.length })} data-testid="notify-unsuppress">
                    Remove from list…
                  </Button>
                }
              />
            )}
          </>
        )}
      </SettingsGroup>

      <SettingsGroup
        card
        title="Recent email"
        hint={`The newest ${LOG_LIMIT} emails from this server’s senders, newest first.`}
        action={
          <Button onClick={() => void load()} disabled={busy} data-testid="notify-refresh">
            {busy ? 'Refreshing…' : 'Refresh'}
          </Button>
        }
        state={partState(log, 'Recent email', 'Loading recent email…') ??
          (logList.length === 0 ? (
            <SettingsLoadState kind="empty" title="Nothing sent yet" testId="notify-log-empty">
              Each email a sender sends shows here, with whether it went out.
            </SettingsLoadState>
          ) : undefined)}
        testId="notify-log"
      >
        {logList.map((r) => {
          const st = logStatusText(r.status)
          const count = typeof r.recipients === 'number' ? r.recipients : (r.to?.length ?? 0)
          const from = senderAddress(r.sender)
          const why = r.error && (r.status === 'failed' || r.status === 'refused') ? ` · ${logErrorText(r.error).replace(/^./, (c) => c.toUpperCase())}` : ''
          return (
            <Row
              key={r.id}
              testId={`notify-log-${r.id}`}
              label={logWhatText(r)}
              hint={`To ${plural(count, 'person', 'people')}${from ? ` · From ${from}` : ''}${why}`}
              right={
                <>
                  <Tag data-case="as-written" tone={st.tone}>{st.text}</Tag>
                  <span className="type-hint text-[var(--color-text-secondary)] tabular-nums whitespace-nowrap w-[7.5rem] text-right">{when(r.createdAt)}</span>
                </>
              }
            />
          )
        })}
      </SettingsGroup>

      {ask?.kind === 'resume' && (
        <ResumeDialog sender={ask.sender} api={api} onDone={done('senders')} onClose={() => setAsk(null)} />
      )}
      {ask?.kind === 'remove-sender' && (
        <ConfirmAction
          testId="notify-remove-dialog"
          title={`Remove ${ask.sender.address}?`}
          confirmLabel="Remove sender"
          busyLabel="Removing…"
          destructive
          failed={`Couldn’t remove ${ask.sender.address}`}
          action="remove"
          workspace={workspaceName}
          fixLabel={(f) => `Turn off ${workspaceName(f.workspace)}’s notices…`}
          onFix={(f) => {
            const agent = agentList.find((a) => a.workspace === f.workspace || workspaceName(a.workspace) === f.workspace) ?? {
              workspace: f.workspace,
              sender: ask.sender.address,
              on: true,
              offReason: null,
              grantedAt: 0,
            }
            setAsk({ kind: 'agent-off', agent, name: workspaceName(agent.workspace) })
          }}
          run={async () => {
            const r = await api.removeSender(ask.sender)
            const days = typeof r?.reservedDays === 'number' ? r.reservedDays : null
            return `${ask.sender.address} is removed.${days ? ` Its address is held for ${plural(days, 'day', 'days')} before it can be added again.` : ''}`
          }}
          onDone={done('senders')}
          onClose={() => setAsk(null)}
        >
          <p>
            It stops sending at once.
            {ask.sender.app ? ` ${ownerText(ask.sender)} can’t send email until it gets a new sender.` : ''} Adding it back is done from
            a terminal on the server.
          </p>
          {(() => {
            const users = agentList.filter((a) => a.on && a.sender === ask.sender.address).map((a) => workspaceName(a.workspace))
            return users.length > 0 ? (
              <p data-testid="notify-remove-users">
                {users.length === 1 ? `Workspace ${users[0]} sends` : `Workspaces ${users.join(', ')} send`} its email notices from it; they
                stop until the owner gives {users.length === 1 ? 'it' : 'them'} another sender.
              </p>
            ) : null
          })()}
        </ConfirmAction>
      )}
      {ask?.kind === 'agent-off' && (
        <ConfirmAction
          testId="notify-agent-off-dialog"
          title={`Turn off email notices for ${ask.name}?`}
          confirmLabel="Turn off"
          busyLabel="Turning off…"
          destructive
          failed={`Couldn’t turn off notices for ${ask.name}`}
          action="agent-off"
          run={async () => {
            await api.agentOff(ask.agent.workspace)
            return `Email notices for ${ask.name} are off. Its agents can’t email people now.`
          }}
          onDone={done('agents')}
          onClose={() => setAsk(null)}
        >
          <p>Its agents stop emailing people at once. Turning notices back on needs the owner, from a terminal on the server.</p>
        </ConfirmAction>
      )}
      {ask?.kind === 'unsuppress' && (
        <ConfirmAction
          testId="notify-unsuppress-dialog"
          title={`Let mail go to ${ask.address} again?`}
          confirmLabel="Remove from list"
          busyLabel="Removing…"
          destructive
          failed={`Couldn’t take ${ask.address} off the list`}
          action="unsuppress"
          run={async () => {
            const r = await api.removeSuppression(ask.address)
            setSearch('')
            setSearched('')
            return `${ask.address} is off the stop list (${plural(r?.removed ?? ask.count, 'entry', 'entries')}), so mail can go to it again.`
          }}
          onDone={done('stopped')}
          onClose={() => setAsk(null)}
        >
          <p>
            {PRODUCT_NAME} stopped mail to it because it bounced, was reported as spam or unsubscribed. Only do this if the person asked for
            mail again or their address works now. If it bounces again, {PRODUCT_NAME} stops again.
          </p>
        </ConfirmAction>
      )}
    </div>
  )
}
