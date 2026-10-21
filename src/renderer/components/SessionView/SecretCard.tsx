// K2 vault secret card (prd-k2-vault-v1 §10.1, KV9; §23.4 VS12/VS13).
//
// 0.46.3 (Rosson 2026-10-21): the simple card. The agent makes the choices
// (where the secret is kept, which sites it may go to, how the agent gets
// it) with `k2 thread secret`. The card says what it chose in one plain
// sentence ("Sales will get this as a file it can read, for this
// workspace"), and the person pastes the value and clicks Save once. If the
// choice is wrong they tell the agent in the chat, and the agent voids the
// card and asks again. "Advanced" (closed by default) holds the old
// controls for people who want to change things by hand; the sentence
// follows whatever is picked there.
//
// The sentence IS the readable-delivery disclosure: the card sends
// `disclosed` with every answer, and the daemon skips the readable-delivery
// second click for it (older cards and the CLI never send it and keep that
// click). The host-mismatch guard (a key that looks like another provider's
// but would go somewhere else; a tricked agent) stays a second step.
//
// The value: the field is an uncontrolled password input. Its text is read
// from the DOM once, on Save, and handed to `onSubmit` (which POSTs it); it
// is never put in React state, a store, a log or the Thread item, and the
// field is emptied after a successful save. A host-mismatch refusal keeps
// the field as it is so "Save anyway" resends the same value with `confirm`.
//
// `compact` (the Zen Conversation widget): the same simple card, with
// "More options" opening the full card in the Thread pane.

import { useRef, useState, type JSX, type ReactNode } from 'react'
import { daemonErrorOf, daemonErrorText } from '@/lib/daemon-error'
import { SquareRadio } from '@/components/ui'
import { CHAMFER_CUT, chamferVars } from '@/components/Thread/chamfer'
import { cardExpiryLabel, type OverlaySecret, type SecretDelivery, type SecretScope, type ThreadAnswerPayload } from './overlayThread'

/** The daemon refusals that turn the card red and want a second click.
 *  `card_readable_delivery` only comes from a server older than 0.46.3
 *  (it doesn't know `disclosed`); the card answers it itself. */
export const SECRET_CARD_WARNINGS = new Set(['card_host_mismatch'])

/** Prompt cap (VS13): plain text, at most 200 characters. */
export const SECRET_PROMPT_MAX = 200

/** Known API hosts → provider names (mirrors the daemon's key presets). */
const PROVIDER_HOSTS: Record<string, string> = {
  'api.anthropic.com': 'Anthropic',
  'api.stripe.com': 'Stripe',
  'api.openai.com': 'OpenAI',
  'api.x.ai': 'xAI',
  'generativelanguage.googleapis.com': 'Google Gemini',
  'api.github.com': 'GitHub',
  'slack.com': 'Slack',
}

/** A site as people read it: `https://api.stripe.com:443` → `api.stripe.com`. */
export function siteLabel(origin: string): string {
  const m = /^([a-z][a-z0-9+.-]*):\/\/([^/]+?)(?::(\d+))?\/?$/i.exec(origin.trim())
  if (!m) return origin.trim()
  const [, scheme, host, port] = m
  const s = scheme.toLowerCase()
  if (s === 'https') return port && port !== '443' ? `${host}:${port}` : host
  if (s === 'http') return port && port !== '80' ? `http://${host}:${port}` : `http://${host}`
  return origin.trim()
}

