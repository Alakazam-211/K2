// prd-zen-user-widgets-v2 UW22, UW23, UW25, UWB7, UWB9, UWB22, R6, R7 —
// K2's review dialog for a custom widget, and the scope picker it shares
// with New Garden's ready-made Gardens.
//
// Drawn by K2 over the window (portalled into the Zen root, outside every
// widget frame), never by the widget. It shows:
//   - "Allow “<name>” in <Garden>?" and who wrote it;
//   - each requested cap in K2's words (the shared cap table's `sentence`,
//     `{where}` = the scope picked below);
//   - which agents: a scope picker (one agent · a Home · some Homes · all
//     my Homes · a server · every server), or fixed when the placement
//     names `home` / `agent`; a live count of the agents it covers, and the
//     8-server limit (R7);
//   - Sending (when it asks for `thread:post`): on by default (R6); with
//     every server, a second line to tick (R7);
//   - "In its own words:" the description and reasons, plain text, cut;
//   - K2's limits line.
// Allow seals nothing in the renderer: the daemon signs and stores the
// grant (UWB4) through the owner-only route. A widget that changed while
// the dialog was open is 409 `widget_changed` → "Review it again."

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { useHomesStore } from '@/stores/homes'
import { useConnectHostStore } from '@/stores/connect-host'
import type { UserWidgetCap } from '@/lib/k2-caps.generated'
import type { ZenCustomWidgetPayload, ZenScope } from '@/lib/zen/zen-custom-types'
import {
  useZenServerRosters,
  zenAskScope,
  zenGrantEntries,
  zenKnownServers,
  zenScopeKind,
  zenScopeRows,
  zenScopeWhere,
  zenServerName,
  ZEN_WIDGET_MAX_SERVERS,
  type ZenPlacementAsk,
  type ZenScopeKind,
} from '@/lib/zen/zen-custom-scope'
import {
  ZEN_WIDGET_AUTHOR_TEXT,
  ZEN_WIDGET_DESCRIPTION_MAX,
  ZEN_WIDGET_LIMITS_TEXT,
  ZEN_WIDGET_REASON_MAX,
  zenCapSentence,
  zenClip,
  zenWidgetDisplayName,
} from '@/lib/zen/zen-custom-words'
import { grantZenWidget, revokeZenWidget, ZenWidgetError, zenWidgetError } from '@/lib/zen/zen-custom-grants'
import { parseHomeAddress } from '@/lib/home-address'
import { registerZenK2Overlay } from '@/lib/zen/zen-controls'
import { loadZenTemplates, useZenTemplatesStore } from '@/lib/zen/zen-templates'
import { zenCatalogConsent, zenCatalogFixedScopeFor } from '@/lib/zen/zen-new-garden'

// ── Scope picker ──────────────────────────────────────────────────────────

const KIND_LABEL: Record<ZenScopeKind, string> = {
  agent: 'One agent',
  home: 'One Home',
  homes: 'Some Homes',
  allHomes: 'All my Homes',
  server: 'One server',
  allServers: 'Every server I use',
}

const KINDS: readonly ZenScopeKind[] = ['agent', 'home', 'homes', 'allHomes', 'server', 'allServers']

interface AgentOption {
  address: string
  label: string
}

function agentOptions(): AgentOption[] {
  const out: AgentOption[] = []
  const seen = new Set<string>()
  for (const h of useHomesStore.getState().homes) {
    for (const r of h.rows) {
      if (seen.has(r.address)) continue
      seen.add(r.address)
      const host = parseHomeAddress(r.address)?.host ?? ''
      out.push({ address: r.address, label: `${r.label} · ${h.name}${host && host !== 'local' ? ` (${zenServerName(host)})` : ''}` })
    }
  }
  return out
}

/** A default scope when nothing is fixed: the first Home, else this computer. */
export function zenDefaultScope(): ZenScope {
  const first = useHomesStore.getState().homes[0]
  return first ? { home: first.id } : { server: 'local' }
}

const selectStyle: React.CSSProperties = {
  height: 28,
  padding: '0 8px',
  color: 'var(--zen-text)',
  background: 'var(--zen-surface)',
  border: '1px solid var(--zen-border)',
  borderRadius: 'calc(var(--zen-radius) - 4px)',
  font: 'inherit',
  minWidth: 0,
  maxWidth: '100%',
}

/**
 * The scope picker (UWB7). `ask` fixes it when the placement names a Home or
 * an agent (UW22); otherwise the person picks any scope up to every server
 * (Rosson 2026-10-07, decision 4).
 */
