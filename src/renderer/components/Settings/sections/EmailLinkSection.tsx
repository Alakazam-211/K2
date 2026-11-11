// Settings → Email Link (K2 Mail, prd-email-server-v1 §17.5 + GH #28) —
// the CROSS-PLATFORM surface: the user connects their OWN external email
// accounts (Gmail and Microsoft by sign-in, any other provider over IMAP
// with an app password) so a workspace's agents can read the inbox, save
// reply drafts and, at the Send level, send as the account.
//
// One page of card groups, like Workspaces / Agents: the connected accounts
// (each opens its own page: account, calendar, workspace access,
// disconnect), then "Connect an account", then the advanced sign-in apps.
//
// Linked inboxes are managed through the SAME unified access layer as
// hosted inboxes (GH #28): each has a **Primary** workspace plus grants,
// via the shared <InboxAccessPanel>.
//
// NOT LINUX-GATED (BINDING): unlike Email Hosting, this is purely an IMAP
// client — nothing is installed or hosted — so the page NEVER reads GET
// /cli/mail/status and NEVER keys off `supported`. It works identically on
// Mac and Linux. Primary-gating on access/* is enforced server-side; the
// client only disables mutating controls in viewer mode.
//
// Every reply it reads goes through email-api's shape guards: a reply it
// can't read is a failed read with Try again, never a crash.
//
// CREDENTIALS HYGIENE (§17.5): the app password is WRITE-ONLY — sent ONCE
// in the add body, vaulted server-side, and never returned, listed, or
// persisted client-side. This component holds it in local state only while
// the form is open and clears it on submit/close.

import { PrimaryRoom } from '@/components/Room/PrimaryRoom'
import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { openUrl } from '@tauri-apps/plugin-opener'
import { BrowserPane } from '@/components/BrowserPane/BrowserPane'
import { useProjectsStore } from '@/stores/projects'
import { useToastStore } from '@/stores/toast'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { useWindowModeStore } from '@/stores/window-mode'
import { webFeatures } from '@/web/features'
import { Button, Callout, Input, Spinner, Tag } from '@/components/ui'
import type { SettingEntry } from '../searchManifest'
import {
  SaveError,
  SaveNote,
  SETTING_ROW_HINT,
  SETTING_SERVER_REASON,
  SETTING_ROW_LABEL,
  SettingDropdown,
  SettingRow,
  SettingsGroup,
  SettingsLoadState,
  type SaveState,
  SETTING_LIST_ROW_HOVER,
} from '../controls/SettingControls'
import { InboxAccessPanel } from './InboxAccessPanel'
import {
  addLinkedInbox,
  clearOauthClient,
  fetchInboxes,
  fetchOauthConfig,
  linkOauthComplete,
  linkOauthStart,
  linkOauthStatus,
  linkOauthUpgrade,
  MailReplyError,
  mailErrorInfo,
  mailErrorMessage,
  removeLinkedInbox,
  setOauthClient,
  type Inbox,
  type OauthClientProvider,
  type OauthConfig,
  type OauthProviderConfig,
} from './email-api'

export const EMAIL_LINK_MANIFEST: SettingEntry[] = [
  { id: 'email-link.inboxes', section: 'email-link', label: 'Connected email accounts', description: 'Your email accounts that workspaces’ agents can use', keywords: ['imap', 'gmail', 'fastmail', 'inbox', 'external', 'connect', 'email', 'account', 'app password', 'link'] },
  { id: 'email-link.add', section: 'email-link', label: 'Connect an email account', description: 'Add a Gmail, Microsoft or other email account for a workspace', keywords: ['add', 'connect', 'imap', 'gmail', 'app password', 'drafts', 'external inbox', 'imap host', 'port', 'tls', 'primary', 'workspace'] },
  { id: 'email-link.oauth', section: 'email-link', label: 'Sign in with Google or Microsoft', description: 'Connect Gmail, Outlook or Microsoft 365 without an app password', keywords: ['oauth', 'google', 'gmail', 'microsoft', 'outlook', '365', 'office', 'device code', 'sign in', 'connect', 'link', 'consent'] },
  { id: 'email-link.access', section: 'email-link', label: 'Email account access', description: 'Choose which workspaces can read, draft or send from an account', keywords: ['access', 'grant', 'revoke', 'share', 'read', 'draft', 'send', 'workspace', 'permission', 'primary', 'members', 'transfer'] },
  { id: 'email-link.oauth-apps', section: 'email-link', label: 'Sign-in apps', description: 'Use your own Google or Microsoft app to sign in to email accounts', keywords: ['oauth', 'client id', 'client secret', 'google', 'gmail', 'microsoft', 'app', 'bring your own', 'byo', 'quota', 'consent screen', 'verification', 'advanced', 'override'] },
]

const TLS_IMPLICIT = 'implicit-tls'
const TLS_STARTTLS = 'starttls'

const TLS_OPTIONS = [
  { value: TLS_IMPLICIT, label: 'SSL/TLS' },
  { value: TLS_STARTTLS, label: 'STARTTLS' },
]
/** The usual port for each security mode. */
const TLS_PORT: Record<string, string> = { [TLS_IMPLICIT]: '993', [TLS_STARTTLS]: '143' }

const VIEWER_NOTE = 'Not available while you view this server'

// ── Small shared bits ────────────────────────────────────────────────────

/** Copy with a short "Copied" on the button itself. */
function CopyButton({ text, label = 'Copy' }: { text: string; label?: string }): React.JSX.Element {
  const [copied, setCopied] = useState(false)
  return (
    <Button
      variant="quiet"
      className="flex-shrink-0"
      onClick={() => {
        void navigator.clipboard
          .writeText(text)
          .then(() => {
            setCopied(true)
            window.setTimeout(() => setCopied(false), 1200)
          })
          .catch(() => {
            useToastStore.getState().addToast('K2 couldn’t copy to the clipboard', 'error')
          })
      }}
    >
      {copied ? 'Copied' : label}
    </Button>
  )
}

/** How an account was connected, in the words a person uses. */
function providerName(inbox: Inbox): string {
  switch (inbox.capabilities?.source) {
    case 'google':
      return 'Gmail'
    case 'microsoft':
      return 'Microsoft'
    case 'imap':
      return 'App password'
    default:
      return inbox.host ? 'App password' : 'Email account'
  }
}

/** The account's health as a tag. */
function StatusTag({ status }: { status: string }): React.JSX.Element {
  if (status === 'connected' || status === 'running') return <Tag tone="ok">Connected</Tag>
  if (status === 'error') return <Tag tone="danger">Can’t connect</Tag>
  return <Tag tone="neutral">Not checked yet</Tag>
}

/** Who uses an account: the Primary, then how many more. */
function usedBy(inbox: Inbox): string {
  const primary = inbox.primary.workspace ?? 'one workspace'
  const more = inbox.grants.length
  if (more === 0) return `Used by ${primary}`
  if (more === 1) return `Used by ${primary} and ${inbox.grants[0].workspace ?? '1 more workspace'}`
  return `Used by ${primary} and ${more} more workspaces`
}

/** The plain line and the server's reason for a failed read. A reply K2
 *  couldn't read gets its own line; the raw parser text never shows. */
function readFailure(err: unknown): { line: string; detail: string | null } {
  if (err instanceof MailReplyError) {
    return {
      line: 'This server sent an answer K2 couldn’t read. Try again; if it keeps happening, update K2 on the server.',
      detail: null,
    }
  }
  return { line: 'K2 couldn’t reach this server’s email settings. Try again in a moment.', detail: mailErrorMessage(err) }
}

/** Why a step failed, in the server's words, or nothing for a reply K2
 *  couldn't read (its own line says so). */
function serverReason(err: unknown): string | null {
  return err instanceof MailReplyError ? null : mailErrorMessage(err)
}

// ── Sign-in flows (Gmail / Microsoft — no app password, O4 flows) ─────────
//
// Gmail: client-side loopback + **embedded BrowserPane on this Settings
// page** (same native child-webview as tab browsers). The rest of the page
// is hidden while the session is live. Works for local and remote daemons
// (PKCE verifier stays on the daemon; code is relayed via complete).
// Microsoft: device flow card (enabled once a Microsoft app is configured,
// MCS2). Tokens never render.

const OAUTH_POLL_MS = 3000
const OAUTH_MAX_MS = 10 * 60 * 1000 // hard bound if the provider gives none

type OauthProvider = 'gmail' | 'microsoft'

