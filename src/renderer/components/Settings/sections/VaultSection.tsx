// Settings → Access → Vault (prd-k2-vault-v1 §16.1, KV39; §18 copy).
//
// A thin client on `/cli/vault/*`. The daemon decides who sees what: a
// Member's list comes back filtered, and K2's own (`system`) secrets only
// come back to people allowed to see them, as metadata.
//
// Values: the only place a value is ever on screen is the Show (reveal)
// dialog, after a confirm (the Mac's own sheet, or a password). It lives in
// that dialog's state only, masked until "Show", and goes after 30 seconds
// or when the dialog closes. Pasted values (add, rotate) are read from their
// field once on submit, sent as `valueB64` in the POST body, and the field
// is emptied. Nothing is stored, logged or kept in a store.
//
// VS1 is deferred for 0.45.2 (Rosson): this page talks to the daemon with
// the app's owner token like every other Settings page; no attested channel.
// Recovery-key setup (0.45.4): the owner token alone isn't a person (VS2),
// so the page first asks for a confirm (the Mac's sheet, or a password),
// then shows the 24 words once and has the person type back three. The
// words live in that dialog's state only, are never copied to the
// clipboard, and are gone once the check step starts.
//
// Layout: the Settings reference pattern (page header, tab strip, card
// groups of rows, one primary action per tab, dialogs from
// AccessDialogs.tsx). Words: vault-copy.ts; dialogs: VaultDialogs.tsx.

import React, { useCallback, useEffect, useMemo, useState } from 'react'
import type { SettingEntry } from '../searchManifest'
import { Button, OverflowMenu, Surface, Tag, type TagTone } from '@/components/ui'
import { daemonErrorText } from '@/lib/daemon-error'
import { siteLabel } from '@/components/SessionView/SecretCard'
import { vaultApi, type VaultPreset, type VaultRequest, type VaultSecret, type VaultStatus } from './vault-api'
import { AccessAuditLink } from '../shared/AccessAuditLink'
import { SETTING_ROW_HINT, SETTING_ROW_LABEL, SettingRow, SettingsGroup, SettingsLoadState } from '../controls/SettingControls'
import { useServerSupports } from '@/lib/server-capabilities'
import { shortDay, takeRequestedVaultTab } from './handoff-api'
import { IncomingList, SentList } from './HandoffLists'
import type { ServerScope } from '@/kessel/server-scope'
import { useActiveHostRole } from './useActiveHostRole'
import {
  AddSecretDialog,
  DeleteDialog,
  DeliveryDialog,
  HistoryDialog,
  RecoveryDialog,
  RevealDialog,
  RotateDialog,
  ShareDialog,
  SitesDialog,
} from './VaultDialogs'
import { groupSecrets, kindLabel, sitesShort, whenWords } from './vault-copy'

export {
  REVEAL_SHOW_MS,
  ceilingChoices,
  deliveryChoices,
  deliveryLabel,
  deliveryReadable,
  groupSecrets,
  isMultilineKind,
  originLabel,
  withCurrent,
  type SecretGroup,
} from './vault-copy'

export const VAULT_MANIFEST: SettingEntry[] = [
  {
    id: 'vault.status',
    section: 'vault',
    label: 'Vault',
    description: 'Where the vault key lives, the recovery key, and what the vault protects on this server',
    keywords: ['vault', 'secret', 'secrets', 'key', 'recovery', 'keychain', 'tamper'],
  },
  {
    id: 'vault.secrets',
    section: 'vault',
    label: 'Secrets',
    description: 'Secrets your agents use: sites, how they get them, sharing, rotate, reveal, history',
    keywords: ['vault', 'secret', 'api key', 'token', 'password', 'rotate', 'reveal', 'share'],
  },
  {
    id: 'vault.requests',
    section: 'vault',
    label: 'Secret requests',
    description: 'Thread secret cards: waiting, saved, declined, expired',
    keywords: ['vault', 'secret card', 'thread', 'request'],
  },
  {
    id: 'vault.incoming',
    section: 'vault',
    label: 'Secrets from other agents',
    description: 'Secrets agents sent to each other: waiting for you, accepted, declined; what your agents sent',
    keywords: ['vault', 'handoff', 'incoming', 'sent', 'send secret', 'accept', 'decline', 'agent to agent'],
  },
]