export function ZenScopePicker({
  ask,
  value,
  onChange,
}: {
  ask: ZenPlacementAsk
  value: ZenScope | null
  onChange(scope: ZenScope | null): void
}): React.JSX.Element {
  const homes = useHomesStore((s) => s.homes)
  useConnectHostStore((s) => s.hosts)
  const fixed = useMemo(() => zenAskScope(ask), [ask, homes]) // eslint-disable-line react-hooks/exhaustive-deps
  if (fixed) {
    return (
      <div data-zen-scope-fixed="" style={{ color: 'var(--zen-text)' }}>
        {'missing' in fixed ? (
          <span data-zen-scope-missing="" style={{ color: 'var(--zen-danger)' }}>
            {fixed.missing} Ask your agent to fix the Garden, or add it on Home.
          </span>
        ) : (
          <span>{zenScopeWhere(fixed.scope).replace(/^only /, 'Only ')}</span>
        )}
      </div>
    )
  }
  const kind = value ? zenScopeKind(value) : 'home'
  const agents = agentOptions()
  const servers = zenKnownServers()
  const pick = (k: ZenScopeKind): void => {
    switch (k) {
      case 'agent':
        onChange(agents[0] ? { agent: agents[0].address } : null)
        return
      case 'home':
        onChange(homes[0] ? { home: homes[0].id } : null)
        return
      case 'homes':
        onChange(homes[0] ? { homes: [homes[0].id] } : null)
        return
      case 'allHomes':
        onChange({ allHomes: true })
        return
      case 'server':
        onChange({ server: servers[0] })
        return
      case 'allServers':
        onChange({ allServers: true })
    }
  }
  return (
    <div className="flex flex-col" style={{ gap: 6 }} data-zen-scope-picker="" role="radiogroup" aria-label="Which agents">
      {KINDS.map((k) => {
        const on = kind === k
        const disabled = (k === 'agent' && agents.length === 0) || ((k === 'home' || k === 'homes') && homes.length === 0)
        return (
          <div key={k} className="flex flex-wrap items-center" style={{ gap: 8 }}>
            <label className="flex items-center cursor-pointer" style={{ gap: 6, opacity: disabled ? 0.5 : 1 }}>
              <input
                type="radio"
                name="zen-scope-kind"
                data-zen-scope-kind={k}
                checked={on}
                disabled={disabled}
                onChange={() => pick(k)}
              />
              <span>{KIND_LABEL[k]}</span>
            </label>
            {on && k === 'agent' && value && 'agent' in value && (
              <select
                aria-label="Agent"
                data-zen-scope-agent=""
                value={value.agent}
                onChange={(e) => onChange({ agent: e.target.value })}
                style={selectStyle}
              >
                {agents.map((a) => (
                  <option key={a.address} value={a.address}>
                    {a.label}
                  </option>
                ))}
              </select>
            )}
            {on && k === 'home' && value && 'home' in value && (
              <select
                aria-label="Home"
                data-zen-scope-home=""
                value={value.home}
                onChange={(e) => onChange({ home: e.target.value })}
                style={selectStyle}
              >
                {homes.map((h) => (
                  <option key={h.id} value={h.id}>
                    {h.name}
                  </option>
                ))}
              </select>
            )}
            {on && k === 'server' && value && 'server' in value && (
              <select
                aria-label="Server"
                data-zen-scope-server=""
                value={value.server}
                onChange={(e) => onChange({ server: e.target.value })}
                style={selectStyle}
              >
                {servers.map((s) => (
                  <option key={s} value={s}>
                    {zenServerName(s)}
                  </option>
                ))}
              </select>
            )}
          </div>
        )
      })}
      {value && 'homes' in value && (
        <div className="flex flex-wrap" style={{ gap: 8, paddingLeft: 22 }} data-zen-scope-homes="">
          {homes.map((h) => {
            const on = value.homes.includes(h.id)
            return (
              <label key={h.id} className="flex items-center cursor-pointer" style={{ gap: 4 }}>
                <input
                  type="checkbox"
                  data-zen-scope-home-check={h.id}
                  checked={on}
                  onChange={() => {
                    const next = on ? value.homes.filter((x) => x !== h.id) : [...value.homes, h.id]
                    onChange(next.length > 0 ? { homes: next } : null)
                  }}
                />
                <span>{h.name}</span>
              </label>
            )
          })}
        </div>
      )}
    </div>
  )
}

/** The scope the picker stands for: the fixed ask, else the pick. */
export function zenEffectiveScope(ask: ZenPlacementAsk, picked: ZenScope | null): ZenScope | null {
  const fixed = zenAskScope(ask)
  if (fixed) return 'scope' in fixed ? fixed.scope : null
  return picked
}