type OauthFlow =
  | { kind: 'device'; provider: 'microsoft'; linkId: string; userCode: string; verificationUrl: string; deadlineMs: number }
  | {
      kind: 'loopback'
      provider: 'gmail'
      linkId: string
      hint?: string
      deadlineMs: number
      clientCapture: true
      authorizationUrl: string
      /** Stable native webview registry key for BrowserPane. */
      browserItemId: string
    }

type ConnectFailure = {
  /** What failed, in plain words. */
  title: string
  /** What it means and the next step. */
  line: string
  /** The server's own reason, shown quietly. */
  detail: string | null
  adminConsentUrl?: string
}

/** Binds the desktop's loopback listener for a Google sign-in. Throws a
 *  plain error when this computer can't (the web build, an old app). */
async function bindLoopback(): Promise<{ captureId: string; redirectUri: string }> {
  const bind = await invoke<{ captureId?: unknown; redirectUri?: unknown } | null>('oauth_loopback_bind')
  if (!bind || typeof bind.captureId !== 'string' || typeof bind.redirectUri !== 'string') {
    throw new Error('K2 couldn’t open a Google sign-in window on this computer. Use the K2 desktop app, or update it.')
  }
  return { captureId: bind.captureId, redirectUri: bind.redirectUri }
}

/** Words for a sign-in that ended without connecting. */
function signInFailure(
  provider: OauthProvider,
  st: { state: string; code?: string; hint?: string; adminConsentUrl?: string },
): ConnectFailure {
  const who = provider === 'gmail' ? 'Google' : 'Microsoft'
  const title = `Couldn’t connect ${provider === 'gmail' ? 'Gmail' : 'Microsoft'}`
  let line: string
  if (st.state === 'denied') line = `Access wasn’t allowed on ${who}’s page. Try again and choose Allow.`
  else if (st.state === 'expired') line = 'The sign-in took too long and expired. Try again.'
  else if (st.code === 'account_mismatch') line = 'A different account signed in. Try again and sign in to the address you entered.'
  else if (st.code === 'admin_consent_required') line = 'Your Microsoft 365 admin needs to approve K2 first. Send them the approval page, then try again.'
  else line = 'The sign-in didn’t finish. Try again.'
  return { title, line, detail: st.hint ?? null, adminConsentUrl: st.adminConsentUrl }
}

/** What the "Connect Microsoft" button can do, from the daemon's OAuth app
 *  config (MCS2). `cfg` is `null` while loading and `'error'` when the read
 *  failed. Enabled only when a real Microsoft client id is in effect —
 *  either baked into this build or the owner's own (Sign-in apps, below). */
export function microsoftLinkState(cfg: OauthConfig | null | 'error'): {
  enabled: boolean
  badge: string | null
  title: string
} {
  if (cfg === null) {
    return { enabled: false, badge: null, title: 'Checking Microsoft sign-in on this server…' }
  }
  if (cfg === 'error') {
    return {
      enabled: false,
      badge: 'Unavailable',
      title: "Couldn't check Microsoft sign-in on this server. Try again under Sign-in apps.",
    }
  }
  const ms = cfg.providers.microsoft
  if (ms.configured === undefined) {
    // A daemon from before Graph send: Microsoft linking isn't offered here.
    return {
      enabled: false,
      badge: 'Update needed',
      title: 'This server’s K2 is too old to connect Microsoft accounts. Update it to add one.',
    }
  }
  if (!ms.configured) {
    return {
      enabled: false,
      badge: 'Needs an app',
      title: 'This copy of K2 has no Microsoft sign-in app. Add your own under Sign-in apps below.',
    }
  }
  return {
    enabled: true,
    badge: null,
    title: 'Sign in on Microsoft’s page with a short code.',
  }
}

/** The "Connect Microsoft" button, enabled only when a Microsoft OAuth app
 *  is configured (see {@link microsoftLinkState}); the tag beside it says
 *  why it is off. */
export function MicrosoftConnectButton({
  cfg,
  canStart,
  busy,
  onStart,
}: {
  cfg: OauthConfig | null | 'error'
  canStart: boolean
  busy: boolean
  onStart: () => void
}): React.JSX.Element {
  const st = microsoftLinkState(cfg)
  return (
    <>
      {st.badge && <Tag tone="muted">{st.badge}</Tag>}
      <Button
        disabled={!st.enabled || !canStart}
        onClick={onStart}
        title={st.title}
        data-testid="connect-microsoft"
        className="flex-shrink-0"
      >
        {busy ? 'Starting…' : 'Connect Microsoft'}
      </Button>
    </>
  )
}

/** "Connect an account": the workspace and address once, then Gmail,
 *  Microsoft, or any other provider with an app password. */