const CHECK = (
  <svg aria-hidden="true" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round" className="w-3 h-3 mt-0.5 flex-shrink-0">
    <path d="M3.5 8.5 6.5 11.5 12.5 4.5" />
  </svg>
)

const OWNER_ONLY_REVEAL = 'Only an Owner or Admin of this server can see a value.'

// ── Rows ─────────────────────────────────────────────────────────────

type RowAction = 'reveal' | 'rotate' | 'sites' | 'delivery' | 'share' | 'toggle' | 'history' | 'copy' | 'delete'

/** A state tag a person must notice (Off, Expired, Waiting for you), in
 *  sentence case so it reads as words, not a shout. */
export function StateTag({ tone, children, title, testId }: { tone: TagTone; children: string; title?: string; testId?: string }): React.JSX.Element {
  return (
    <Tag tone={tone} data-case="as-written" title={title} data-testid={testId}>
      {children}
    </Tag>
  )
}

/** One quiet line that never wraps: the full text is its title. */
const QUIET_LINE = 'type-hint text-[var(--color-text-secondary)] mt-0.5 leading-relaxed truncate'

const isPast = (ts: string | null | undefined): boolean => !!ts && Number.isFinite(Date.parse(ts)) && Date.parse(ts) < Date.now()

/** One secret in two short lines: its own words (or name), then quietly
 *  NAME · kind · sites, and "agents can read it" when they can. A tag only
 *  for a state to notice (Off, Expired). Where it came from, its version and
 *  how agents get it are in History and How agents get it. Show value on the
 *  right; everything else in the row's menu, Delete last. */
export function SecretRow({
  secret,
  member,
  busy,
  onAction,
}: {
  secret: VaultSecret
  member: boolean
  busy: boolean
  onAction: (a: RowAction) => void
}): React.JSX.Element {
  const off = secret.status === 'disabled'
  const expired = isPast(secret.expiresAt)
  const sites = secret.hostsDisplay ?? secret.hosts.map((origin) => ({ origin, unicode: '', mixedScript: false }))
  const siteNames = sites.map((h) => siteLabel(h.origin))
  const mixed = sites.filter((h) => h.mixedScript)
  const title = secret.description?.trim() || secret.name
  const facts = [
    ...(title !== secret.name ? [secret.name] : []),
    kindLabel(secret.kind),
    sitesShort(siteNames),
    ...(secret.expiresAt && !expired ? [`until ${shortDay(secret.expiresAt) || whenWords(secret.expiresAt)}`] : []),
  ].join(' · ')
  return (
    <div className="px-4 py-3 transition-colors hover:bg-[var(--color-wash-1)]" data-testid="vault-secret-row" data-secret={secret.name}>
      <div className="flex items-center justify-between gap-6">
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-2 min-w-0">
            <span className={`type-name truncate ${off ? 'text-[var(--color-text-secondary)]' : 'text-[var(--color-text-primary)]'}`} title={title}>
              {title}
            </span>
            {off ? <StateTag tone="muted" title="Agents can’t use it until you turn it on.">Off</StateTag> : null}
            {expired ? <StateTag tone="muted" title="Rotate it to give agents a new value.">Expired</StateTag> : null}
          </div>
          <p className={QUIET_LINE} title={[facts, siteNames.length > 1 ? `Sites: ${siteNames.join(', ')}` : ''].filter(Boolean).join('\n')}>
            {facts}
            {secret.agentCanSee ? (
              <span data-testid="vault-agent-can-see" title="Agents get the value itself, so they can read it.">
                {' · agents can read it'}
              </span>
            ) : null}
          </p>
          {mixed.map((h) => (
            <p key={h.origin} className={`${SETTING_ROW_HINT} text-[var(--color-status-error-soft)]`} data-testid="vault-mixed-script">
              <span className="font-mono">{siteLabel(h.origin)}</span> uses mixed scripts{h.unicode ? ` and reads as ${h.unicode}` : ''}. Check it’s the site you meant.
            </p>
          ))}
        </div>
        <div className="flex items-center gap-2 flex-shrink-0">
          <Button
            variant="quiet"
            onClick={() => onAction('reveal')}
            disabled={member || busy}
            title={member ? OWNER_ONLY_REVEAL : undefined}
            aria-label={`Show the value of ${secret.name}`}
            data-testid="vault-row-reveal"
          >
            Show value
          </Button>
          <OverflowMenu
            label={`More actions for ${secret.name}`}
            items={[
              { id: 'rotate', label: 'Rotate…', hint: 'Paste a new value', onSelect: () => onAction('rotate') },
              { id: 'sites', label: 'Edit sites…', onSelect: () => onAction('sites') },
              { id: 'delivery', label: 'How agents get it…', onSelect: () => onAction('delivery') },
              { id: 'share', label: 'Share…', onSelect: () => onAction('share') },
              {
                id: 'toggle',
                label: off ? 'Turn on' : 'Turn off',
                hint: off ? 'Agents can use it again' : 'Agents can’t use it until you turn it on',
                disabled: busy,
                onSelect: () => onAction('toggle'),
              },
              { id: 'history', label: 'History…', group: true, onSelect: () => onAction('history') },
              ...(secret.ref ? [{ id: 'copy', label: 'Copy reference', hint: 'How an agent names it', onSelect: () => onAction('copy') }] : []),
              { id: 'delete', label: 'Delete…', danger: true, group: true, onSelect: () => onAction('delete') },
            ]}
          />
        </div>
      </div>
    </div>
  )
}