/** "4 agents on 2 servers", plus the R7 limit when it bites. */
export function ZenScopeCount({ scope }: { scope: ZenScope | null }): React.JSX.Element | null {
  useHomesStore((s) => s.homes)
  useZenServerRosters((s) => s.rosters)
  if (!scope) return null
  const { rows, servers, overflow } = zenScopeRows(scope)
  const n = rows.length
  return (
    <div data-zen-scope-count={n} style={{ fontSize: '0.85em', color: 'var(--zen-text-muted)' }}>
      {n === 0
        ? 'No agents there yet.'
        : `${n} agent${n === 1 ? '' : 's'} now${servers.length > 1 ? ` on ${servers.length} servers` : ''}. Agents added there later are included.`}
      {overflow.length > 0 && (
        <span data-zen-scope-overflow="">
          {' '}
          A widget can reach {ZEN_WIDGET_MAX_SERVERS} servers at a time; {overflow.map(zenServerName).join(', ')}{' '}
          {overflow.length === 1 ? 'is' : 'are'} left out.
        </span>
      )}
    </div>
  )
}

// ── Sending ───────────────────────────────────────────────────────────────

export const ZEN_SENDING_LABEL = 'Let it send messages to these agents as you'
export const ZEN_SENDING_EVERY_SERVER_TEXT =
  'I understand it can send messages as me to agents on every server I use.'

export function ZenSendingChoice({
  sending,
  onSending,
  everyServer,
  confirmed,
  onConfirmed,
}: {
  sending: boolean
  onSending(on: boolean): void
  everyServer: boolean
  confirmed: boolean
  onConfirmed(on: boolean): void
}): React.JSX.Element {
  return (
    <div className="flex flex-col" style={{ gap: 4 }}>
      <label className="flex items-center cursor-pointer" style={{ gap: 6 }}>
        <input type="checkbox" data-zen-sending-choice="" checked={sending} onChange={(e) => onSending(e.target.checked)} />
        <span>{ZEN_SENDING_LABEL}</span>
      </label>
      <span style={{ fontSize: '0.8em', color: 'var(--zen-text-muted)', paddingLeft: 22 }}>
        Text only, never files. You can turn sending off any time from the widget’s ⋯ menu.
      </span>
      {sending && everyServer && (
        <label className="flex items-center cursor-pointer" style={{ gap: 6, paddingLeft: 22 }}>
          <input
            type="checkbox"
            data-zen-sending-confirm=""
            checked={confirmed}
            onChange={(e) => onConfirmed(e.target.checked)}
          />
          <span>{ZEN_SENDING_EVERY_SERVER_TEXT}</span>
        </label>
      )}
    </div>
  )
}

// ── The dialog ────────────────────────────────────────────────────────────

/** Portal target: the Zen root (keeps Zen's tokens), else the body. */
function useZenLayer(anchor: React.RefObject<HTMLElement | null>): HTMLElement | null {
  const [layer, setLayer] = useState<HTMLElement | null>(null)
  useEffect(() => {
    const root = anchor.current?.closest<HTMLElement>('[data-zen-root]') ?? document.querySelector<HTMLElement>('[data-zen-root]')
    setLayer(root ?? document.body)
  }, [anchor])
  return layer
}

export function ZenOverlay({
  label,
  onClose,
  children,
  testId,
  vars,
  width = 480,
}: {
  label: string
  onClose(): void
  children: React.ReactNode
  testId: string
  /** Extra CSS variables on the overlay (Settings maps `--zen-*` here). */
  vars?: React.CSSProperties
  width?: number
}): React.JSX.Element {
  const anchor = useRef<HTMLSpanElement | null>(null)
  const layer = useZenLayer(anchor)
  const boxRef = useRef<HTMLDivElement | null>(null)
  // A K2 overlay (FC18): while it is open, the required-controls check
  // reads a control under it as covered by K2, not by the page.
  const offOverlay = useRef<(() => void) | null>(null)
  useEffect(() => () => offOverlay.current?.(), [])
  const backdropRef = useCallback((el: HTMLDivElement | null) => {
    offOverlay.current?.()
    offOverlay.current = el ? registerZenK2Overlay(el) : null
  }, [])
  useEffect(() => {
    const onKey = (e: KeyboardEvent): void => {
      if (e.key !== 'Escape') return
      e.preventDefault()
      e.stopPropagation()
      onClose()
    }
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  }, [onClose])
  useEffect(() => {
    // A field inside that took focus as it mounted (New Garden's name) keeps it.
    if (boxRef.current && !boxRef.current.contains(document.activeElement)) boxRef.current.focus({ preventScroll: true })
  }, [layer])
  return (
    <>
      <span ref={anchor} hidden />
      {layer &&
        createPortal(
          <div
            ref={backdropRef}
            className="no-drag flex items-center justify-center"
            data-zen-overlay=""
            style={{ ...vars, position: 'fixed', inset: 0, zIndex: 50, background: 'rgba(0, 0, 0, 0.32)' }}
            onMouseDown={(e) => {
              if (e.target === e.currentTarget) onClose()
            }}
          >
            <div
              ref={boxRef}
              role="dialog"
              aria-modal="true"
              aria-label={label}
              tabIndex={-1}
              data-testid={testId}
              className="flex flex-col"
              style={{
                width: `min(${width}px, calc(100vw - 32px))`,
                maxHeight: 'calc(100vh - 48px)',
                overflowY: 'auto',
                gap: 14,
                padding: 20,
                color: 'var(--zen-text)',
                background: 'var(--zen-surface-raised)',
                border: '1px solid var(--zen-border)',
                borderRadius: 'var(--zen-radius)',
                boxShadow: '0 18px 50px rgba(0, 0, 0, 0.25)',
                outline: 'none',
              }}
            >
              {children}
            </div>
          </div>,
          layer,
        )}
    </>
  )
}