function ConnectGroup({
  canMutate,
  oauthCfg,
  onConnected,
  onSessionActiveChange,
}: {
  canMutate: boolean
  oauthCfg: OauthConfig | null | 'error'
  /** Refresh the list and say the account is connected. */
  onConnected: (address: string, workspace: string) => void
  /** Root hides the rest of the page while Google's page is up. */
  onSessionActiveChange: (active: boolean) => void
}): React.JSX.Element {
  const projects = useProjectsStore((s) => s.projects)

  const [project, setProject] = useState('')
  const [address, setAddress] = useState('')
  const [busy, setBusy] = useState<OauthProvider | 'imap' | null>(null)
  const [flow, setFlow] = useState<OauthFlow | null>(null)
  const [failure, setFailure] = useState<ConnectFailure | null>(null)
  const [imapOpen, setImapOpen] = useState(false)
  // Cancel in-flight client-capture wait when the user resets or unmounts.
  const captureCancelRef = useRef<string | null>(null)
  // Latest embedded consent webview id (for unmount reap without depending
  // on `flow` in a cleanup that would re-fire on every flow transition).
  const oauthBrowserItemIdRef = useRef<string | null>(null)

  const workspaceName = projects.find((p) => p.id === project)?.name ?? 'the workspace'
  const fieldsReady = project.trim().length > 0 && address.trim().length > 0
  const canStart = canMutate && busy === null && flow === null && fieldsReady
  const gmailSession = flow?.kind === 'loopback' && flow.clientCapture && flow.authorizationUrl ? flow : null

  useEffect(() => {
    onSessionActiveChange(gmailSession !== null)
  }, [gmailSession, onSessionActiveChange])
  useEffect(() => () => onSessionActiveChange(false), [onSessionActiveChange])

  /** Reap the embedded Gmail consent webview before dropping flow state.
   *  Relying on BrowserPane unmount alone races Cancel → Start again and
   *  leaves Tauri with `browser-{parent}-oauth-gmail-<linkId>` still registered. */
  const closeOauthBrowser = useCallback((itemId: string | null | undefined): void => {
    if (!itemId || !webFeatures.browserPane) return
    let parentWindow = 'main'
    try {
      parentWindow = getCurrentWindow().label || 'main'
    } catch {
      // web shim / non-tauri
    }
    void invoke('browser_close', { itemId, parentWindow }).catch(() => undefined)
  }, [])

  const dropFlow = useCallback((): void => {
    closeOauthBrowser(oauthBrowserItemIdRef.current)
    oauthBrowserItemIdRef.current = null
    setFlow(null)
  }, [closeOauthBrowser])

  const start = useCallback(
    async (provider: OauthProvider): Promise<void> => {
      if (!canStart) return
      setBusy(provider)
      // New start: drop any previous consent webview first.
      dropFlow()
      setFailure(null)
      try {
        if (provider === 'gmail') {
          // Always client-capture from the Email Link UI so consent runs
          // inside K2 (embedded browser), not a system window the user
          // may never see when they only have K2 focused / remote desktop.
          const bind = await bindLoopback()
          captureCancelRef.current = bind.captureId
          try {
            const res = await linkOauthStart({
              address: address.trim(),
              provider: 'gmail',
              workspace: project.trim(),
              clientCapture: true,
              redirectUri: bind.redirectUri,
            })
            if (res.flow !== 'loopback' || !res.authorizationUrl || !res.state) {
              throw new MailReplyError('the sign-in step', 'no Google sign-in page')
            }
            // Narrow for the async complete path (TS does not keep the guard).
            const oauthState = res.state
            const authorizationUrl = res.authorizationUrl
            const linkId = res.linkId
            const browserItemId = `oauth-gmail-${linkId}`
            oauthBrowserItemIdRef.current = browserItemId
            setFlow({
              kind: 'loopback',
              provider: 'gmail',
              linkId,
              hint: res.hint,
              deadlineMs: Date.now() + OAUTH_MAX_MS,
              clientCapture: true,
              authorizationUrl,
              browserItemId,
            })
            setBusy(null)
            // Wait + complete in the background; status poll drives the UI.
            void (async () => {
              try {
                const capture = await invoke<{
                  code?: string | null
                  state?: string | null
                  error?: string | null
                } | null>('oauth_loopback_wait', {
                  captureId: bind.captureId,
                  expectedState: oauthState,
                  timeoutSecs: 300,
                })
                if (captureCancelRef.current !== bind.captureId) return
                await linkOauthComplete({
                  linkId,
                  code: capture?.code ?? undefined,
                  state: capture?.state ?? oauthState,
                  error: capture?.error ?? undefined,
                })
              } catch (e) {
                if (captureCancelRef.current !== bind.captureId) return
                setFailure({ title: 'Couldn’t connect Gmail', line: 'The sign-in didn’t finish. Try again.', detail: serverReason(e) })
                dropFlow()
              } finally {
                if (captureCancelRef.current === bind.captureId) {
                  captureCancelRef.current = null
                }
              }
            })()
            return
          } catch (e) {
            await invoke('oauth_loopback_cancel', { captureId: bind.captureId }).catch(() => undefined)
            captureCancelRef.current = null
            dropFlow()
            throw e
          }
        }
        const res = await linkOauthStart({
          address: address.trim(),
          provider,
          workspace: project.trim(),
        })
        if (res.flow === 'device') {
          const bound = Number.isFinite(res.expiresIn) && res.expiresIn > 0 ? res.expiresIn * 1000 : OAUTH_MAX_MS
          setFlow({
            kind: 'device',
            provider: 'microsoft',
            linkId: res.linkId,
            userCode: res.userCode,
            verificationUrl: res.verificationUrl,
            deadlineMs: Date.now() + Math.min(bound, OAUTH_MAX_MS),
          })
        } else {
          setFailure({
            title: 'Couldn’t connect Microsoft',
            line: 'This server started a sign-in K2 can’t show. Update K2 on the server and try again.',
            detail: res.hint ?? null,
          })
        }
      } catch (e) {
        const info = mailErrorInfo(e)
        const name = provider === 'gmail' ? 'Gmail' : 'Microsoft'
        if (info.code === 'remote_unsupported') {
          setFailure({
            title: `Couldn’t connect ${name} from here`,
            line: 'Connect this account from the K2 desktop app: it shows the sign-in page inside K2.',
            detail: info.hint ?? null,
          })
        } else if (e instanceof MailReplyError) {
          setFailure({ title: `Couldn’t connect ${name}`, line: readFailure(e).line, detail: null })
        } else {
          setFailure({
            title: `Couldn’t connect ${name}`,
            line: 'K2 couldn’t start the sign-in. Check the address and try again.',
            detail: mailErrorMessage(e),
          })
        }
      } finally {
        setBusy(null)
      }
    },
    [canStart, address, project, dropFlow],
  )

  const onConnectedRef = useRef(onConnected)
  onConnectedRef.current = onConnected
  useEffect(() => {
    if (!flow) return
    let cancelled = false
    const tick = async (): Promise<void> => {
      if (cancelled) return
      if (Date.now() > flow.deadlineMs) {
        setFailure(signInFailure(flow.provider, { state: 'expired' }))
        dropFlow()
        return
      }
      try {
        const s = await linkOauthStatus(flow.linkId)
        if (cancelled) return
        if (s.state === 'connected') {
          dropFlow()
          setAddress('')
          setProject('')
          onConnectedRef.current(s.address ?? address.trim(), workspaceName)
        } else if (s.state === 'denied' || s.state === 'expired' || s.state === 'error') {
          setFailure(signInFailure(flow.provider, s))
          dropFlow()
        }
      } catch (e) {
        if (cancelled) return
        setFailure({
          title: `Couldn’t connect ${flow.provider === 'gmail' ? 'Gmail' : 'Microsoft'}`,
          line: e instanceof MailReplyError ? readFailure(e).line : 'K2 lost track of the sign-in. Try again.',
          detail: serverReason(e),
        })
        dropFlow()
      }
    }
    const id = window.setInterval(() => void tick(), OAUTH_POLL_MS)
    void tick()
    return () => {
      cancelled = true
      window.clearInterval(id)
    }
  }, [flow, address, workspaceName, dropFlow])

  const cancel = useCallback((): void => {
    const cap = captureCancelRef.current
    captureCancelRef.current = null
    if (cap) {
      void invoke('oauth_loopback_cancel', { captureId: cap }).catch(() => undefined)
    }
    dropFlow()
    setFailure(null)
  }, [dropFlow])

  // Unmount-only: cancel capture + reap consent webview if Settings closes
  // mid-flow. Must NOT re-run on flow transitions (would cancel a live start).
  useEffect(() => {
    return () => {
      const cap = captureCancelRef.current
      captureCancelRef.current = null
      if (cap) {
        void invoke('oauth_loopback_cancel', { captureId: cap }).catch(() => undefined)
      }
      closeOauthBrowser(oauthBrowserItemIdRef.current)
      oauthBrowserItemIdRef.current = null
    }
  }, [closeOauthBrowser])

  // ── Google's sign-in page, full height, in place of the page ──────────
  if (gmailSession) {
    return (
      <div className="flex flex-col h-full min-h-0" data-settings-id="email-link.oauth-browser">
        <div className="flex-shrink-0 px-6 pt-6 pb-4 space-y-3 border-b border-[var(--color-border)]">
          <div className="flex items-start justify-between gap-4">
            <div className="min-w-0">
              <h2 className="type-title text-[var(--color-text-primary)]">Sign in to Google</h2>
              <p className={SETTING_ROW_HINT}>
                Sign in as {address.trim() || 'your account'} and choose Allow. This page moves on by itself
                when Google is done.
              </p>
            </div>
            <div className="flex items-center gap-2 flex-shrink-0">
              <Button
                variant="quiet"
                onClick={() =>
                  void openUrl(gmailSession.authorizationUrl).catch(() => {
                    useToastStore.getState().addToast('K2 couldn’t open your browser', 'error')
                  })
                }
                title="Use this if the page below doesn't load"
              >
                Open in your browser
              </Button>
              <Button variant="quiet" onClick={cancel}>
                Cancel
              </Button>
            </div>
          </div>
          <div className="flex items-center gap-2 min-w-0 type-hint text-[var(--color-text-secondary)]">
            <Spinner />
            <span className="flex-1 min-w-0 truncate">{gmailSession.hint ?? 'Waiting for you to allow access…'}</span>
            <CopyButton text={gmailSession.authorizationUrl} label="Copy page link" />
          </div>
        </div>
        {/* absolute inset-0 so the dock has a real non-zero rect for
            browser_create (flex-1 alone can stay 0×0 for a frame in Settings). */}
        <div className="flex-1 min-h-[280px] relative bg-[var(--color-bg)]">
          <div className="absolute inset-0 flex flex-col">
            {webFeatures.browserPane ? (
              <PrimaryRoom>
                <BrowserPane
                  itemId={gmailSession.browserItemId}
                  tabId="settings-oauth-gmail"
                  paneGroupId="settings-oauth-gmail"
                  url={gmailSession.authorizationUrl}
                  standalone
                />
              </PrimaryRoom>
            ) : (
              <div className="flex flex-1 flex-col items-center justify-center gap-3 p-6 text-center">
                <p className="type-row text-[var(--color-text-secondary)] max-w-md">
                  K2 can’t show Google’s page here. Open it in your browser and sign in; this page still
                  moves on by itself.
                </p>
                <Button variant="accent" onClick={() => void openUrl(gmailSession.authorizationUrl)}>
                  Open Google sign-in
                </Button>
              </div>
            )}
          </div>
        </div>
      </div>
    )
  }

  const locked = !canMutate || busy !== null || flow !== null
  const notReady = !canMutate ? VIEWER_NOTE : !fieldsReady ? 'Choose a workspace and enter the address first' : undefined
  const msState = microsoftLinkState(oauthCfg)

  return (
    <div className="space-y-3">
      <SettingsGroup
        title="Connect an account"
        hint="Choose the workspace whose agents will use it and enter the address, then sign in with your provider. You can share it with more workspaces later."
        settingId="email-link.add"
        card
      >
        <SettingRow label="Workspace" hint="Its agents can use the account, and it decides who else can." field>
          <SettingDropdown
            value={project}
            placeholder="Choose a workspace"
            options={projects.map((p) => ({ value: p.id, label: p.name }))}
            onChange={(v) => setProject(v)}
            disabled={locked}
            ariaLabel="Workspace"
            fullWidth
          />
        </SettingRow>
        <SettingRow label="Email address" hint="The account you’ll sign in to." field>
          <Input
            type="email"
            aria-label="Email address"
            value={address}
            disabled={locked}
            onChange={(e) => setAddress(e.target.value)}
            placeholder="you@example.com"
            className="w-full type-control disabled:opacity-50 disabled:cursor-not-allowed"
            spellCheck={false}
          />
        </SettingRow>
        <div data-settings-id="email-link.oauth">
          <SettingRow label="Gmail" hint="Sign in with Google here in K2.">
            <Button
              // The IMAP form's Connect is the one filled button while it's open.
              variant={imapOpen ? 'default' : 'accent'}
              className="flex-shrink-0"
              disabled={!canStart}
              onClick={() => void start('gmail')}
              title={notReady ?? 'Open Google’s sign-in page in K2'}
            >
              {busy === 'gmail' ? 'Starting…' : 'Connect Gmail'}
            </Button>
          </SettingRow>
        </div>
        <SettingRow label="Outlook or Microsoft 365" hint={msState.title}>
          <MicrosoftConnectButton
            cfg={oauthCfg}
            canStart={canStart}
            busy={busy === 'microsoft'}
            onStart={() => void start('microsoft')}
          />
        </SettingRow>
        <SettingRow
          label="Other email"
          hint="Fastmail, iCloud and others, with an app password from your provider."
          below={
            imapOpen ? (
              <ImapForm
                canMutate={canMutate}
                project={project}
                address={address}
                fieldsReady={fieldsReady}
                busy={busy === 'imap'}
                setBusy={(b) => setBusy(b ? 'imap' : null)}
                onCancel={() => setImapOpen(false)}
                onConnected={(addr) => {
                  setImapOpen(false)
                  setAddress('')
                  setProject('')
                  onConnected(addr, workspaceName)
                }}
              />
            ) : undefined
          }
        >
          <Button
            variant="quiet"
            className="flex-shrink-0"
            aria-expanded={imapOpen}
            disabled={!canMutate || flow !== null}
            onClick={() => setImapOpen((o) => !o)}
            title={canMutate ? undefined : VIEWER_NOTE}
          >
            {imapOpen ? 'Hide' : 'Set up…'}
          </Button>
        </SettingRow>
      </SettingsGroup>

      {flow?.kind === 'device' && (
        <SettingsGroup title="Finish signing in to Microsoft" hint="Open Microsoft’s sign-in page and enter this code." card testId="device-flow">
          <SettingRow label="Your code" hint="It works once, for a few minutes.">
            <span className="type-title font-mono tracking-widest text-[var(--color-text-primary)] select-all">{flow.userCode}</span>
            <CopyButton text={flow.userCode} label="Copy code" />
          </SettingRow>
          <SettingRow label="Sign-in page" hint={<span className="font-mono">{flow.verificationUrl}</span>}>
            <Button variant="accent" className="flex-shrink-0" onClick={() => void openUrl(flow.verificationUrl)}>
              Open sign-in page
            </Button>
          </SettingRow>
          <div className="px-4 py-3 flex items-center justify-between gap-6" role="status">
            <span className="flex items-center gap-2 type-row text-[var(--color-text-secondary)]">
              <Spinner />
              Waiting for you to sign in as {address.trim() || 'the account'}…
            </span>
            <Button variant="quiet" className="flex-shrink-0" onClick={cancel}>
              Cancel
            </Button>
          </div>
        </SettingsGroup>
      )}

      {failure && (
        <SettingsLoadState
          kind="error"
          title={failure.title}
          detail={failure.detail}
          testId="connect-error"
          action={
            <>
              {failure.adminConsentUrl && (
                <Button variant="quiet" onClick={() => void openUrl(failure.adminConsentUrl as string)}>
                  Open the approval page
                </Button>
              )}
              <Button variant="quiet" onClick={() => setFailure(null)}>
                Try again
              </Button>
            </>
          }
        >
          {failure.line}
        </SettingsLoadState>
      )}
    </div>
  )
}