/** The provider a site list points at, if it's a known API. */
export function providerForSites(origins: readonly string[]): string | null {
  for (const o of origins) {
    const host = siteLabel(o).replace(/^https?:\/\//, '').replace(/:\d+$/, '').toLowerCase()
    const p = PROVIDER_HOSTS[host]
    if (p) return p
  }
  return null
}

function normalScope(s: string | null | undefined): SecretScope {
  return s === 'server' || s === 'session' ? s : 'workspace'
}

function normalDelivery(d: string | null | undefined): SecretDelivery {
  return d === 'file' || d === 'env' ? d : 'fetch'
}

export function isReadableDelivery(d: SecretDelivery): boolean {
  return d !== 'fetch'
}

/** Kinds where "use a restricted key" makes sense. */
function keyLike(kind: string | null | undefined): boolean {
  return !kind || kind === 'api_key' || kind === 'oauth_token' || kind === 'generic'
}

/** "a, b and c". */
function listWords(items: readonly string[]): string {
  if (items.length <= 1) return items.join('')
  return `${items.slice(0, -1).join(', ')} and ${items[items.length - 1]}`
}

/** Where the secret is kept, in the plain sentence. */
export const SECRET_SCOPE_WORDS: Record<SecretScope, string> = {
  workspace: 'this workspace',
  server: 'all your agents',
  session: 'this chat only',
}

/** The plain sentence: what the agent chose, as the person reads it.
 *  Bold parts are `<strong>`; the text alone reads as one sentence. */
export function SecretPlan({
  agent,
  delivery,
  scope,
  sites,
  compact = false,
}: {
  agent: string
  delivery: SecretDelivery
  scope: SecretScope
  sites: readonly string[]
  compact?: boolean
}): JSX.Element {
  const b = (t: string): ReactNode => <strong className="font-semibold">{t}</strong>
  const where = SECRET_SCOPE_WORDS[scope]
  let body: ReactNode
  if (delivery === 'file' || delivery === 'env') {
    body = (
      <>
        {b(agent)} will get this as {delivery === 'file' ? 'a ' : 'an '}
        {b(delivery === 'file' ? 'file it can read' : 'environment variable it can read')}, for {b(where)}.
      </>
    )
  } else if (sites.length > 0) {
    body = (
      <>
        K2 will add this to calls to {b(listWords(sites.map(siteLabel)))}, for {b(where)}; {b(agent)} never sees it.
      </>
    )
  } else {
    body = (
      <>
        {agent} didn't say which site this is for, so K2 can't use it yet. {compact ? 'Use More options' : 'Open Advanced'} to
        add one.
      </>
    )
  }
  return (
    <div data-testid="thread-secret-plan" data-delivery={delivery} data-scope={scope} className="break-words">
      {body}
    </div>
  )
}

/** The host-mismatch warning in plain words. */
function mismatchText(hint: string, sites: readonly string[]): string {
  const provider = /looks like an? (.+?) key/i.exec(hint)?.[1]
  const to = sites.length > 0 ? listWords(sites.map(siteLabel)) : 'another site'
  if (provider) return `This looks like a ${provider} key, but it would be sent to ${to}. Save it only if that's right.`
  const h = hint.trim() || 'Check this before saving.'
  return h.charAt(0).toUpperCase() + h.slice(1)
}

interface SiteChip {
  /** What is sent: the agent's normalised origin, or what the person typed. */
  value: string
  byAgent: boolean
}

export interface SecretCardProps {
  secret: OverlaySecret
  /** Who asked (the Thread item's `from`, stamped by the daemon). */
  agent: string
  /** `cardAnswererLabel` of the card's `answeredBy` ("You", a login). */
  answeredBy?: string | null
  /** The asking agent's workspace (its Thread address before any `/`). */
  workspace?: string
  /** The server's label (the room's). */
  server?: string
  compact?: boolean
  /** POSTs the answer. Throws the daemon's refusal. */
  onSubmit: (answer: ThreadAnswerPayload & { secret: string }) => Promise<void>
  onDismiss: () => void
  /** Compact only: open the full card in the Thread pane. */
  onMoreOptions?: () => void
}

export function SecretCard({
  secret,
  answeredBy,
  agent: agentIn,
  workspace: workspaceIn,
  server,
  compact = false,
  onSubmit,
  onDismiss,
  onMoreOptions,
}: SecretCardProps): JSX.Element {
  const agent = agentIn.trim() || workspaceIn?.trim() || 'the agent'
  const workspace = workspaceIn?.trim() || agent
  const pending = secret.status === 'pending'
  // The vault request's 7 days; the daemon closes the card (`expired`).
  const expiry = pending ? cardExpiryLabel(secret.expiresAt, Math.floor(Date.now() / 1000)) : null

  const proposedScope = normalScope(secret.scope)
  const proposedSites = (secret.hosts ?? []).filter((h) => h.trim() !== '')
  const proposedDelivery = normalDelivery(secret.delivery)

  const fieldRef = useRef<HTMLInputElement>(null)
  const [hasValue, setHasValue] = useState(false)
  const [advanced, setAdvanced] = useState(false)
  const [scope, setScope] = useState<SecretScope>(proposedScope)
  const [sites, setSites] = useState<SiteChip[]>(() => proposedSites.map((value) => ({ value, byAgent: true })))
  const [delivery, setDelivery] = useState<SecretDelivery>(proposedDelivery)
  const [siteDraft, setSiteDraft] = useState('')
  const [warning, setWarning] = useState<{ code: string; hint: string } | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  const siteValues = sites.map((s) => s.value)
  const sitesChanged =
    siteValues.length !== proposedSites.length || siteValues.some((v, i) => v !== proposedSites[i])
  const needsSite = delivery === 'fetch' && sites.length === 0
  const provider = providerForSites(siteValues)
  const prompt = (secret.prompt ?? '').slice(0, SECRET_PROMPT_MAX)

  /** Any change to what is saved drops a warning: it was about the old choice. */
  const changed = (): void => {
    setWarning(null)
    setError(null)
  }

  const addSites = (): void => {
    const add = siteDraft
      .split(/[,\s]+/)
      .map((s) => s.trim())
      .filter((s) => s !== '' && !sites.some((c) => c.value === s))
    if (add.length === 0) return
    setSites((prev) => [...prev, ...add.map((value) => ({ value, byAgent: false }))])
    setSiteDraft('')
    changed()
  }

  const save = async (): Promise<void> => {
    const el = fieldRef.current
    if (!el || busy || needsSite) return
    const confirm = warning !== null
    setBusy(true)
    setError(null)
    // Read once from the field; nothing keeps it past this call.
    const value = el.value
    const send = (withConfirm: boolean): Promise<void> =>
      onSubmit({
        secret: value,
        ...(scope !== proposedScope ? { scope } : {}),
        ...(sitesChanged ? { hosts: siteValues } : {}),
        ...(delivery !== proposedDelivery ? { delivery } : {}),
        // The plain sentence above the field said what the agent can do with it.
        disclosed: true,
        ...(withConfirm ? { confirm: true } : {}),
      })
    try {
      try {
        await send(confirm)
      } catch (e) {
        // A server older than 0.46.3 doesn't know `disclosed` and asks for a
        // second click on a readable delivery. The person read the sentence,
        // and that server checks the host mismatch before this, so it passed:
        // resend once with `confirm`.
        if (confirm || daemonErrorOf(e)?.code !== 'card_readable_delivery') throw e
        await send(true)
      }
      el.value = ''
      setHasValue(false)
      setWarning(null)
    } catch (e) {
      const info = daemonErrorOf(e)
      if (info && SECRET_CARD_WARNINGS.has(info.code) && !confirm) {
        setWarning(info)
      } else {
        setWarning(null)
        setError(daemonErrorText(e))
      }
    } finally {
      setBusy(false)
    }
  }

  const decline = (): void => {
    if (fieldRef.current) fieldRef.current.value = ''
    setHasValue(false)
    onDismiss()
  }

  const muted = 'text-[var(--thread-card-muted,var(--color-text-muted))]'
  const text = 'text-[var(--thread-card-text,var(--color-text-primary))]'
  const border = 'border-[var(--thread-card-border,var(--color-border))]'
  const danger = 'text-[var(--color-danger,#e5484d)]'

  if (!pending) {
    const done =
      secret.status === 'set' ? 'saved' : secret.status === 'expired' ? 'request expired' : 'not given'
    return (
      <div data-testid="thread-secret-card" data-status={secret.status} className="mt-1">
        <div className={`text-[11px] ${muted}`} data-testid="thread-secret-set">
          <span className="font-mono">{secret.name}</span> {done}
          {secret.status === 'set' && answeredBy ? <span data-testid="thread-secret-answered-by"> by {answeredBy}</span> : null}
          {secret.status === 'set' && secret.ref ? (
            <>
              {' '}
              as <span className="font-mono select-all">{secret.ref}</span>
            </>
          ) : null}
        </div>
      </div>
    )
  }

  const scopeLabel: Record<SecretScope, string> = {
    workspace: `${workspace} only`,
    server: 'all my agents',
    session: 'this chat',
  }
  const deliveryLabel: Record<SecretDelivery, string> = {
    fetch: 'K2 adds it to calls',
    file: `a file ${agent} can read`,
    env: `an environment variable ${agent} can read`,
  }

  return (
    <div
      data-testid="thread-secret-card"
      data-compact={compact ? 'true' : 'false'}
      // K2's square Style (desktop Thread): chamfered.
      data-chamfer=""
      style={chamferVars({
        cut: CHAMFER_CUT.surface,
        fill: 'var(--thread-card-bg,var(--color-bg))',
        edge: warning ? 'var(--color-danger,#e5484d)' : 'var(--thread-card-border,var(--color-border))',
        border: 1,
      })}
      data-warning={warning?.code ?? ''}
      className={`mt-1 w-full box-border flex flex-col gap-2 rounded-[var(--thread-card-radius,0px)] border ${
        warning ? 'border-[var(--color-danger,#e5484d)]' : border
      } bg-[var(--thread-card-bg,var(--color-bg))] px-2.5 py-2 text-[11px] ${text}`}
    >
      {/* K2-drawn header: who asks, from where. */}
      <div data-testid="thread-secret-header">
        <div className="font-medium">
          {agent} asks for a secret: <span className="font-mono font-normal" data-testid="thread-secret-name">{secret.name}</span>
        </div>
        <div className={`text-[10px] ${muted}`}>
          {[workspace, server].filter(Boolean).join(' · ')}
          {expiry && (
            <span data-testid="thread-secret-expiry">
              {' · '}
              {expiry}
            </span>
          )}
        </div>
      </div>
      {prompt && (
        <div data-testid="thread-secret-prompt" className="whitespace-pre-wrap break-words">
          <span className={muted}>{agent} says: </span>
          {prompt}
        </div>
      )}
      <SecretPlan agent={agent} delivery={delivery} scope={scope} sites={siteValues} compact={compact} />
      <input
        ref={fieldRef}
        data-testid="thread-secret-field"
        type="password"
        autoComplete="off"
        spellCheck={false}
        aria-label={`Value for ${secret.name}`}
        placeholder="Paste the value"
        onChange={(e) => {
          setHasValue(e.currentTarget.value !== '')
          changed()
        }}
        className={`w-full box-border bg-transparent text-[12px] ${text} outline-none border ${border} px-2 py-1`}
      />
      <div className={`text-[10px] ${muted}`} data-testid="thread-secret-hint">
        Not right? Tell {agent} in the chat; it can cancel this and ask again.
      </div>

      {compact ? (
        onMoreOptions && (
          <button
            type="button"
            data-testid="thread-secret-more"
            onClick={onMoreOptions}
            className={`self-start underline ${muted} cursor-pointer`}
          >
            More options
          </button>
        )
      ) : (
        <>
          <button
            type="button"
            data-testid="thread-secret-advanced-toggle"
            aria-expanded={advanced}
            onClick={() => setAdvanced((v) => !v)}
            className={`self-start ${muted} cursor-pointer`}
          >
            {advanced ? '▾' : '▸'} Advanced
          </button>
          {advanced && (
            <div data-testid="thread-secret-advanced" className={`flex flex-col gap-2 border-t ${border} pt-2`}>
              <fieldset className="flex flex-col gap-1" data-testid="thread-secret-delivery">
                <legend className={`mb-0.5 ${muted}`}>How {agent} uses it</legend>
                {(['fetch', 'file', 'env'] as const).map((d) => (
                  <label key={d} className="flex items-start gap-1 cursor-pointer">
                    <SquareRadio
                      className="mt-0.5"
                      name={`secret-delivery-${secret.name}`}
                      value={d}
                      checked={delivery === d}
                      onChange={() => {
                        setDelivery(d)
                        changed()
                      }}
                    />
                    <span>
                      {deliveryLabel[d]}
                      {isReadableDelivery(d) && delivery === d && (
                        <span className={`block text-[10px] ${muted}`} data-testid="thread-secret-readable">
                          {agent} can see this value
                        </span>
                      )}
                    </span>
                  </label>
                ))}
              </fieldset>

              <div className="flex flex-col gap-1" data-testid="thread-secret-sites">
                <div className={muted}>Only send it to</div>
                <div className="flex flex-wrap gap-1">
                  {sites.map((s) => (
                    <span
                      key={s.value}
                      data-testid="thread-secret-site"
                      data-by-agent={s.byAgent ? 'true' : 'false'}
                      className={`inline-flex items-center gap-1 border ${border} px-1.5 py-0.5`}
                    >
                      <span className="font-mono">{siteLabel(s.value)}</span>
                      {s.byAgent && <span className={`text-[10px] ${muted}`}>{agent} suggests</span>}
                      <button
                        type="button"
                        aria-label={`Remove ${siteLabel(s.value)}`}
                        onClick={() => {
                          setSites((prev) => prev.filter((c) => c.value !== s.value))
                          changed()
                        }}
                        className={`${muted} cursor-pointer`}
                      >
                        ×
                      </button>
                    </span>
                  ))}
                </div>
                <div className="flex gap-1">
                  <input
                    data-testid="thread-secret-site-input"
                    value={siteDraft}
                    onChange={(e) => setSiteDraft(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === 'Enter') {
                        e.preventDefault()
                        addSites()
                      }
                    }}
                    placeholder="Add a site, like api.example.com"
                    className={`flex-1 min-w-0 bg-transparent ${text} outline-none border ${border} px-2 py-0.5`}
                  />
                  <button
                    type="button"
                    data-testid="thread-secret-site-add"
                    disabled={!siteDraft.trim()}
                    onClick={addSites}
                    className={`px-2 py-0.5 border ${border} disabled:opacity-40 cursor-pointer`}
                  >
                    Add
                  </button>
                </div>
              </div>

              <fieldset className="flex flex-col gap-1" data-testid="thread-secret-scope">
                <legend className={`mb-0.5 ${muted}`}>Keep it for</legend>
                <div className="flex flex-wrap gap-x-3 gap-y-1">
                  {(['workspace', 'server', 'session'] as const).map((s) => (
                    <label key={s} className="flex items-center gap-1 cursor-pointer">
                      <SquareRadio
                        name={`secret-scope-${secret.name}`}
                        value={s}
                        checked={scope === s}
                        onChange={() => {
                          setScope(s)
                          changed()
                        }}
                      />
                      {scopeLabel[s]}
                    </label>
                  ))}
                </div>
              </fieldset>

              {keyLike(secret.secretKind) && (
                <div className={`text-[10px] ${muted}`} data-testid="thread-secret-honest">
                  This key can still do anything {provider ?? 'its service'} allows it to. Use a restricted key if you
                  can.
                </div>
              )}
            </div>
          )}
        </>
      )}

      {warning && (
        <div className={`text-[11px] ${danger}`} role="alert" data-testid="thread-secret-warning">
          <strong>Check this first.</strong> {mismatchText(warning.hint, siteValues)} If {agent} set this up wrong,
          tell it in the chat instead.
        </div>
      )}
      {error && (
        <div className={`text-[11px] ${danger}`} role="alert" data-testid="thread-secret-error">
          {error}
        </div>
      )}
      <div className="flex justify-end gap-2">
        <button
          type="button"
          data-testid="thread-secret-dismiss"
          onClick={decline}
          disabled={busy}
          className={`px-2 py-1 text-[11px] ${muted} cursor-pointer`}
        >
          Not now
        </button>
        <button
          type="button"
          data-testid="thread-secret-submit"
          disabled={!hasValue || busy || needsSite}
          onClick={() => void save()}
          className={`px-2 py-1 text-[11px] border ${
            warning ? 'border-[var(--color-danger,#e5484d)]' : border
          } disabled:opacity-40 cursor-pointer disabled:cursor-not-allowed`}
        >
          {warning ? 'Save anyway' : 'Save'}
        </button>
      </div>
    </div>
  )
}
