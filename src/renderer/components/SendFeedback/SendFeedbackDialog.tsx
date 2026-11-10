// Send feedback to K2 (prd-k2-product-feedback-v1 PF5–PF7, PF25–PF27).
//
// Two tabs on the WINDOW's server (PF6): Send (category, text, an opt-in
// contact email, the "what's sent" line, the diagnostics consent and its
// preview) and Sent (this person's submissions with their state, live by
// `product_feedback_status`; the owner and admins see everyone's). The
// daemon keeps, sends, retries and scrubs; this dialog only shows what it
// says and sends the person's choices.
//
// Diagnostics: nothing is collected until the box is ticked (or the box
// starts ticked because this person asked K2 to remember it). The preview
// is exactly what is sent: each section can be left out and lines deleted
// (only deleted: the daemon refuses anything else), and send names the
// preview by its draft id, so the daemon never collects again.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { Button, Callout, Checkbox, DialogFrame, DialogScrim, Input, Segmented } from '@/components/ui'
import { primaryScope } from '@/kessel/server-scope'
import { isWebClient } from '@/lib/is-web'
import {
  ASK_AGENT_HINT,
  CATEGORIES,
  CONTACT_ADDRESS,
  DIAGNOSTICS_HINT,
  DIAGNOSTICS_LABEL,
  MAX_TEXT_BYTES,
  PRODUCT_FEEDBACK_FEATURE,
  REMEMBER_LABEL,
  SEND_FEEDBACK_LABEL,
  WHATS_SENT_LINE,
  cancelSent,
  collectDiagnostics,
  feedbackAsText,
  fetchFeedbackSettings,
  listSent,
  onlyDeletesLines,
  refusalMessage,
  retrySent,
  saveFeedbackSettings,
  sendFeedback,
  stateChip,
  utf8Bytes,
  type DiagnosticsDraft,
  type FeedbackCategory,
  type FeedbackSettings,
  type SentItem,
} from '@/lib/product-feedback'
import { onProductFeedbackStatus } from '@/stores/session-events'
import { useSendFeedbackStore, type SendFeedbackTab } from '@/stores/send-feedback'

async function appVersion(): Promise<string | null> {
  try {
    const { getVersion } = await import('@tauri-apps/api/app')
    return await getVersion()
  } catch {
    return null
  }
}

const TONE_CLASS: Record<'muted' | 'ok' | 'warn' | 'error', string> = {
  muted: 'text-[var(--color-text-muted)] border-[var(--color-border)]',
  ok: 'text-[var(--color-status-ok-soft)] border-[color-mix(in_oklab,var(--color-status-ok)_40%,transparent)]',
  warn: 'text-[var(--color-status-warn-amber-soft)] border-[color-mix(in_oklab,var(--color-status-warn)_40%,transparent)]',
  error: 'text-[var(--color-status-error-soft)] border-[color-mix(in_oklab,var(--color-status-error)_40%,transparent)]',
}

const TEXTAREA_CLASS =
  'w-full resize-y bg-[var(--color-bg)] text-xs text-[var(--color-text-primary)] px-2 py-1.5 border border-[var(--color-border)] rounded-[var(--radius-field)] outline-none focus:border-[var(--color-accent)]'