/** Any other provider: the mail server and an app password. Uses the
 *  workspace and address from the rows above. */
function ImapForm({
  canMutate,
  project,
  address,
  fieldsReady,
  busy,
  setBusy,
  onCancel,
  onConnected,
}: {
  canMutate: boolean
  project: string
  address: string
  fieldsReady: boolean
  busy: boolean
  setBusy: (b: boolean) => void
  onCancel: () => void
  onConnected: (address: string) => void
}): React.JSX.Element {
  const [host, setHost] = useState('')
  const [port, setPort] = useState('993')
  const [tls, setTls] = useState(TLS_IMPLICIT)
  const [username, setUsername] = useState('')
  const [displayName, setDisplayName] = useState('')
  const [draftsFolder, setDraftsFolder] = useState('')
  const [password, setPassword] = useState('')
  const [error, setError] = useState<{ line: string; detail: string | null } | null>(null)

  const canSubmit = canMutate && !busy && fieldsReady && host.trim().length > 0 && password.length > 0
  const missing = !fieldsReady
    ? 'Choose a workspace and enter the address above.'
    : host.trim() === ''
      ? 'Enter the mail server.'
      : password === ''
        ? 'Enter the app password.'
        : null

  const submit = async (): Promise<void> => {
    if (!canSubmit) return
    setBusy(true)
    setError(null)
    const portNum = Number(port)
    try {
      const res = await addLinkedInbox({
        project: project.trim(),
        address: address.trim(),
        host: host.trim(),
        port: Number.isFinite(portNum) && portNum > 0 ? portNum : undefined,
        tls,
        username: username.trim() || undefined,
        displayName: displayName.trim() || undefined,
        draftsFolder: draftsFolder.trim() || undefined,
        password,
      })
      // WRITE-ONLY: drop the secret from memory the instant it's sent.
      setPassword('')
      onConnected(res.address)
    } catch (e) {
      // The daemon's own reason (bad drafts folder → the real folder list,
      // login or server failure, unknown workspace), quietly under the line.
      setError({
        line: 'K2 couldn’t sign in to the mail server. Check the server, port and app password, then try again.',
        detail: serverReason(e),
      })
    } finally {
      setBusy(false)
    }
  }

  const disabled = !canMutate || busy
  const row = (label: string, hint: string, node: React.ReactNode): React.JSX.Element => (
    <div className="flex items-start justify-between gap-6 py-2.5">
      <div className="flex-1 min-w-0">
        <div className={SETTING_ROW_LABEL}>{label}</div>
        <div className={SETTING_ROW_HINT}>{hint}</div>
      </div>
      <div className="w-60 flex-shrink-0">{node}</div>
    </div>
  )

  return (
    <div
      className="mt-1 border-t border-[var(--color-border)] divide-y divide-[var(--color-border)]"
      data-testid="imap-form"
      onKeyDown={(e) => {
        if (e.key === 'Enter' && (e.target as HTMLElement).tagName === 'INPUT') {
          e.preventDefault()
          void submit()
        }
      }}
    >
      {row(
        'Mail server',
        'Your provider’s IMAP server, from its help pages.',
        <Input aria-label="Mail server" value={host} disabled={disabled} onChange={(e) => setHost(e.target.value)} placeholder="imap.example.com" className="w-full type-control disabled:opacity-50 disabled:cursor-not-allowed" spellCheck={false} />,
      )}
      {row(
        'Port and security',
        'Most providers use SSL/TLS on port 993; STARTTLS uses 143.',
        <div className="flex items-center gap-2">
          <Input aria-label="Port" inputMode="numeric" value={port} disabled={disabled} onChange={(e) => setPort(e.target.value)} className="w-16 type-control disabled:opacity-50 disabled:cursor-not-allowed" />
          <SettingDropdown
            value={tls}
            options={TLS_OPTIONS}
            onChange={(v) => {
              // Follow the mode's usual port unless one was typed in.
              if (port === TLS_PORT[tls]) setPort(TLS_PORT[v] ?? port)
              setTls(v)
            }}
            disabled={disabled}
            ariaLabel="Security"
            fullWidth
            className="flex-1 min-w-0"
          />
        </div>,
      )}
      {row(
        'App password',
        'Make one in your provider’s security settings; your normal password won’t work. K2 keeps it in its vault and never shows it again.',
        <Input aria-label="App password" type="password" autoComplete="new-password" value={password} disabled={disabled} onChange={(e) => setPassword(e.target.value)} placeholder="App password" className="w-full type-control disabled:opacity-50 disabled:cursor-not-allowed" />,
      )}
      {row(
        'User name',
        'Optional. Most providers use the email address.',
        <Input aria-label="User name" value={username} disabled={disabled} onChange={(e) => setUsername(e.target.value)} placeholder={address.trim() || 'Same as the address'} className="w-full type-control disabled:opacity-50 disabled:cursor-not-allowed" spellCheck={false} />,
      )}
      {row(
        'Name on drafts',
        'Optional. Shown as the sender on drafts.',
        <Input aria-label="Name on drafts" value={displayName} disabled={disabled} onChange={(e) => setDisplayName(e.target.value)} placeholder="Alex" className="w-full type-control disabled:opacity-50 disabled:cursor-not-allowed" />,
      )}
      {row(
        'Drafts folder',
        'Optional. K2 finds it; enter a name only if it picks the wrong one.',
        <Input aria-label="Drafts folder" value={draftsFolder} disabled={disabled} onChange={(e) => setDraftsFolder(e.target.value)} placeholder="Found automatically" className="w-full type-control disabled:opacity-50 disabled:cursor-not-allowed" spellCheck={false} />,
      )}
      <div className="pt-3 space-y-3">
        <div className="flex items-center justify-between gap-6">
          <span className="type-hint text-[var(--color-text-secondary)]">{missing ?? 'K2 checks the sign-in before it saves the account.'}</span>
          <div className="flex items-center gap-2 flex-shrink-0">
            <Button variant="quiet" onClick={onCancel} disabled={busy}>
              Cancel
            </Button>
            <Button variant="accent" disabled={!canSubmit} onClick={() => void submit()} data-testid="imap-connect">
              {busy ? 'Connecting…' : 'Connect account'}
            </Button>
          </div>
        </div>
        {error && (
          <div role="alert" data-testid="imap-error" className="leading-relaxed max-w-[75ch]">
            <p className="type-hint text-[var(--color-status-error-soft)]">Couldn’t connect this account. {error.line}</p>
            {error.detail && (
              <p className={SETTING_SERVER_REASON}>The server said: {error.detail.trim().replace(/[.\s]+$/, '')}.</p>
            )}
          </div>
        )}
      </div>
    </div>
  )
}

