// prd-zen-gardens-v1 G25 and Rosson 2026-10-04 — "+ New Garden" in the
// Garden switcher's menu; prd-zen-user-widgets-v2 UWB22, UWB23 and R5 as
// changed (2026-10-08) — the Garden catalog.
//
// A menu item that turns into a name field. Enter (a usable, unused name)
// moves on to a clear choice of how the Garden starts:
//   - **Start with the default**: Garden 1's layout (the texting page:
//     a Home's agents beside the conversation), template `texting`;
//   - **Start empty and ask my agent**: an empty Garden (`blank`) whose
//     Ask my agent chooser opens as soon as it shows;
//   - **Ready-made Gardens**: the catalog (`GET /cli/zen/templates`,
//     section `catalog`; the Diary first). It is data: a new catalog Garden
//     shows here with no code change. One that runs a widget (`needsGrant`)
//     asks which agents it may see, and whether it may send, in the same
//     click (UWB22): the daemon creates the Garden and its grant together.
//     Nothing is ever added to someone's Gardens without this click.
// "Start with the default" is the selected choice (focused, so Enter takes
// it): it gives a working page straight away, and an empty Garden is one
// click from it anyway (Start with the default on the empty page). Each
// creates the Garden (`gardens.create`, cap `gardens:manage`), switches
// to it and closes the menu. Esc goes back a step; a refused name goes
// back to the field with the reason.

import { useEffect, useRef, useState } from 'react'
import type { ZenGardenTemplate, ZenWidgetBridge } from '@/lib/zen/zen-bridge'
import type { ZenScope, ZenTemplateInfo } from '@/lib/zen/zen-custom-types'
import { loadZenTemplates, useZenCatalogBadges, useZenTemplatesStore, zenTemplateSections } from '@/lib/zen/zen-templates'
import { zenScopeKind, zenScopeRows } from '@/lib/zen/zen-custom-scope'
import { ZenCapList, ZenScopeCount, ZenScopePicker, ZenSendingChoice, zenDefaultScope } from './ZenGrantDialog'

/** The two starts as K2 has always offered them (the fallback list). */
export const ZEN_NEW_GARDEN_STARTS: ReadonlyArray<{
  start: ZenGardenTemplate
  ask: boolean
  title: string
  detail: string
}> = [
  {
    start: 'texting',
    ask: false,
    title: 'Start with the default',
    detail: 'Garden 1’s layout: your agents beside a conversation.',
  },
  {
    start: 'blank',
    ask: true,
    title: 'Start empty and ask my agent',
    detail: 'An empty page. Your agent builds it with you.',
  },
]

/** Why `name` can't be a new Garden's name, or null. Same rules as the
 *  daemon (1 to 60 characters, unique case aside), checked before the
 *  choice so a clash never waits for a second step. */
export function zenNewGardenNameProblem(name: string, taken: readonly string[]): string | null {
  // eslint-disable-next-line no-control-regex
  const clean = name.replace(/[\u0000-\u001f\u007f]/g, '').trim()
  if (clean.length === 0 || [...clean].length > 60) return 'A Garden name is 1 to 60 characters.'
  const lower = clean.toLocaleLowerCase()
  if (taken.some((t) => t.toLocaleLowerCase() === lower)) return `You already have a Garden called “${clean}”.`
  return null
}

const fieldStyle: React.CSSProperties = {
  height: 30,
  padding: '0 10px',
  color: 'var(--zen-text)',
  background: 'var(--zen-surface)',
  border: '1px solid var(--zen-border)',
  borderRadius: 'calc(var(--zen-radius) - 4px)',
  font: 'inherit',
  outline: 'none',
}

function choiceStyle(selected: boolean): React.CSSProperties {
  return {
    gap: 2,
    padding: '7px 10px',
    borderRadius: 'calc(var(--zen-radius) - 4px)',
    color: 'var(--zen-text)',
    background: selected ? 'var(--zen-surface)' : 'transparent',
    border: `1px solid ${selected ? 'var(--zen-accent)' : 'var(--zen-border)'}`,
  }
}

/** The starts K2 shows: the daemon's `start` rows in its order (`blank`
 *  opens Ask my agent), else the built-in two. */
function startChoices(list: readonly ZenTemplateInfo[]): Array<{ start: string; ask: boolean; title: string; detail: string }> {
  const starts = zenTemplateSections(list).starts
  if (starts.length === 0) return [...ZEN_NEW_GARDEN_STARTS]
  return starts.map((t) => ({ start: t.short, ask: t.short === 'blank', title: t.label, detail: t.description }))
}