export default function SendFeedbackDialog(): React.JSX.Element {
  const scope = useMemo(() => primaryScope(), [])
  const tab = useSendFeedbackStore((s) => s.tab)
  const setTab = useSendFeedbackStore((s) => s.setTab)
  const close = useSendFeedbackStore((s) => s.close)
  const openedFrom = useSendFeedbackStore((s) => s.openedFrom)
  const supported = scope.serverSupports(PRODUCT_FEEDBACK_FEATURE)
  const web = isWebClient()

  const [settings, setSettings] = useState<FeedbackSettings | null>(null)
  const [settingsError, setSettingsError] = useState<string | null>(null)
  const [version, setVersion] = useState<string | null>(null)

  // The form.
  const [category, setCategory] = useState<FeedbackCategory>('bug')
  const [text, setText] = useState('')
  const [contactOn, setContactOn] = useState(false)
  const [email, setEmail] = useState('')
  const [diagOn, setDiagOn] = useState(false)
  const [draft, setDraft] = useState<DiagnosticsDraft | null>(null)
  const [collecting, setCollecting] = useState(false)
  const [diagError, setDiagError] = useState<string | null>(null)
  const [previewOpen, setPreviewOpen] = useState(false)
  const [dropped, setDropped] = useState<Set<string>>(new Set())
  const [edited, setEdited] = useState<Record<string, string>>({})
  const [sending, setSending] = useState(false)
  const [sendError, setSendError] = useState<string | null>(null)
  const [sentNote, setSentNote] = useState<string | null>(null)
  const [copied, setCopied] = useState(false)

  // The Sent tab.
  const [items, setItems] = useState<SentItem[] | null>(null)
  const [allRows, setAllRows] = useState(false)
  const [listError, setListError] = useState<string | null>(null)

  const sendingRef = useRef(false)

  useEffect(() => {
    void appVersion().then(setVersion)
  }, [])

  const loadSettings = useCallback(async () => {
    if (!supported) return
    try {
      const s = await fetchFeedbackSettings(scope)
      setSettings(s)
      setSettingsError(null)
      // PF27: start ticked only when this person asked to be remembered.
      if (s.rememberDiagnostics === true) setDiagOn(true)
    } catch (e) {
      setSettingsError(refusalMessage(e))
    }
  }, [scope, supported])

  const loadSent = useCallback(async () => {
    if (!supported) return
    try {
      const r = await listSent(scope)
      setItems(r.items)
      setAllRows(r.all)
      setListError(null)
    } catch (e) {
      setListError(refusalMessage(e))
    }
  }, [scope, supported])

  useEffect(() => {
    void loadSettings()
    void loadSent()
    if (!supported) return
    const off = onProductFeedbackStatus(scope, () => {
      void loadSent()
    })
    return off
  }, [scope, supported, loadSettings, loadSent])

  // Collect once the box is ticked (PF25): the daemon gathers and scrubs.
  useEffect(() => {
    // A failed collection waits for the person to tick the box again.
    if (!diagOn || draft || collecting || diagError || !supported) return
    setCollecting(true)
    setDiagError(null)
    collectDiagnostics(scope, {
      client: web ? 'web' : 'desktop',
      page: openedFrom,
      k2Version: version,
      includeClientFaults: !web && !scope.isRemote,
    })
      .then((d) => setDraft(d))
      .catch((e) => setDiagError(refusalMessage(e)))
      .finally(() => setCollecting(false))
  }, [diagOn, draft, collecting, diagError, supported, scope, web, openedFrom, version])

  useEffect(() => {
    const onKey = (e: KeyboardEvent): void => {
      if (e.key !== 'Escape') return
      e.preventDefault()
      e.stopPropagation()
      if (!sendingRef.current) close()
    }
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  }, [close])

  const bytes = utf8Bytes(text)
  const tooLong = bytes > MAX_TEXT_BYTES
  const canSend = supported && settings?.canSend === true
  const editErrors = useMemo(() => {
    if (!draft) return {}
    const out: Record<string, boolean> = {}
    for (const s of draft.sections) {
      const e = edited[s.id]
      if (e !== undefined && !onlyDeletesLines(s.text, e)) out[s.id] = true
    }
    return out
  }, [draft, edited])
  const editBlocked = Object.keys(editErrors).length > 0
  const emailOk = !contactOn || /^[^\s@,;]+@[^\s@,;]+\.[^\s@,;]+$/.test(email.trim())
  const readyToSend =
    canSend && !sending && text.trim() !== '' && !tooLong && emailOk && (!diagOn || (draft !== null && !editBlocked))

  const unavailableLine = !supported
    ? `This server's K2 is older than Send feedback. Copy your text and email ${CONTACT_ADDRESS}.`
    : settings && !settings.canSend
      ? settings.message ?? `This server can't send feedback now. Copy your text and email ${CONTACT_ADDRESS}.`
      : null
  const serviceLine =
    settings?.canSend && (settings.service.state === 'unreachable' || settings.service.state === 'paused' || settings.service.state === 'limited')
      ? settings.service.message
      : null

  const toggleRemember = async (on: boolean): Promise<void> => {
    try {
      setSettings(await saveFeedbackSettings(scope, { rememberDiagnostics: on }))
    } catch (e) {
      setDiagError(refusalMessage(e))
    }
  }

  const copyAsText = async (): Promise<void> => {
    try {
      await navigator.clipboard.writeText(feedbackAsText({ category, text, k2Version: version }))
      setCopied(true)
    } catch {
      setSendError(`Couldn't copy. Select your text and copy it, then email ${CONTACT_ADDRESS}.`)
    }
  }

  const submit = async (): Promise<void> => {
    if (!readyToSend) return
    sendingRef.current = true
    setSending(true)
    setSendError(null)
    try {
      const drop = draft ? draft.sections.filter((s) => dropped.has(s.id)).map((s) => s.id) : []
      const keptEdits: Record<string, string> = {}
      for (const [id, t] of Object.entries(edited)) if (!dropped.has(id)) keptEdits[id] = t
      await sendFeedback(scope, {
        category,
        text,
        contactEmail: contactOn ? email.trim() : null,
        client: web ? 'web' : 'desktop',
        k2Version: version,
        locale: typeof navigator !== 'undefined' ? navigator.language : null,
        diagnostics: diagOn && draft ? { draftId: draft.draftId, drop, edited: keptEdits } : null,
      })
      setText('')
      setDraft(null)
      setDropped(new Set())
      setEdited({})
      setPreviewOpen(false)
      setDiagOn(settings?.rememberDiagnostics === true)
      setSentNote('Thank you. Your feedback is saved on this server and sends to Alakazam Labs as soon as it can.')
      void loadSent()
      setTab('sent')
    } catch (e) {
      setSendError(refusalMessage(e))
    } finally {
      sendingRef.current = false
      setSending(false)
    }
  }

  const dismiss = (): void => {
    if (!sendingRef.current) close()
  }

  return (
    <>
      <DialogScrim
        data-testid="send-feedback-scrim"
        onMouseDown={(e) => {
          e.stopPropagation()
          dismiss()
        }}
      />
      <DialogFrame
        data-testid="send-feedback-dialog"
        role="dialog"
        aria-label={SEND_FEEDBACK_LABEL}
        onMouseDown={(e) => e.stopPropagation()}
        style={{
          width: 560,
          maxWidth: 'calc(100vw - 48px)',
          maxHeight: 'calc(100vh - 96px)',
          display: 'flex',
          flexDirection: 'column',
        }}
      >
        <div className="flex items-center gap-3 px-4 py-3 border-b border-[var(--color-border)]">
          <div className="text-sm font-semibold text-[var(--color-text-primary)] truncate">{SEND_FEEDBACK_LABEL}</div>
          <Segmented<SendFeedbackTab>
            aria-label="Send or Sent"
            data-testid="send-feedback-tabs"
            value={tab}
            onChange={setTab}
            options={[
              { value: 'send', label: 'Send' },
              { value: 'sent', label: 'Sent' },
            ]}
          />
          <button
            type="button"
            onClick={dismiss}
            className="ml-auto flex h-6 w-6 items-center justify-center text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)] transition-colors flex-shrink-0"
            title="Close (Esc)"
            aria-label="Close"
          >
            <svg width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round">
              <line x1="2" y1="2" x2="10" y2="10" />
              <line x1="10" y1="2" x2="2" y2="10" />
            </svg>
          </button>
        </div>

        {tab === 'send' ? (
          <div className="px-4 py-3 flex flex-col gap-3 min-h-0 overflow-y-auto" data-testid="send-feedback-form">
            {settings?.agents === true ? (
              <p className="type-hint text-[var(--color-text-muted)]" data-testid="send-feedback-ask-agent">
                {ASK_AGENT_HINT}
              </p>
            ) : null}
            {unavailableLine ? (
              <Callout tone="warn" data-testid="send-feedback-unavailable">
                <div>{unavailableLine}</div>
                <div className="mt-2 flex items-center gap-2">
                  <Button size="sm" onClick={() => void copyAsText()} disabled={text.trim() === ''} data-testid="send-feedback-copy">
                    {copied ? 'Copied' : 'Copy as text'}
                  </Button>
                  <span className="select-all font-mono">{CONTACT_ADDRESS}</span>
                </div>
              </Callout>
            ) : null}
            {settingsError ? (
              <Callout tone="error" role="alert">
                {settingsError}
              </Callout>
            ) : null}
            {sentNote ? (
              <Callout tone="ok" data-testid="send-feedback-thanks">
                {sentNote}
              </Callout>
            ) : null}

            <Segmented<FeedbackCategory>
              aria-label="Category"
              data-testid="send-feedback-category"
              className="self-start"
              value={category}
              onChange={setCategory}
              options={CATEGORIES.map((c) => ({ value: c.value, label: c.label }))}
            />

            <div className="flex flex-col gap-1">
              <textarea
                autoFocus
                data-testid="send-feedback-text"
                aria-label="Your feedback"
                value={text}
                disabled={sending}
                onChange={(e) => {
                  setText(e.target.value)
                  setSentNote(null)
                  setCopied(false)
                }}
                placeholder="What happened, or what would make K2 better?"
                className={`${TEXTAREA_CLASS} min-h-[120px]`}
              />
              {bytes > MAX_TEXT_BYTES * 0.9 ? (
                <div className={`type-hint ${tooLong ? 'text-[var(--color-status-error-soft)]' : 'text-[var(--color-text-muted)]'}`}>
                  {bytes.toLocaleString()} of {MAX_TEXT_BYTES.toLocaleString()} bytes
                </div>
              ) : null}
            </div>

            <p className="type-hint text-[var(--color-text-muted)]" data-testid="send-feedback-whats-sent">
              {WHATS_SENT_LINE}
            </p>

            <div className="flex flex-col gap-1.5">
              <label className="flex items-center gap-2 text-[12px] text-[var(--color-text-secondary)] cursor-pointer select-none">
                <Checkbox
                  data-testid="send-feedback-contact"
                  checked={contactOn}
                  disabled={sending}
                  onChange={(e) => setContactOn(e.target.checked)}
                />
                Alakazam may email me about this
              </label>
              {contactOn ? (
                <div className="pl-[calc(var(--control-check)+0.5rem)]">
                  <Input
                    data-testid="send-feedback-email"
                    type="email"
                    value={email}
                    onChange={(e) => setEmail(e.target.value)}
                    placeholder="you@example.com"
                    aria-invalid={!emailOk}
                  />
                </div>
              ) : null}
            </div>

            <div className="flex flex-col gap-1.5">
              <label className="flex items-start gap-2 text-[12px] text-[var(--color-text-secondary)] cursor-pointer select-none">
                <Checkbox
                  data-testid="send-feedback-diagnostics"
                  checked={diagOn}
                  disabled={sending || !supported}
                  onChange={(e) => {
                    setDiagOn(e.target.checked)
                    if (!e.target.checked) {
                      setPreviewOpen(false)
                      setDiagError(null)
                    }
                  }}
                />
                <span className="flex flex-col gap-0.5">
                  <span className="text-[var(--color-text-primary)]">{DIAGNOSTICS_LABEL}</span>
                  <span className="type-hint text-[var(--color-text-muted)]">{DIAGNOSTICS_HINT}</span>
                </span>
              </label>
              {diagOn ? (
                <div className="flex flex-col gap-1.5 pl-[calc(var(--control-check)+0.5rem)]">
                  <div className="flex items-center gap-2">
                    <Button
                      size="sm"
                      variant="quiet"
                      data-testid="send-feedback-preview-toggle"
                      disabled={!draft}
                      onClick={() => setPreviewOpen((o) => !o)}
                    >
                      {collecting ? 'Collecting…' : previewOpen ? 'Hide what’s included' : 'Read what’s included'}
                    </Button>
                    {draft ? (
                      <span className="type-hint text-[var(--color-text-muted)]">
                        {draft.sections.length - dropped.size} of {draft.sections.length} sections
                      </span>
                    ) : null}
                  </div>
                  {settings?.rememberDiagnostics !== null && settings ? (
                    <label className="flex items-center gap-2 text-[12px] text-[var(--color-text-secondary)] cursor-pointer select-none">
                      <Checkbox
                        data-testid="send-feedback-remember"
                        checked={settings.rememberDiagnostics === true}
                        onChange={(e) => void toggleRemember(e.target.checked)}
                      />
                      {REMEMBER_LABEL}
                    </label>
                  ) : null}
                  {diagError ? (
                    <Callout tone="error" role="alert">
                      {diagError}
                    </Callout>
                  ) : null}
                </div>
              ) : null}
            </div>

            {diagOn && previewOpen && draft ? (
              <div className="flex flex-col gap-2" data-testid="send-feedback-preview">
                {draft.sections.map((s) => {
                  const out = dropped.has(s.id)
                  return (
                    <div key={s.id} className="flex flex-col gap-1" data-section={s.id}>
                      <div className="flex items-center gap-2">
                        <span className={`text-[12px] font-medium ${out ? 'text-[var(--color-text-muted)] line-through' : 'text-[var(--color-text-primary)]'}`}>
                          {s.title}
                        </span>
                        <Button
                          size="sm"
                          variant="ghost"
                          className="ml-auto"
                          data-testid={`send-feedback-leave-out-${s.id}`}
                          onClick={() =>
                            setDropped((d) => {
                              const n = new Set(d)
                              if (n.has(s.id)) n.delete(s.id)
                              else n.add(s.id)
                              return n
                            })
                          }
                        >
                          {out ? 'Include' : 'Leave out'}
                        </Button>
                      </div>
                      {out ? null : (
                        <>
                          <textarea
                            aria-label={`${s.title}: delete any lines you don't want to send`}
                            spellCheck={false}
                            value={edited[s.id] ?? s.text}
                            onChange={(e) => setEdited((m) => ({ ...m, [s.id]: e.target.value }))}
                            className={`${TEXTAREA_CLASS} font-mono min-h-[64px] max-h-[220px]`}
                          />
                          {editErrors[s.id] ? (
                            <div className="type-hint text-[var(--color-status-error-soft)]" role="alert">
                              You can only delete lines here, not add or change them.
                            </div>
                          ) : null}
                        </>
                      )}
                    </div>
                  )
                })}
              </div>
            ) : null}

            {serviceLine ? (
              <Callout tone="info" data-testid="send-feedback-service">
                {serviceLine}
              </Callout>
            ) : null}
            {sendError ? (
              <Callout tone="error" role="alert" data-testid="send-feedback-error">
                {sendError}
              </Callout>
            ) : null}
          </div>
        ) : (
          <SentList
            items={items}
            all={allRows}
            error={listError}
            supported={supported}
            onRetry={async (id) => {
              try {
                await retrySent(scope, id)
              } catch (e) {
                setListError(refusalMessage(e))
              }
              void loadSent()
            }}
            onCancel={async (id) => {
              try {
                await cancelSent(scope, id)
              } catch (e) {
                setListError(refusalMessage(e))
              }
              void loadSent()
            }}
          />
        )}

        <div className="px-4 py-3 border-t border-[var(--color-border)] flex items-center gap-2">
          <span className="type-hint text-[var(--color-text-muted)] mr-auto truncate" data-testid="send-feedback-through">
            Sent through {scope.label}
          </span>
          {tab === 'send' ? (
            <>
              <Button size="md" variant="ghost" onClick={dismiss} disabled={sending}>
                Cancel
              </Button>
              <Button size="md" variant="accent" onClick={() => void submit()} disabled={!readyToSend} data-testid="send-feedback-send">
                {sending ? 'Sending…' : 'Send'}
              </Button>
            </>
          ) : (
            <Button size="md" variant="ghost" onClick={dismiss}>
              Close
            </Button>
          )}
        </div>
      </DialogFrame>
    </>
  )
}