// ── Sign-in apps (advanced) — bring-your-own OAuth client override ───────
//
// Owner-only. Overrides K2's built-in default OAuth client with the owner's
// OWN registered Google/Microsoft app, per provider, so the consent flow
// runs against THEIR quota, verification, and consent screen. Gmail takes a
// client id + secret; Microsoft is a public client (PKCE) and takes a
// client id ONLY.
//
// SECRET HYGIENE (mirrors the app password): the client secret is
// WRITE-ONLY. The GET never returns it — `secretSet` only reports whether
// one is vaulted. The secret field is NEVER prefilled; when a secret is
// already set we say so and only send `clientSecret` if the owner types a
// new one. The non-secret client id IS prefilled from the config.

function OauthClientRow({
  provider,
  label,
  hasSecret,
  cfg,
  canMutate,
  onChanged,
}: {
  provider: OauthClientProvider
  label: string
  /** Gmail = true (client id + secret); Microsoft = false (public, id only). */
  hasSecret: boolean
  cfg: OauthProviderConfig
  canMutate: boolean
  /** Refetch the config after a successful save/reset to refresh status. */
  onChanged: () => void
}): React.JSX.Element {
  const isCustom = cfg.source === 'custom'
  const [open, setOpen] = useState(false)
  const [clientId, setClientId] = useState(isCustom ? cfg.clientId ?? '' : '')
  // WRITE-ONLY: never seeded from the server; only holds a freshly typed value.
  const [clientSecret, setClientSecret] = useState('')
  const [busy, setBusy] = useState<null | 'save' | 'reset'>(null)
  const [result, setResult] = useState<SaveState>('idle')
  const savedTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  useEffect(() => () => {
    if (savedTimer.current) clearTimeout(savedTimer.current)
  }, [])

  // Re-seed the (non-secret) client id when the server config changes — e.g.
  // after another save or a reset-to-default. The secret is left untouched.
  useEffect(() => {
    setClientId(cfg.source === 'custom' ? cfg.clientId ?? '' : '')
    setClientSecret('')
  }, [cfg.source, cfg.clientId, cfg.secretSet])

  const done = (): void => {
    setResult('saved')
    if (savedTimer.current) clearTimeout(savedTimer.current)
    savedTimer.current = setTimeout(() => setResult('idle'), 2000)
    setOpen(false)
    onChanged()
  }

  const canSave = canMutate && busy === null && clientId.trim().length > 0
  const save = async (): Promise<void> => {
    if (!canSave) return
    setBusy('save')
    setResult('idle')
    try {
      await setOauthClient({
        provider,
        clientId: clientId.trim(),
        // Only send the secret when the owner typed a new one; blank KEEPS the
        // vaulted secret (Gmail) and is irrelevant for Microsoft (public).
        clientSecret: hasSecret && clientSecret.length > 0 ? clientSecret : undefined,
      })
      // Drop the secret from memory the instant it's sent.
      setClientSecret('')
      done()
    } catch (e) {
      setResult({ error: mailErrorMessage(e) })
    } finally {
      setBusy(null)
    }
  }

  const reset = async (): Promise<void> => {
    if (!canMutate || busy !== null) return
    const ok = await useConfirmDialogStore.getState().confirm({
      title: `Go back to K2’s ${label} app?`,
      message: `K2 forgets your ${label} app’s ID${hasSecret ? ' and secret' : ''}. Accounts you connected keep working until they next sign in.`,
      confirmLabel: 'Use K2’s app',
    })
    if (!ok) return
    setBusy('reset')
    setResult('idle')
    try {
      await clearOauthClient(provider)
      setClientSecret('')
      done()
    } catch (e) {
      setResult({ error: mailErrorMessage(e) })
    } finally {
      setBusy(null)
    }
  }

  const idChanged = clientId.trim() !== (cfg.clientId ?? '')
  return (
    <SettingRow
      label={label}
      testId={`oauth-app-${provider}`}
      hint={
        <>
          {isCustom ? 'Signs in with your own app.' : 'Signs in with K2’s app.'}
          <SaveError state={result} />
        </>
      }
      below={
        open ? (
          <div className="mt-1 border-t border-[var(--color-border)] divide-y divide-[var(--color-border)]" data-testid={`oauth-app-form-${provider}`}>
            <div className="flex items-start justify-between gap-6 py-2.5">
              <div className="flex-1 min-w-0">
                <div className={SETTING_ROW_LABEL}>Client ID</div>
                <div className={SETTING_ROW_HINT}>From the app you registered with {label}.</div>
              </div>
              <Input
                aria-label={`${label} client ID`}
                value={clientId}
                disabled={!canMutate || busy !== null}
                autoComplete="off"
                spellCheck={false}
                onChange={(e) => setClientId(e.target.value)}
                placeholder={provider === 'gmail' ? '….apps.googleusercontent.com' : 'Application (client) ID'}
                className="w-60 flex-shrink-0 type-control font-mono disabled:opacity-50"
              />
            </div>
            {hasSecret && (
              <div className="flex items-start justify-between gap-6 py-2.5">
                <div className="flex-1 min-w-0">
                  <div className={SETTING_ROW_LABEL}>Client secret</div>
                  <div className={SETTING_ROW_HINT}>
                    {cfg.secretSet
                      ? 'A secret is saved. Leave this empty to keep it.'
                      : 'K2 keeps it in its vault and never shows it again.'}
                  </div>
                  {cfg.secretSet && clientSecret.length === 0 && idChanged && isCustom && (
                    <p className="type-hint text-[var(--color-status-error-soft)] mt-1 leading-relaxed max-w-[75ch]">
                      You changed the client ID, so enter its secret too.
                    </p>
                  )}
                </div>
                <Input
                  aria-label={`${label} client secret`}
                  type="password"
                  value={clientSecret}
                  disabled={!canMutate || busy !== null}
                  autoComplete="new-password"
                  onChange={(e) => setClientSecret(e.target.value)}
                  placeholder={cfg.secretSet ? 'Saved' : 'Client secret'}
                  className="w-60 flex-shrink-0 type-control disabled:opacity-50"
                />
              </div>
            )}
            <div className="pt-3 flex items-center justify-end gap-2">
              <Button variant="quiet" onClick={() => setOpen(false)} disabled={busy !== null}>
                Cancel
              </Button>
              <Button variant="accent" disabled={!canSave} onClick={() => void save()}>
                {busy === 'save' ? 'Saving…' : 'Save'}
              </Button>
            </div>
          </div>
        ) : undefined
      }
    >
      <SaveNote state={result} />
      {isCustom && !open && (
        <Button variant="quiet" className="flex-shrink-0" disabled={!canMutate || busy !== null} onClick={() => void reset()}>
          {busy === 'reset' ? 'Switching…' : 'Use K2’s app…'}
        </Button>
      )}
      <Button
        variant="quiet"
        className="flex-shrink-0"
        aria-expanded={open}
        disabled={!canMutate || busy !== null}
        title={canMutate ? undefined : VIEWER_NOTE}
        onClick={() => setOpen((o) => !o)}
      >
        {open ? 'Hide' : isCustom ? 'Change…' : 'Use your own…'}
      </Button>
    </SettingRow>
  )
}