/** K2's own secrets: metadata only, never actions. The one exception is a
 *  secret the daemon marks `personReveal` (Sew's recovery key): the Owner or
 *  an Admin may reveal it, with the same confirm as any reveal. */
function SystemRow({ secret, onReveal }: { secret: VaultSecret; onReveal: () => void }): React.JSX.Element {
  const title = secret.description?.trim() || secret.name
  return (
    <div className="px-4 py-3" data-testid="vault-system-row">
      <div className="flex items-center justify-between gap-6">
        <div className="flex-1 min-w-0">
          <p className="type-name text-[var(--color-text-primary)] truncate" title={title}>
            {title}
          </p>
          <p className={QUIET_LINE}>
            {title !== secret.name ? `${secret.name} · ` : ''}
            {kindLabel(secret.kind)}
            {secret.status === 'disabled' ? ' · off' : secret.status !== 'active' ? ` · ${secret.status}` : ''}
          </p>
        </div>
        {secret.personReveal && secret.status === 'active' ? (
          <Button variant="quiet" onClick={onReveal} aria-label={`Show the value of ${secret.name}`} data-testid="vault-reveal-start">
            Show value
          </Button>
        ) : null}
      </div>
    </div>
  )
}

// ── Requests ─────────────────────────────────────────────────────────

/** Only states a person must notice get a tag; the rest are words. */
const REQUEST_TAG: Record<string, { label: string; tone: TagTone }> = {
  pending: { label: 'Waiting for you', tone: 'warn' },
  expired: { label: 'Expired', tone: 'muted' },
}

/** "Saved by alex · asked by atlas · 2 Nov 11:05" */
function requestLine(r: VaultRequest): string {
  const when = whenWords(r.createdAt)
  const asked = `asked by ${r.requestedBy}`
  const lead =
    r.status === 'set'
      ? `Saved${r.answeredBy ? ` by ${r.answeredBy}` : ''} · ${asked}`
      : r.status === 'void'
        ? `Declined${r.answeredBy ? ` by ${r.answeredBy}` : ''} · ${asked}`
        : `Asked by ${r.requestedBy}`
  return when ? `${lead} · ${when}` : lead
}