function SentList({
  items,
  all,
  error,
  supported,
  onRetry,
  onCancel,
}: {
  items: SentItem[] | null
  all: boolean
  error: string | null
  supported: boolean
  onRetry: (id: string) => Promise<void>
  onCancel: (id: string) => Promise<void>
}): React.JSX.Element {
  return (
    <div className="px-4 py-3 flex flex-col gap-2 min-h-[160px] overflow-y-auto" data-testid="send-feedback-sent">
      {all ? (
        <p className="type-hint text-[var(--color-text-muted)]">Everyone's on this server, agents' included.</p>
      ) : null}
      {error ? (
        <Callout tone="error" role="alert">
          {error}
        </Callout>
      ) : null}
      {!supported ? (
        <p className="text-[12px] text-[var(--color-text-muted)]">This server's K2 is older than Send feedback.</p>
      ) : items === null ? (
        <p className="text-[12px] text-[var(--color-text-muted)]">Loading…</p>
      ) : items.length === 0 ? (
        <p className="text-[12px] text-[var(--color-text-muted)]">Nothing sent from here yet.</p>
      ) : (
        items.map((it) => {
          const chip = stateChip(it.state)
          return (
            <div key={it.id} data-testid="send-feedback-row" data-state={it.state} className="flex flex-col gap-1 border-b border-[var(--color-border)] pb-2">
              <div className="flex items-center gap-2 text-[12px]">
                <span className={`border rounded-[var(--radius-field)] px-1.5 py-px text-[11px] ${TONE_CLASS[chip.tone]}`}>{chip.label}</span>
                <span className="text-[var(--color-text-secondary)] capitalize">{it.category}</span>
                <span className="text-[var(--color-text-muted)]">{new Date(it.createdAt).toLocaleString()}</span>
                {!it.mine ? <span className="text-[var(--color-text-muted)] truncate">· {it.sender}</span> : null}
                {it.hasDiagnostics ? <span className="text-[var(--color-text-muted)]">· K2 details</span> : null}
                <span className="ml-auto flex gap-1">
                  {it.state === 'failed' ? (
                    <Button size="sm" onClick={() => void onRetry(it.id)}>
                      Retry
                    </Button>
                  ) : null}
                  {it.state === 'queued' ? (
                    <Button size="sm" variant="ghost" onClick={() => void onCancel(it.id)}>
                      Cancel
                    </Button>
                  ) : null}
                </span>
              </div>
              <div className="text-[12px] text-[var(--color-text-primary)] whitespace-pre-wrap break-words line-clamp-3">{it.text}</div>
              {it.errorMessage && it.state !== 'sent' && it.state !== 'cancelled' ? (
                <div className="type-hint text-[var(--color-text-muted)]">
                  {it.state === 'refused' ? `Refused: ${it.errorMessage}` : it.errorMessage}
                </div>
              ) : null}
            </div>
          )
        })
      )}
    </div>
  )
}