function SignInAppsGroup({
  canMutate,
  cfg,
  cfgError,
  onRetry,
  onChanged,
}: {
  canMutate: boolean
  cfg: OauthConfig | null | 'error'
  cfgError: unknown
  onRetry: () => void
  onChanged: () => void
}): React.JSX.Element {
  const failure = cfg === 'error' ? readFailure(cfgError) : null
  return (
    <SettingsGroup
      title="Sign-in apps"
      badge={<Tag tone="muted">Advanced</Tag>}
      hint="K2 signs in to Google and Microsoft with its own app. Use your own app to have your own quota and sign-in screen."
      settingId="email-link.oauth-apps"
      card
      state={
        cfg === null ? (
          <SettingsLoadState kind="loading" title="Checking the sign-in apps…" />
        ) : failure ? (
          <SettingsLoadState
            kind="error"
            title="Couldn’t load the sign-in apps"
            detail={failure.detail}
            action={<Button variant="quiet" onClick={onRetry}>Try again</Button>}
          >
            {failure.line}
          </SettingsLoadState>
        ) : undefined
      }
    >
      {cfg !== null && cfg !== 'error' && (
        <>
          <OauthClientRow provider="gmail" label="Google" hasSecret cfg={cfg.providers.gmail} canMutate={canMutate} onChanged={onChanged} />
          <OauthClientRow provider="microsoft" label="Microsoft" hasSecret={false} cfg={cfg.providers.microsoft} canMutate={canMutate} onChanged={onChanged} />
        </>
      )}
    </SettingsGroup>
  )
}

// ── Add calendar (MCS3 Microsoft, MCS4 Google) ───────────────────────────
//
// A Microsoft or Google link consented for mail only can't do `k2 calendar`;
// its capabilities name `scope_not_granted` on the calendar caps. "Add
// calendar" re-consents the SAME link for calendars (`k2 mail link upgrade
// <address> --features calendar`) — grants, levels and ids stay. Microsoft
// signs in with a device code; Google with its consent page (client-capture
// loopback, incremental consent), opened in the system browser.

/** True when this inbox is a Microsoft or Google link whose calendar caps
 *  are off only because the consent didn't include calendars. */
export function needsCalendarConsent(inbox: Pick<Inbox, 'capabilities'>): boolean {
  const c = inbox.capabilities
  if (!c || (c.source !== 'microsoft' && c.source !== 'google')) return false
  return (c.missing ?? []).some((m) => m.cap.startsWith('calendar.') && m.reason === 'scope_not_granted')
}

type UpgradeFlow =
  | { kind: 'device'; linkId: string; userCode: string; verificationUrl: string; deadlineMs: number }
  | { kind: 'browser'; linkId: string; authorizationUrl: string; deadlineMs: number }

/** The Calendar card for an account signed in for mail only: one row with
 *  Add calendar, then the sign-in it starts, then what went wrong if it
 *  did. Renders nothing for an account that has nothing to add. */
export function AddCalendarAccess({
  inbox,
  canMutate,
  onUpgraded,
  pollMs = OAUTH_POLL_MS,
}: {
  inbox: Inbox
  canMutate: boolean
  /** Re-read the inbox list (its capabilities change). */
  onUpgraded: () => void
  pollMs?: number
}): React.JSX.Element | null {
  const [busy, setBusy] = useState(false)
  const [flow, setFlow] = useState<UpgradeFlow | null>(null)
  const [failure, setFailure] = useState<{ text: string; adminConsentUrl?: string } | null>(null)
  const onUpgradedRef = useRef(onUpgraded)
  onUpgradedRef.current = onUpgraded
  // Google: the loopback capture waiting for the consent redirect.
  const captureRef = useRef<string | null>(null)
  const isGoogle = inbox.capabilities?.source === 'google'

  const cancelCapture = useCallback((): void => {
    const cap = captureRef.current
    captureRef.current = null
    if (cap) void invoke('oauth_loopback_cancel', { captureId: cap }).catch(() => undefined)
  }, [])
  useEffect(() => cancelCapture, [cancelCapture])

  const startGoogle = useCallback(async (): Promise<void> => {
    const bind = await bindLoopback()
    captureRef.current = bind.captureId
    try {
      const res = await linkOauthUpgrade({
        inbox: inbox.address,
        features: 'calendar',
        clientCapture: true,
        redirectUri: bind.redirectUri,
      })
      if (res.flow !== 'loopback') {
        throw new Error('K2 could not start a Google consent page. Try again.')
      }
      if (!res.authorizationUrl || !res.state) {
        throw new Error(res.hint ?? 'K2 could not start a Google consent page. Try again.')
      }
      const { linkId, authorizationUrl } = res
      const oauthState = res.state
      setFlow({ kind: 'browser', linkId, authorizationUrl, deadlineMs: Date.now() + OAUTH_MAX_MS })
      void openUrl(authorizationUrl).catch(() => undefined)
      // Relay the redirect's code in the background; the status poll
      // drives the UI.
      void (async () => {
        try {
          const capture = await invoke<{ code?: string | null; state?: string | null; error?: string | null } | null>(
            'oauth_loopback_wait',
            { captureId: bind.captureId, expectedState: oauthState, timeoutSecs: 300 },
          )
          if (captureRef.current !== bind.captureId) return
          await linkOauthComplete({
            linkId,
            code: capture?.code ?? undefined,
            state: capture?.state ?? oauthState,
            error: capture?.error ?? undefined,
          })
        } catch (e) {
          if (captureRef.current !== bind.captureId) return
          setFailure({ text: mailErrorMessage(e) })
          setFlow(null)
        } finally {
          if (captureRef.current === bind.captureId) captureRef.current = null
        }
      })()
    } catch (e) {
      cancelCapture()
      throw e
    }
  }, [inbox.address, cancelCapture])

  const start = useCallback(async (): Promise<void> => {
    setBusy(true)
    setFailure(null)
    try {
      if (isGoogle) {
        await startGoogle()
        return
      }
      const res = await linkOauthUpgrade({ inbox: inbox.address, features: 'calendar' })
      if (res.flow === 'device') {
        const bound = Number.isFinite(res.expiresIn) && res.expiresIn > 0 ? res.expiresIn * 1000 : OAUTH_MAX_MS
        setFlow({
          kind: 'device',
          linkId: res.linkId,
          userCode: res.userCode,
          verificationUrl: res.verificationUrl,
          deadlineMs: Date.now() + Math.min(bound, OAUTH_MAX_MS),
        })
      } else {
        setFailure({ text: res.hint ?? 'K2 could not start a Microsoft device sign-in. Try again.' })
      }
    } catch (e) {
      setFailure({ text: e instanceof MailReplyError ? readFailure(e).line : mailErrorMessage(e) })
    } finally {
      setBusy(false)
    }
  }, [inbox.address, isGoogle, startGoogle])

  useEffect(() => {
    if (!flow) return
    let cancelled = false
    const tick = async (): Promise<void> => {
      if (cancelled) return
      if (Date.now() > flow.deadlineMs) {
        setFailure({ text: 'The request expired before it was approved.' })
        setFlow(null)
        return
      }
      try {
        const st = await linkOauthStatus(flow.linkId)
        if (cancelled) return
        if (st.state === 'connected') {
          setFlow(null)
          useToastStore.getState().addToast(`Calendar access added for ${inbox.address}`, 'success')
          onUpgradedRef.current()
        } else if (st.state === 'denied' || st.state === 'expired' || st.state === 'error') {
          const lead =
            st.state === 'denied'
              ? 'Access was denied.'
              : st.state === 'expired'
                ? 'The request expired before it was approved.'
                : st.code === 'account_mismatch'
                  ? 'A different account signed in.'
                  : st.code === 'admin_consent_required'
                    ? 'Your Microsoft 365 admin needs to approve calendar access first.'
                    : st.code === 'scope_not_granted'
                      ? 'Calendar access was not allowed on the consent page.'
                      : 'Something went wrong adding calendar access.'
          setFailure({ text: st.hint ? `${lead} ${st.hint}` : lead, adminConsentUrl: st.adminConsentUrl })
          setFlow(null)
          cancelCapture()
        }
      } catch (e) {
        if (cancelled) return
        setFailure({ text: e instanceof MailReplyError ? readFailure(e).line : mailErrorMessage(e) })
        setFlow(null)
      }
    }
    const id = window.setInterval(() => void tick(), pollMs)
    void tick()
    return () => {
      cancelled = true
      window.clearInterval(id)
    }
  }, [flow, inbox.address, pollMs, cancelCapture])

  if (!needsCalendarConsent(inbox)) return null
  return (
    <SettingsGroup title="Calendar" card testId="add-calendar">
      <SettingRow
        label="Calendar isn’t connected"
        hint={`This account was connected for mail only. Add its calendar so agents with access can read and add events.${isGoogle ? ' Google asks you to allow it in your browser.' : ''}`}
        below={
          failure ? (
            <div className="space-y-2" data-testid="add-calendar-error" role="alert">
              <p className="type-hint text-[var(--color-status-error-soft)] leading-relaxed max-w-[75ch]">{failure.text}</p>
              {failure.adminConsentUrl && (
                <Button variant="quiet" onClick={() => void openUrl(failure.adminConsentUrl as string)}>
                  Open the admin consent page
                </Button>
              )}
            </div>
          ) : undefined
        }
      >
        <Button
          data-testid="add-calendar-button"
          className="flex-shrink-0"
          disabled={!canMutate || busy || flow !== null}
          onClick={() => void start()}
          title={
            !canMutate
              ? VIEWER_NOTE
              : isGoogle
                ? 'Open Google’s page to allow calendar access'
                : 'Sign in again with a code to add calendar access'
          }
        >
          {busy ? 'Starting…' : 'Add calendar'}
        </Button>
      </SettingRow>
      {flow?.kind === 'browser' && (
        <div className="px-4 py-3 space-y-2" data-testid="add-calendar-browser" role="status">
          <p className={SETTING_ROW_HINT}>
            Allow calendar access for {inbox.address} on Google’s page. It opened in your browser.
          </p>
          <div className="flex items-center justify-between gap-6">
            <span className="flex items-center gap-2 type-hint text-[var(--color-text-secondary)]">
              <Spinner />
              Waiting for approval… this updates by itself.
            </span>
            <div className="flex items-center gap-2 flex-shrink-0">
              <CopyButton text={flow.authorizationUrl} label="Copy page link" />
              <Button variant="quiet" onClick={() => void openUrl(flow.authorizationUrl)}>
                Open Google’s page again
              </Button>
            </div>
          </div>
        </div>
      )}
      {flow?.kind === 'device' && (
        <div className="px-4 py-3 space-y-2" data-testid="add-calendar-device" role="status">
          <p className={SETTING_ROW_HINT}>
            Go to <span className="font-mono">{flow.verificationUrl}</span> and enter this code, signed in as{' '}
            {inbox.address}:
          </p>
          <div className="flex items-center justify-between gap-6">
            <span className="type-title font-mono tracking-widest text-[var(--color-text-primary)] select-all">{flow.userCode}</span>
            <div className="flex items-center gap-2 flex-shrink-0">
              <CopyButton text={flow.userCode} label="Copy code" />
              <Button variant="quiet" onClick={() => void openUrl(flow.verificationUrl)}>
                Open sign-in page
              </Button>
            </div>
          </div>
          <p className="flex items-center gap-2 type-hint text-[var(--color-text-secondary)]">
            <Spinner />
            Waiting for approval… this updates by itself.
          </p>
        </div>
      )}
    </SettingsGroup>
  )
}