function RequestsList(): React.JSX.Element {
  const [rows, setRows] = useState<VaultRequest[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [nonce, setNonce] = useState(0)
  useEffect(() => {
    let live = true
    vaultApi
      .requests()
      .then((r) => {
        if (!live) return
        setRows(r)
        setError(null)
      })
      .catch((e) => live && setError(daemonErrorText(e)))
    return () => {
      live = false
    }
  }, [nonce])
  const retry = (): void => {
    setError(null)
    setRows(null)
    setNonce((n) => n + 1)
  }
  return (
    <SettingsGroup
      title="Secret requests"
      hint="When an agent asks you for a secret in a Thread, the card is listed here too. Answer it in that Thread."
      card
      testId="vault-requests"
      state={
        error ? (
          <SettingsLoadState kind="error" title="Couldn’t load the requests" detail={error} action={<Button onClick={retry}>Try again</Button>}>
            Nothing was lost. Try again in a moment.
          </SettingsLoadState>
        ) : rows === null ? (
          <SettingsLoadState kind="loading" title="Loading requests…" />
        ) : rows.length === 0 ? (
          <SettingsLoadState kind="empty" title="No requests yet">
            When an agent needs a secret it doesn’t have, it asks in a Thread, and the card shows here.
          </SettingsLoadState>
        ) : undefined
      }
    >
      {rows?.map((r) => {
        const tag = REQUEST_TAG[r.status]
        return (
          <div key={r.cardId} className="px-4 py-3" data-testid="vault-request-row" data-status={r.status}>
            <div className="flex items-center gap-2 min-w-0">
              <span className="type-name text-[var(--color-text-primary)] font-mono truncate">{r.name}</span>
              {tag ? <StateTag tone={tag.tone}>{tag.label}</StateTag> : null}
            </div>
            <p className={QUIET_LINE}>{requestLine(r)}</p>
            {r.prompt ? <p className="type-hint text-[var(--color-text-secondary)] mt-0.5 leading-relaxed whitespace-pre-wrap break-words">“{r.prompt}”</p> : null}
          </div>
        )
      })}
    </SettingsGroup>
  )
}

// ── About this vault ────────────────────────────────────────────────

function Value({ children, tone = 'plain', testId }: { children: React.ReactNode; tone?: 'plain' | 'bad'; testId?: string }): React.JSX.Element {
  return (
    <span data-testid={testId} className={`type-row ${tone === 'bad' ? 'text-[var(--color-status-error-soft)]' : 'text-[var(--color-text-secondary)]'}`}>
      {children}
    </span>
  )
}

const cap = (s: string): string => s.charAt(0).toUpperCase() + s.slice(1)

function AboutVault({ status }: { status: VaultStatus }): React.JSX.Element {
  const recoveryOk = status.recovery === 'confirmed'
  const tamperOk = !status.tamper || status.tamper === 'ok'
  return (
    <SettingsGroup
      title="About this vault"
      hint={status.honest ? <span data-testid="vault-honest">{status.honest}</span> : undefined}
      card
      settingId="vault.status"
      testId="vault-status"
    >
      <SettingRow label="Where the key lives" hint="The key that unlocks every secret here.">
        <Value testId="vault-key-home">{status.keyHomeLabel || status.keyHome || 'Unknown'}</Value>
      </SettingRow>
      <SettingRow label="Recovery key" hint="The way back in if that key is ever lost.">
        {recoveryOk ? (
          <Value>Set up{status.recoveryConfirmedAt ? ` ${whenWords(status.recoveryConfirmedAt).split(' ').slice(0, 2).join(' ')}` : ''}</Value>
        ) : (
          <Tag tone="warn">Not set up</Tag>
        )}
      </SettingRow>
      <SettingRow
        label="Rollback check"
        hint="Notices if the vault file is swapped for an older copy."
        below={status.rollback ? <p className="type-hint text-[var(--color-status-error-soft)]">The vault file looks older than expected.</p> : undefined}
      >
        <Value tone={status.rollback ? 'bad' : 'plain'}>{cap(status.rollbackGuard || 'off')}</Value>
      </SettingRow>
      <SettingRow label="Tamper check" hint="Checks the vault file hasn’t been changed outside K2.">
        <Value tone={tamperOk ? 'plain' : 'bad'} testId="vault-tamper">
          {tamperOk ? 'No problems' : status.tamper}
        </Value>
      </SettingRow>
      <SettingRow label="Vault password" hint="Confirms it’s you where Touch ID isn’t available.">
        <Value>{status.confirmPassword ? 'Set' : 'Not set'}</Value>
      </SettingRow>
      {status.counts ? (
        <SettingRow label="Secrets" hint="Yours, K2’s own, and requests waiting for you.">
          <Value>
            {status.counts.user} yours · {status.counts.system} K2’s · {status.counts.pendingRequests} waiting
            {status.counts.rotateDue ? ` · ${status.counts.rotateDue} due for rotation` : ''}
          </Value>
        </SettingRow>
      ) : null}
      {status.adopted && status.adopted.length > 0 ? (
        <div className="px-4 py-3">
          <p className={SETTING_ROW_LABEL}>Moved in from older storage</p>
          <p className={SETTING_ROW_HINT} data-testid="vault-adopted">
            {status.adopted.map((a) => `${a.store} (${a.moved} moved${a.kept ? `, ${a.kept} kept` : ''})`).join(', ')}.
            {status.legacyCopies ? ` ${cap(status.legacyCopies)}.` : ''}
          </p>
        </div>
      ) : null}
      <div className="px-4 py-3">
        <p className={SETTING_ROW_LABEL}>What it can’t stop yet</p>
        <p className={SETTING_ROW_HINT}>
          An agent that gets hold of this server’s owner token can change a secret’s sites or how it’s given out, as you can. Each secret’s history
          records it.
        </p>
      </div>
    </SettingsGroup>
  )
}

/** The vault has no confirmed recovery key: what that risks, and the one
 *  step to fix it (the Owner's; a Member is told who can). */
function RecoveryPrompt({ member, onSetUp }: { member: boolean; onSetUp: () => void }): React.JSX.Element {
  return (
    <Surface chamfer material="tile" className="px-4 py-3" data-testid="vault-recovery-banner">
      <div className="flex items-start justify-between gap-6">
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <p className={SETTING_ROW_LABEL}>Set up a recovery key</p>
            <Tag tone="warn">Not set up</Tag>
          </div>
          <p className={SETTING_ROW_HINT}>
            Without one, losing this server’s vault key means losing every secret in it. It takes a minute: you write down 24 words and type three
            back.
          </p>
          {member ? <p className={SETTING_ROW_HINT}>Only the server’s owner can set it up.</p> : null}
        </div>
        {member ? null : (
          <Button className="flex-shrink-0" onClick={onSetUp} data-testid="vault-recovery-open">
            Set up recovery key
          </Button>
        )}
      </div>
    </Surface>
  )
}

// ── Page ─────────────────────────────────────────────────────────────

type VaultTab = 'secrets' | 'incoming' | 'sent' | 'requests'

const TAB_LABEL: Record<VaultTab, string> = { secrets: 'Secrets', incoming: 'Incoming', sent: 'Sent', requests: 'Requests' }
const TAB_SETTINGS_ID: Record<VaultTab, string> = {
  secrets: 'vault.secrets',
  incoming: 'vault.incoming',
  sent: 'vault.sent',
  requests: 'vault.requests',
}

type Dialog =
  | { kind: 'add' }
  | { kind: 'recovery' }
  | { kind: Exclude<RowAction, 'toggle' | 'copy'>; secret: VaultSecret }
  | null

type Notice = { tone: 'ok'; text: string } | { tone: 'error'; text: string; detail: string } | null

/** `scope`: this window's server, from Settings (Incoming/Sent list and decide there). */
export function VaultSection({ scope }: { scope: ServerScope }): React.JSX.Element {
  const handoff = useServerSupports('vault-handoff')
  const { role } = useActiveHostRole()
  // A Member keeps workspace secrets and changes their own; showing values
  // and server-wide secrets are an Owner's or Admin's. The server decides.
  const member = role.kind === 'known' && role.role === 'member'
  const [tab, setTab] = useState<VaultTab>(() => {
    const want = takeRequestedVaultTab()
    return want === 'incoming' || want === 'sent' ? want : 'secrets'
  })
  const tabs: VaultTab[] = handoff ? ['secrets', 'incoming', 'sent', 'requests'] : ['secrets', 'requests']
  const [status, setStatus] = useState<VaultStatus | null>(null)
  const [secrets, setSecrets] = useState<VaultSecret[]>([])
  const [system, setSystem] = useState<VaultSecret[]>([])
  const [presets, setPresets] = useState<VaultPreset[]>([])
  const [error, setError] = useState<string | null>(null)
  const [loaded, setLoaded] = useState(false)
  const [dialog, setDialog] = useState<Dialog>(null)
  const [notice, setNotice] = useState<Notice>(null)
  const [busyId, setBusyId] = useState<string | null>(null)

  const refresh = useCallback(async () => {
    const [st, list, sys, pre] = await Promise.allSettled([
      vaultApi.status(),
      vaultApi.list(),
      vaultApi.list({ system: true }),
      vaultApi.presets(),
    ])
    if (st.status === 'fulfilled') setStatus(st.value)
    if (list.status === 'fulfilled') {
      setSecrets(list.value)
      setError(null)
    } else {
      setError(daemonErrorText(list.reason))
    }
    // K2's own secrets only when the daemon hands them back (it filters).
    setSystem(sys.status === 'fulfilled' ? sys.value.filter((s) => s.namespace === 'system') : [])
    if (pre.status === 'fulfilled') setPresets(pre.value)
    setLoaded(true)
  }, [])

  useEffect(() => {
    void refresh()
  }, [refresh])

  const groups = useMemo(() => groupSecrets(secrets), [secrets])
  const vaultClosed = status !== null && !status.open
  const close = (): void => setDialog(null)
  const saved = (text: string): void => {
    setDialog(null)
    setNotice({ tone: 'ok', text })
    void refresh()
  }
  const retry = (): void => {
    setLoaded(false)
    setError(null)
    void refresh()
  }

  const onRow = async (s: VaultSecret, a: RowAction): Promise<void> => {
    setNotice(null)
    if (a === 'copy') {
      const clip = typeof navigator !== 'undefined' ? navigator.clipboard : undefined
      if (!clip || !s.ref) return setNotice({ tone: 'error', text: 'Couldn’t copy the reference.', detail: 'the clipboard isn’t available here' })
      try {
        await clip.writeText(s.ref)
        setNotice({ tone: 'ok', text: `Copied ${s.name}’s reference. An agent can ask for it by that name.` })
      } catch (e) {
        setNotice({ tone: 'error', text: 'Couldn’t copy the reference.', detail: daemonErrorText(e) })
      }
      return
    }
    if (a === 'toggle') {
      const off = s.status === 'disabled'
      setBusyId(s.id)
      try {
        if (off) await vaultApi.enable(s.id)
        else await vaultApi.disable(s.id)
        setNotice({
          tone: 'ok',
          text: off ? `${s.name} is on again. Agents can use it from their next call.` : `${s.name} is off. Agents can’t use it until you turn it on.`,
        })
        void refresh()
      } catch (e) {
        setNotice({ tone: 'error', text: off ? `Couldn’t turn ${s.name} on.` : `Couldn’t turn ${s.name} off.`, detail: daemonErrorText(e) })
      } finally {
        setBusyId(null)
      }
      return
    }
    setDialog({ kind: a, secret: s })
  }

  const header = (
    <header className="min-w-0">
      <h2 className="type-title text-[var(--color-text-primary)]">Vault</h2>
      <p className="type-hint text-[var(--color-text-secondary)] mt-1 leading-relaxed max-w-[75ch]">
        API keys, passwords and tokens your agents use. Each one is sent only to the sites you pick, and every use is logged.
      </p>
      <AccessAuditLink className="mt-1" />
    </header>
  )

  if (vaultClosed && status) {
    return (
      <div className="w-full max-w-[52rem] space-y-6 pb-6">
        {header}
        <SettingsLoadState
          kind="error"
          title="The vault isn’t open"
          detail={status.error?.hint || status.error?.code || null}
          action={<Button onClick={retry}>Try again</Button>}
          testId="vault-not-open"
        >
          Your secrets are kept safe, but until it opens agents can’t use them and you can’t add or change any. If your Mac asks for its password,
          enter it to unlock the keychain; if nothing asks, quit and reopen K2. Then choose Try again.
        </SettingsLoadState>
      </div>
    )
  }

  const secretsTab = (
    <div className="space-y-6">
      <div className="flex items-center justify-between gap-6">
        <p className="type-hint text-[var(--color-text-secondary)] leading-relaxed max-w-[75ch]">
          {member
            ? 'As a Member you can add secrets for a workspace and change the ones you added. Seeing a value needs an Owner or Admin.'
            : 'Grouped by which agents can use them. Show a value, rotate it or change where it goes from its row.'}
        </p>
        <div className="flex items-center gap-2 flex-shrink-0">
          <Button variant="ghost" onClick={retry} disabled={!loaded}>
            Refresh
          </Button>
          <Button
            variant="accent"
            disabled={vaultClosed}
            onClick={() => {
              setNotice(null)
              setDialog({ kind: 'add' })
            }}
            data-testid="vault-add"
          >
            Add secret
          </Button>
        </div>
      </div>

      {notice ? (
        <Surface chamfer material="tile" className="px-4 py-2.5" data-testid="vault-notice">
          <div className="flex items-start justify-between gap-4">
            {notice.tone === 'ok' ? (
              <p role="status" className="flex items-start gap-1.5 type-hint text-[var(--color-status-ok-soft)]">
                {CHECK}
                {notice.text}
              </p>
            ) : (
              <div role="alert" className="min-w-0">
                <p className="type-row text-[var(--color-status-error-soft)]">{notice.text}</p>
                <p className={SETTING_ROW_HINT}>Nothing changed. Try again. The server said: {notice.detail.trim().replace(/[.\s]+$/, '')}.</p>
              </div>
            )}
            <Button variant="quiet" className="flex-shrink-0" onClick={() => setNotice(null)}>
              Dismiss
            </Button>
          </div>
        </Surface>
      ) : null}

      {status && status.open && status.recovery !== 'confirmed' ? (
        <RecoveryPrompt member={member} onSetUp={() => setDialog({ kind: 'recovery' })} />
      ) : null}

      {!loaded ? (
        <SettingsLoadState kind="loading" title="Opening the vault…" testId="vault-loading">
          Reading the secrets on this server.
        </SettingsLoadState>
      ) : error ? (
        <SettingsLoadState kind="error" title="Couldn’t load your secrets" detail={error} action={<Button onClick={retry}>Try again</Button>} testId="vault-error">
          Your secrets are safe; this page just couldn’t read the list. Try again in a moment.
        </SettingsLoadState>
      ) : groups.length === 0 ? (
        <SettingsLoadState kind="empty" title="No secrets yet" testId="vault-empty">
          Add an API key, password or token with Add secret. You choose the sites K2 may send it to.
        </SettingsLoadState>
      ) : (
        groups.map((g) => (
          <div key={g.key} data-testid="vault-group" data-group={g.key}>
            <SettingsGroup title={g.title} hint={g.hint} card>
              {g.rows.map((s) => (
                <SecretRow key={s.id} secret={s} member={member} busy={busyId === s.id} onAction={(a) => void onRow(s, a)} />
              ))}
            </SettingsGroup>
          </div>
        ))
      )}

      {system.length > 0 ? (
        <SettingsGroup title="K2’s own secrets" hint="K2 uses these itself. This page shows their names only." card testId="vault-system">
          {system.map((s) => (
            <SystemRow key={s.id} secret={s} onReveal={() => setDialog({ kind: 'reveal', secret: s })} />
          ))}
        </SettingsGroup>
      ) : null}

      {/* Only beside a list: an empty or failed page keeps to its one card. */}
      {status && status.open && loaded && !error && groups.length > 0 ? <AboutVault status={status} /> : null}
    </div>
  )

  return (
    <div className="w-full max-w-[52rem] space-y-5 pb-6">
      {header}

      <div role="tablist" aria-label="Vault" className="flex flex-wrap gap-0.5 border-b border-[var(--color-border)] w-full">
        {tabs.map((t) => {
          const active = tab === t
          return (
            <button
              key={t}
              type="button"
              role="tab"
              aria-selected={active}
              onClick={() => setTab(t)}
              data-settings-id={TAB_SETTINGS_ID[t]}
              data-k2-focus=""
              className={`relative px-3 py-2 type-control transition-colors no-drag cursor-pointer border-b-2 -mb-px outline-none ${
                active
                  ? 'border-[var(--color-accent)] text-[var(--color-text-primary)]'
                  : 'border-transparent text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)]'
              }`}
            >
              {TAB_LABEL[t]}
            </button>
          )
        })}
      </div>

      <div className="pt-1">
        {tab === 'secrets' && secretsTab}
        {tab === 'requests' && <RequestsList />}
        {handoff && tab === 'incoming' && <IncomingList scope={scope} />}
        {handoff && tab === 'sent' && <SentList scope={scope} />}
      </div>

      {dialog?.kind === 'add' ? <AddSecretDialog presets={presets} member={member} onClose={close} onSaved={saved} /> : null}
      {dialog?.kind === 'recovery' ? <RecoveryDialog onClose={close} onConfirmed={() => void refresh()} /> : null}
      {dialog?.kind === 'reveal' ? <RevealDialog secret={dialog.secret} onClose={close} /> : null}
      {dialog?.kind === 'rotate' ? <RotateDialog secret={dialog.secret} onClose={close} onSaved={saved} /> : null}
      {dialog?.kind === 'sites' ? <SitesDialog secret={dialog.secret} onClose={close} onChanged={() => void refresh()} /> : null}
      {dialog?.kind === 'delivery' ? <DeliveryDialog secret={dialog.secret} onClose={close} onSaved={saved} /> : null}
      {dialog?.kind === 'share' ? <ShareDialog secret={dialog.secret} onClose={close} onSaved={saved} /> : null}
      {dialog?.kind === 'delete' ? <DeleteDialog secret={dialog.secret} onClose={close} onDeleted={saved} /> : null}
      {dialog?.kind === 'history' ? <HistoryDialog secret={dialog.secret} onClose={close} /> : null}
    </div>
  )
}