export function ZenNewGarden({ bridge, onDone }: { bridge: ZenWidgetBridge; onDone(): void }): React.JSX.Element {
  const [step, setStep] = useState<'closed' | 'name' | 'start' | 'grant'>('closed')
  const [name, setName] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [entry, setEntry] = useState<ZenTemplateInfo | null>(null)
  const [scope, setScope] = useState<ZenScope | null>(null)
  const [sending, setSending] = useState(true)
  const [confirmed, setConfirmed] = useState(false)
  const templates = useZenTemplatesStore((s) => s.templates)
  // A "New" (or other) badge per catalog entry, set by whoever knows.
  const badges = useZenCatalogBadges((s) => s.badges)
  const inputRef = useRef<HTMLInputElement | null>(null)
  const firstChoiceRef = useRef<HTMLButtonElement | null>(null)

  useEffect(() => {
    if (step === 'name') inputRef.current?.focus()
    if (step === 'start') firstChoiceRef.current?.focus()
  }, [step])

  const reset = (): void => {
    setStep('closed')
    setName('')
    setError(null)
    setEntry(null)
  }

  const next = (): void => {
    const problem = zenNewGardenNameProblem(
      name,
      bridge.gardens.list().map((g) => g.name),
    )
    setError(problem)
    if (!problem) setStep('start')
  }

  const create = (start: ZenGardenTemplate, ask: boolean, grant?: { scope: ZenScope; sending: boolean }): void => {
    if (busy) return
    setBusy(true)
    setError(null)
    void Promise.resolve()
      .then(() => bridge.gardens.create(name, start, grant ? { ask, grant } : { ask }))
      .then(() => {
        setBusy(false)
        reset()
        onDone()
      })
      .catch((err: unknown) => {
        setBusy(false)
        setError(err instanceof Error ? err.message : String(err))
        // A refused name goes back to the field; anything else stays here.
        if (err instanceof Error && 'code' in err && (err.code === 'garden_exists' || err.code === 'bad_name')) {
          setStep('name')
        }
      })
  }

  const pickCatalog = (t: ZenTemplateInfo): void => {
    if (!t.needsGrant) {
      create(t.short, false)
      return
    }
    setEntry(t)
    setScope(zenDefaultScope())
    setSending(true)
    setConfirmed(false)
    setError(null)
    setStep('grant')
  }

  // Plain keys stay in the menu; ⌘ / Ctrl chords reach the app.
  const keepKeys = (e: React.KeyboardEvent): void => {
    if (!e.metaKey && !e.ctrlKey) e.stopPropagation()
  }

  if (step === 'closed') {
    return (
      <button
        type="button"
        role="menuitem"
        data-zen-new-garden=""
        onClick={() => {
          setStep('name')
          // The catalog may have grown since the last time (UWB23).
          void loadZenTemplates()
        }}
        className="flex w-full items-center gap-3 text-left cursor-pointer"
        style={{ minHeight: 32, padding: '6px 10px', borderRadius: 'calc(var(--zen-radius) - 4px)', color: 'var(--zen-text)' }}
      >
        <span aria-hidden style={{ width: 8, textAlign: 'center', color: 'var(--zen-text-muted)' }}>
          +
        </span>
        <span>New Garden</span>
      </button>
    )
  }

  const errorLine = error && (
    <span data-zen-new-garden-error="" style={{ fontSize: '0.8em', color: 'var(--zen-danger)', padding: '0 4px' }}>
      {error}
    </span>
  )

  if (step === 'name') {
    return (
      <div className="flex flex-col" style={{ padding: '4px 6px', gap: 4 }}>
        <input
          ref={inputRef}
          type="text"
          value={name}
          maxLength={60}
          aria-label="New Garden name"
          placeholder="Garden name"
          data-zen-new-garden-name=""
          onChange={(e) => {
            setName(e.target.value)
            if (error) setError(null)
          }}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && !e.nativeEvent.isComposing) {
              e.preventDefault()
              next()
            } else if (e.key === 'Escape') {
              e.preventDefault()
              e.stopPropagation()
              reset()
            }
            keepKeys(e)
          }}
          style={fieldStyle}
        />
        {errorLine}
      </div>
    )
  }

  if (step === 'grant' && entry?.needsGrant) {
    const caps = entry.needsGrant.caps
    const posts = caps.includes('thread:post')
    const everyServer = scope !== null && zenScopeKind(scope) === 'allServers'
    const empty = scope === null || zenScopeRows(scope).rows.length === 0
    const blocked = busy || empty || (posts && sending && everyServer && !confirmed)
    return (
      <div
        role="group"
        aria-label={`Which agents “${name.trim()}” may see`}
        data-zen-new-garden-grant={entry.short}
        className="flex flex-col"
        style={{ padding: '4px 6px', gap: 8, maxWidth: 340 }}
        onKeyDown={(e) => {
          if (e.key === 'Escape') {
            e.preventDefault()
            e.stopPropagation()
            setError(null)
            setStep('start')
          }
          keepKeys(e)
        }}
      >
        <span className="truncate" style={{ padding: '2px 4px', fontWeight: 600, color: 'var(--zen-text)' }}>
          “{name.trim()}” · {entry.label}
        </span>
        <span style={{ fontSize: '0.85em', color: 'var(--zen-text-muted)' }}>Its page can:</span>
        <ZenCapList caps={caps} scope={scope} />
        <ZenScopePicker ask={{}} value={scope} onChange={setScope} />
        <ZenScopeCount scope={scope} />
        {posts && (
          <ZenSendingChoice
            sending={sending}
            onSending={setSending}
            everyServer={everyServer}
            confirmed={confirmed}
            onConfirmed={setConfirmed}
          />
        )}
        <div className="flex justify-end" style={{ gap: 6 }}>
          <button
            type="button"
            data-zen-new-garden-back=""
            disabled={busy}
            onClick={() => setStep('start')}
            className="cursor-pointer"
            style={{ ...choiceStyle(false), padding: '4px 12px' }}
          >
            Back
          </button>
          <button
            type="button"
            data-zen-new-garden-create=""
            disabled={blocked}
            onClick={() => scope && create(entry.short, false, { scope, sending: posts ? sending : false })}
            className="cursor-pointer disabled:cursor-default"
            style={{ ...choiceStyle(true), padding: '4px 12px', fontWeight: 600, opacity: blocked ? 0.5 : 1 }}
          >
            {busy ? 'Creating…' : 'Create'}
          </button>
        </div>
        {errorLine}
      </div>
    )
  }

  const starts = startChoices(templates)
  const catalog = zenTemplateSections(templates).catalog
  return (
    <div
      role="group"
      aria-label={`How “${name.trim()}” starts`}
      data-zen-new-garden-start=""
      className="flex flex-col"
      style={{ padding: '4px 6px', gap: 4, maxWidth: 300 }}
      onKeyDown={(e) => {
        if (e.key === 'Escape') {
          e.preventDefault()
          e.stopPropagation()
          setError(null)
          setStep('name')
        } else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
          e.preventDefault()
          const items = Array.from(e.currentTarget.querySelectorAll<HTMLButtonElement>('[data-zen-new-garden-choice]'))
          const at = items.indexOf(document.activeElement as HTMLButtonElement)
          const to = e.key === 'ArrowDown' ? Math.min(items.length - 1, at + 1) : Math.max(0, at - 1)
          items[to]?.focus()
        }
        keepKeys(e)
      }}
    >
      <span
        data-zen-new-garden-named=""
        className="truncate"
        style={{ padding: '2px 4px', fontWeight: 600, color: 'var(--zen-text)' }}
      >
        “{name.trim()}”
      </span>
      {starts.map((c, i) => (
        <button
          key={c.start}
          ref={i === 0 ? firstChoiceRef : undefined}
          type="button"
          disabled={busy}
          data-zen-new-garden-choice={c.start}
          data-zen-new-garden-selected={i === 0 ? '' : undefined}
          onClick={() => create(c.start, c.ask)}
          className="flex w-full flex-col items-start text-left cursor-pointer disabled:cursor-default"
          style={choiceStyle(i === 0)}
        >
          <span style={{ fontWeight: 600 }}>{c.title}</span>
          <span style={{ fontSize: '0.8em', color: 'var(--zen-text-muted)' }}>{c.detail}</span>
        </button>
      ))}
      {catalog.length > 0 && (
        <>
          <span
            data-zen-new-garden-catalog-title=""
            style={{ padding: '6px 4px 0', fontSize: '0.75em', color: 'var(--zen-text-muted)', textTransform: 'uppercase', letterSpacing: '0.04em' }}
          >
            Ready-made Gardens
          </span>
          {catalog.map((t) => (
            <button
              key={t.short}
              type="button"
              disabled={busy}
              data-zen-new-garden-choice={t.short}
              data-zen-new-garden-catalog=""
              onClick={() => pickCatalog(t)}
              className="flex w-full flex-col items-start text-left cursor-pointer disabled:cursor-default"
              style={choiceStyle(false)}
            >
              <span className="flex items-center" style={{ fontWeight: 600, gap: 6 }}>
                {t.label}
                {badges[t.short] && (
                  <span
                    data-zen-catalog-badge={t.short}
                    style={{
                      fontSize: '0.65em',
                      padding: '0 5px',
                      border: '1px solid var(--zen-accent)',
                      borderRadius: 999,
                      color: 'var(--zen-accent)',
                      textTransform: 'uppercase',
                      letterSpacing: '0.04em',
                    }}
                  >
                    {badges[t.short]}
                  </span>
                )}
              </span>
              <span style={{ fontSize: '0.8em', color: 'var(--zen-text-muted)' }}>{t.description}</span>
            </button>
          ))}
        </>
      )}
      {errorLine}
    </div>
  )
}