export const zenButtonStyle = (primary: boolean): React.CSSProperties => ({
  padding: '5px 14px',
  borderRadius: 'calc(var(--zen-radius) - 4px)',
  border: `1px solid ${primary ? 'var(--zen-accent)' : 'var(--zen-border)'}`,
  background: primary ? 'var(--zen-accent)' : 'transparent',
  color: primary ? 'var(--zen-accent-text, #fff)' : 'var(--zen-text)',
  fontWeight: primary ? 600 : 400,
})

/** The widget's own words, labelled as its own (UW22). */
function OwnWords({ widget }: { widget: ZenCustomWidgetPayload }): React.JSX.Element | null {
  const reasons = widget.requested.flatMap((c) => (widget.reasons[c] ? [[c, widget.reasons[c] as string] as const] : []))
  if (!widget.description && reasons.length === 0) return null
  return (
    <div data-zen-grant-own-words="" className="flex flex-col" style={{ gap: 4, fontSize: '0.9em' }}>
      <span style={{ color: 'var(--zen-text-muted)' }}>In its own words:</span>
      {widget.description && <span style={{ fontStyle: 'italic' }}>{zenClip(widget.description, ZEN_WIDGET_DESCRIPTION_MAX)}</span>}
      {reasons.map(([cap, why]) => (
        <span key={cap} style={{ fontStyle: 'italic' }}>
          {zenClip(why, ZEN_WIDGET_REASON_MAX)}
        </span>
      ))}
    </div>
  )
}

export function ZenCapList({ caps, scope }: { caps: readonly UserWidgetCap[]; scope: ZenScope | null }): React.JSX.Element {
  const where = zenScopeWhere(scope)
  return (
    <ul data-zen-grant-caps="" className="flex flex-col" style={{ gap: 4, margin: 0, paddingLeft: 18, listStyle: 'disc' }}>
      {caps.map((c) => (
        <li key={c} data-zen-grant-cap={c}>
          {zenCapSentence(c, where)}
        </li>
      ))}
    </ul>
  )
}

/**
 * Review / Permissions for one placement. `existing` = the grant it has
 * (Permissions… from the corner menu: adds Turn off).
 */