// ── One account's page ───────────────────────────────────────────────────

function AccountDetail({
  inbox,
  canMutate,
  onBack,
  onRemoved,
  onChanged,
  patchInbox,
}: {
  inbox: Inbox
  canMutate: boolean
  onBack: () => void
  onRemoved: (address: string) => void
  /** Re-read the inbox list (capabilities changed server-side). */
  onChanged: () => void
  /** Optimistically patch this inbox's row in the parent list. */
  patchInbox: (address: string, patch: (i: Inbox) => Inbox) => void
}): React.JSX.Element {
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const source = inbox.capabilities?.source
  const oauth = source === 'google' || source === 'microsoft'
  const calendarOn = oauth && inbox.capabilities?.calendar?.read === true

  const disconnect = useCallback(async (): Promise<void> => {
    const confirmed = await useConfirmDialogStore.getState().confirm({
      title: `Disconnect ${inbox.address}?`,
      message:
        'Agents in every workspace lose access to this account, and K2 deletes its saved sign-in. ' +
        'Nothing in the mailbox changes. You can connect it again later.',
      confirmLabel: 'Disconnect',
      destructive: true,
    })
    if (!confirmed) return
    setBusy(true)
    setError(null)
    try {
      await removeLinkedInbox(inbox.address)
      onRemoved(inbox.address)
    } catch (e) {
      setError(mailErrorMessage(e))
      setBusy(false)
    }
  }, [inbox.address, onRemoved])

  return (
    <div className="space-y-6" data-testid="email-account">
      <header className="space-y-3">
        <Button variant="quiet" onClick={onBack} data-testid="email-back">
          &larr; All accounts
        </Button>
        <div className="min-w-0">
          <h2 className="type-title text-[var(--color-text-primary)] truncate">{inbox.address}</h2>
          <p className="type-hint text-[var(--color-text-secondary)] mt-1 leading-relaxed">
            {providerName(inbox)} · {usedBy(inbox)}
            {!canMutate && ' · You’re viewing this server, so you can’t change it here.'}
          </p>
        </div>
      </header>

      <SettingsGroup title="Account" card>
        <SettingRow
          label="Status"
          hint={
            inbox.status === 'error'
              ? 'K2 couldn’t sign in to this account the last time it checked, so agents can’t use it. If its password changed, disconnect it and connect it again.'
              : inbox.status === 'connected' || inbox.status === 'running'
                ? 'K2 signs in to this account normally.'
                : 'K2 hasn’t checked this account yet.'
          }
        >
          <StatusTag status={inbox.status} />
        </SettingRow>
        <SettingRow
          label="Signed in with"
          hint={oauth ? `K2 keeps the sign-in from ${source === 'google' ? 'Google' : 'Microsoft'}; no password is stored.` : 'An app password, kept in K2’s vault.'}
        >
          <span className="type-row text-[var(--color-text-secondary)]">
            {source === 'google' ? 'Google' : source === 'microsoft' ? 'Microsoft' : 'App password'}
          </span>
        </SettingRow>
        {!oauth && inbox.host && (
          <SettingRow label="Mail server" hint={inbox.tls === TLS_STARTTLS ? 'STARTTLS' : 'SSL/TLS'}>
            <span className="type-row font-mono text-[var(--color-text-secondary)]">{inbox.host}</span>
          </SettingRow>
        )}
      </SettingsGroup>

      {needsCalendarConsent(inbox) ? (
        <AddCalendarAccess inbox={inbox} canMutate={canMutate} onUpgraded={onChanged} />
      ) : calendarOn ? (
        <SettingsGroup title="Calendar" card>
          <SettingRow label="Calendar" hint="Agents with access can read this account’s calendar and add events.">
            <Tag tone="ok">Connected</Tag>
          </SettingRow>
        </SettingsGroup>
      ) : null}

      <InboxAccessPanel inbox={inbox} canMutate={canMutate} patchInbox={patchInbox} card />

      <SettingsGroup title="Disconnect" card>
        <SettingRow
          label="Disconnect this account"
          hint="Agents lose access and K2 deletes the saved sign-in. Nothing in the mailbox changes."
          below={
            error ? (
              <p role="alert" className="type-hint text-[var(--color-status-error-soft)] leading-relaxed max-w-[75ch]">
                Couldn’t disconnect: {error.replace(/[.\s]+$/, '')}. The account is still connected; try again.
              </p>
            ) : undefined
          }
        >
          <Button
            variant="danger-quiet"
            className="flex-shrink-0"
            disabled={!canMutate || busy}
            title={canMutate ? undefined : VIEWER_NOTE}
            onClick={() => void disconnect()}
          >
            {busy ? 'Disconnecting…' : 'Disconnect…'}
          </Button>
        </SettingRow>
      </SettingsGroup>
    </div>
  )
}

// ── The page ─────────────────────────────────────────────────────────────

type ListState = { kind: 'loading' } | { kind: 'error'; err: unknown } | { kind: 'ready'; rows: Inbox[] }
type Notice = { kind: 'connected'; address: string; workspace: string } | { kind: 'disconnected'; address: string }

function AccountRow({ inbox, onOpen }: { inbox: Inbox; onOpen: () => void }): React.JSX.Element {
  return (
    <div
      role="button"
      tabIndex={0}
      data-k2-focus=""
      data-testid={`account-row-${inbox.address}`}
      aria-label={`Manage ${inbox.address}`}
      onClick={onOpen}
      onKeyDown={(e) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault()
          onOpen()
        }
      }}
      className={`relative flex items-center gap-4 px-4 py-3 no-drag cursor-pointer outline-none ${SETTING_LIST_ROW_HOVER}`}
    >
      <div className="flex-1 min-w-0">
        <div className="type-name text-[var(--color-text-primary)] truncate">{inbox.address}</div>
        <div className="type-hint text-[var(--color-text-secondary)] mt-0.5 truncate">
          {providerName(inbox)} · {usedBy(inbox)}
        </div>
      </div>
      <StatusTag status={inbox.status} />
      <svg className="w-3 h-3 flex-shrink-0 text-[var(--color-text-secondary)]" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2} aria-hidden="true">
        <path strokeLinecap="round" strokeLinejoin="round" d="M9 5l7 7-7 7" />
      </svg>
    </div>
  )
}

export function EmailLinkSection(): React.JSX.Element {
  const viewerReadOnly = useWindowModeStore((s) => s.resolved && s.mode === 'viewer')
  const canMutate = !viewerReadOnly

  const [list, setList] = useState<ListState>({ kind: 'loading' })
  const [revision, setRevision] = useState(0)
  const reload = useCallback(() => setRevision((r) => r + 1), [])
  const [openAddress, setOpenAddress] = useState<string | null>(null)
  const [notice, setNotice] = useState<Notice | null>(null)
  /** Google's sign-in page fills the page while it is up. */
  const [gmailActive, setGmailActive] = useState(false)

  const [oauthCfg, setOauthCfg] = useState<OauthConfig | null | 'error'>(null)
  const [oauthErr, setOauthErr] = useState<unknown>(null)
  const [oauthRevision, setOauthRevision] = useState(0)
  const reloadOauth = useCallback(() => setOauthRevision((r) => r + 1), [])

  // ── The connected accounts: the unified catalog, filtered to LINKED
  //    accounts. Cross-platform: no status/supported gate. ─────────────
  useEffect(() => {
    let cancelled = false
    setList((l) => (l.kind === 'ready' ? l : { kind: 'loading' }))
    fetchInboxes()
      .then((r) => {
        if (!cancelled) setList({ kind: 'ready', rows: r.filter((i) => i.source === 'linked') })
      })
      .catch((err) => {
        if (!cancelled) setList({ kind: 'error', err })
      })
    return () => {
      cancelled = true
    }
  }, [revision])

  // ── Which sign-in apps this server has (the Microsoft button, Sign-in
  //    apps). One read for both. ─────────────────────────────────────────
  useEffect(() => {
    let cancelled = false
    setOauthCfg(null)
    fetchOauthConfig()
      .then((c) => {
        if (!cancelled) setOauthCfg(c)
      })
      .catch((e) => {
        if (cancelled) return
        setOauthErr(e)
        setOauthCfg('error')
      })
    return () => {
      cancelled = true
    }
  }, [oauthRevision])

  const rows = list.kind === 'ready' ? list.rows : null
  const shown = useMemo(
    () => (openAddress && rows ? rows.find((i) => i.address === openAddress) ?? null : null),
    [openAddress, rows],
  )
  // An account that went away (disconnected elsewhere) falls back to the list.
  useEffect(() => {
    if (openAddress && rows && !rows.some((i) => i.address === openAddress)) setOpenAddress(null)
  }, [openAddress, rows])

  const onConnected = useCallback(
    (address: string, workspace: string): void => {
      setNotice({ kind: 'connected', address, workspace })
      reload()
    },
    [reload],
  )

  // Optimistic patch of a single inbox row (primary/grants) — mutates the
  // already-loaded list in place, no refetch (no fetch-in-render).
  const patchInbox = useCallback((address: string, patch: (i: Inbox) => Inbox): void => {
    setList((prev) =>
      prev.kind === 'ready' ? { kind: 'ready', rows: prev.rows.map((i) => (i.address === address ? patch(i) : i)) } : prev,
    )
  }, [])

  const failure = list.kind === 'error' ? readFailure(list.err) : null

  // A confirmation lands at the top of the list, often above where the
  // person is (the Connect card); bring it into view.
  const noticeRef = useRef<HTMLDivElement>(null)
  useEffect(() => {
    if (notice) noticeRef.current?.scrollIntoView?.({ block: 'nearest', behavior: 'smooth' })
  }, [notice])

  return (
    <div
      className={
        gmailActive ? 'h-full min-h-0 flex flex-col' : 'h-full min-h-0 overflow-y-auto p-6 [scrollbar-gutter:stable]'
      }
    >
      <div className={gmailActive ? 'flex-1 min-h-0 flex flex-col' : 'w-full max-w-[52rem] space-y-6 pb-6'}>
        {!gmailActive && shown && (
          <AccountDetail
            key={shown.address}
            inbox={shown}
            canMutate={canMutate}
            patchInbox={patchInbox}
            onChanged={reload}
            onBack={() => setOpenAddress(null)}
            onRemoved={(address) => {
              setOpenAddress(null)
              setNotice({ kind: 'disconnected', address })
              setList((prev) =>
                prev.kind === 'ready' ? { kind: 'ready', rows: prev.rows.filter((i) => i.address !== address) } : prev,
              )
              reload()
            }}
          />
        )}

        {!gmailActive && !shown && (
          <>
            <header className="min-w-0">
              <h2 className="type-title text-[var(--color-text-primary)]">Email Link</h2>
              <p className="type-hint text-[var(--color-text-secondary)] mt-1 leading-relaxed max-w-[75ch]">
                Connect your own email accounts so agents can read your mail and draft replies for you. You choose
                which workspaces can use each account.
              </p>
            </header>

            <div className="space-y-3">
              {notice && (
                <div ref={noticeRef} className="scroll-mt-6">
                  <Callout tone="ok" role="status" className="type-hint flex items-center justify-between gap-4" data-testid="email-notice">
                    <span>
                      {notice.kind === 'connected'
                        ? `${notice.address} is connected. Agents in ${notice.workspace} can use it now.`
                        : `${notice.address} is disconnected. No workspace can use it, and K2 deleted its saved sign-in.`}
                    </span>
                    <span className="flex items-center gap-2 flex-shrink-0">
                      {notice.kind === 'connected' && rows?.some((i) => i.address === notice.address) && (
                        <Button
                          variant="quiet"
                          onClick={() => {
                            setOpenAddress(notice.address)
                            setNotice(null)
                          }}
                        >
                          Manage
                        </Button>
                      )}
                      <Button variant="quiet" onClick={() => setNotice(null)}>
                        Dismiss
                      </Button>
                    </span>
                  </Callout>
                </div>
              )}
              <SettingsGroup
                title="Connected accounts"
                hint="Open an account to choose which workspaces can use it, or to disconnect it."
                settingId="email-link.inboxes"
                card
                state={
                  list.kind === 'loading' ? (
                    <SettingsLoadState kind="loading" title="Loading your email accounts…" testId="accounts-loading" />
                  ) : failure ? (
                    <SettingsLoadState
                      kind="error"
                      title="Couldn’t load your email accounts"
                      detail={failure.detail}
                      testId="accounts-error"
                      action={<Button variant="quiet" onClick={reload}>Try again</Button>}
                    >
                      {failure.line}
                    </SettingsLoadState>
                  ) : rows && rows.length === 0 ? (
                    <SettingsLoadState kind="empty" title="No email accounts yet" testId="accounts-empty">
                      Connect one below to let a workspace’s agents read it and draft replies.
                    </SettingsLoadState>
                  ) : undefined
                }
              >
                {rows?.map((i) => (
                  <AccountRow
                    key={i.address}
                    inbox={i}
                    onOpen={() => {
                      setNotice(null)
                      setOpenAddress(i.address)
                    }}
                  />
                ))}
              </SettingsGroup>
            </div>
          </>
        )}

        {!shown && (
          <ConnectGroup
            canMutate={canMutate}
            oauthCfg={oauthCfg}
            onConnected={onConnected}
            onSessionActiveChange={setGmailActive}
          />
        )}

        {!gmailActive && !shown && (
          <SignInAppsGroup canMutate={canMutate} cfg={oauthCfg} cfgError={oauthErr} onRetry={reloadOauth} onChanged={reloadOauth} />
        )}
      </div>
    </div>
  )
}