export function ZenGrantDialog({
  widget,
  gardenId,
  gardenName,
  onClose,
}: {
  widget: ZenCustomWidgetPayload
  gardenId: string
  gardenName: string
  onClose(): void
}): React.JSX.Element {
  const ask: ZenPlacementAsk = useMemo(
    () => ({ ...(widget.props.home ? { home: widget.props.home } : {}), ...(widget.props.agent ? { agent: widget.props.agent } : {}) }),
    [widget.props.home, widget.props.agent],
  )
  const existing = widget.grant && widget.grant.state !== 'none' && widget.grant.state !== 'invalid' ? widget.grant : null
  const [picked, setPicked] = useState<ZenScope | null>(() => existing?.scope ?? zenDefaultScope())
  const posts = widget.requested.includes('thread:post')
  const [sending, setSending] = useState<boolean>(existing ? existing.sending : true)
  const [confirmed, setConfirmed] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  useHomesStore((s) => s.homes)
  useZenServerRosters((s) => s.rosters)
  // K2's own catalog widget (the Diary) has its scope fixed by the
  // catalog: this computer's agents. No picker; one plain sentence.
  const templates = useZenTemplatesStore((s) => s.templates)
  useEffect(() => {
    if (widget.widget.startsWith('k2:') && useZenTemplatesStore.getState().status === 'idle') void loadZenTemplates()
  }, [widget.widget])
  const catalogScope = zenCatalogFixedScopeFor(widget.widget, templates)
  const catalogEntry = catalogScope ? (templates.find((t) => t.needsGrant?.widget === widget.widget) ?? null) : null

  const scope = catalogScope ?? zenEffectiveScope(ask, picked)
  const everyServer = scope !== null && zenScopeKind(scope) === 'allServers'
  const rows = scope ? zenScopeRows(scope).rows : []
  const needsConfirm = posts && sending && everyServer && !confirmed
  const canAllow = !busy && scope !== null && rows.length > 0 && !needsConfirm && widget.hash !== ''

  const allow = (): void => {
    if (!canAllow || !scope) return
    setBusy(true)
    setError(null)
    void grantZenWidget({
      garden: gardenId,
      placement: widget.id,
      widget: widget.widget,
      caps: [...widget.requested],
      scope,
      entries: zenGrantEntries(rows),
      sending: posts ? sending : false,
      hash: widget.hash,
    })
      .then(() => onClose())
      .catch((err: unknown) => {
        setBusy(false)
        setError(zenWidgetError(err).message)
      })
  }
  const turnOff = (): void => {
    setBusy(true)
    setError(null)
    void revokeZenWidget({ garden: gardenId, placement: widget.id })
      .then(() => onClose())
      .catch((err: unknown) => {
        setBusy(false)
        setError((err instanceof ZenWidgetError ? err : zenWidgetError(err)).message)
      })
  }
  const name = zenWidgetDisplayName(widget.name)
  return (
    <ZenOverlay label={`Allow ${name}`} onClose={onClose} testId="zen-grant-dialog">
      <div className="flex flex-col" style={{ gap: 4 }}>
        <strong data-zen-grant-title="" style={{ fontSize: '1.05em' }}>
          Allow “{name}” in {gardenName}?
        </strong>
        <span style={{ fontSize: '0.85em', color: 'var(--zen-text-muted)' }}>{ZEN_WIDGET_AUTHOR_TEXT}</span>
      </div>
      {widget.requested.length > 0 && (
        <div className="flex flex-col" style={{ gap: 4 }}>
          <span style={{ color: 'var(--zen-text-muted)' }}>It asks to:</span>
          <ZenCapList caps={widget.requested} scope={scope} />
        </div>
      )}
      {catalogEntry ? (
        <div className="flex flex-col" style={{ gap: 6 }} data-zen-grant-fixed-scope="">
          <span>{zenCatalogConsent(catalogEntry)}</span>
          <ZenScopeCount scope={scope} />
        </div>
      ) : (
        <div className="flex flex-col" style={{ gap: 6 }}>
          <span style={{ color: 'var(--zen-text-muted)' }}>Which agents:</span>
          <ZenScopePicker ask={ask} value={picked} onChange={setPicked} />
          <ZenScopeCount scope={scope} />
        </div>
      )}
      {posts && (
        <ZenSendingChoice
          sending={sending}
          onSending={setSending}
          everyServer={everyServer}
          confirmed={confirmed}
          onConfirmed={setConfirmed}
        />
      )}
      <OwnWords widget={widget} />
      <span style={{ fontSize: '0.85em', color: 'var(--zen-text-muted)' }}>{ZEN_WIDGET_LIMITS_TEXT}</span>
      {error && (
        <span role="alert" data-zen-grant-error="" style={{ color: 'var(--zen-danger)', fontSize: '0.9em' }}>
          {error}
        </span>
      )}
      <div className="flex items-center justify-end" style={{ gap: 8 }}>
        {existing && (
          <button type="button" data-zen-grant-turn-off="" disabled={busy} onClick={turnOff} className="cursor-pointer" style={{ ...zenButtonStyle(false), marginRight: 'auto' }}>
            Turn off
          </button>
        )}
        <button type="button" data-zen-grant-not-now="" disabled={busy} onClick={onClose} className="cursor-pointer" style={zenButtonStyle(false)}>
          {existing ? 'Cancel' : 'Not now'}
        </button>
        <button
          type="button"
          data-zen-grant-allow=""
          disabled={!canAllow}
          onClick={allow}
          className="cursor-pointer disabled:cursor-default"
          style={{ ...zenButtonStyle(true), opacity: canAllow ? 1 : 0.5 }}
        >
          {busy ? 'Allowing…' : 'Allow'}
        </button>
      </div>
    </ZenOverlay>
  )
}
