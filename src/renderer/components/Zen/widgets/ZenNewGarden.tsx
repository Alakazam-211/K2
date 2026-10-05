// prd-zen-gardens-v1 G25 and Rosson 2026-10-04 — "+ New Garden" in the
// Garden switcher's menu.
//
// A menu item that turns into a name field. Enter (a usable, unused name)
// moves on to a clear choice of how the Garden starts:
//   - **Start with the default**: Garden 1's layout (the texting page:
//     a Home's agents beside the conversation), template `texting`;
//   - **Start empty and ask my agent**: an empty Garden (`blank`) whose
//     Ask my agent chooser opens as soon as it shows.
// "Start with the default" is the selected choice (focused, so Enter takes
// it): it gives a working page straight away, and an empty Garden is one
// click from it anyway (Start with the default on the empty page). Either
// creates the Garden (`gardens.create`, cap `gardens:manage`), switches
// to it and closes the menu. Esc goes back a step; a refused name goes
// back to the field with the reason.

import { useEffect, useRef, useState } from 'react'
import type { ZenGardenTemplate, ZenWidgetBridge } from '@/lib/zen/zen-bridge'

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

export function ZenNewGarden({ bridge, onDone }: { bridge: ZenWidgetBridge; onDone(): void }): React.JSX.Element {
  const [step, setStep] = useState<'closed' | 'name' | 'start'>('closed')
  const [name, setName] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
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
  }

  const next = (): void => {
    const problem = zenNewGardenNameProblem(
      name,
      bridge.gardens.list().map((g) => g.name),
    )
    setError(problem)
    if (!problem) setStep('start')
  }

  const create = (start: ZenGardenTemplate, ask: boolean): void => {
    if (busy) return
    setBusy(true)
    setError(null)
    void Promise.resolve()
      .then(() => bridge.gardens.create(name, start, { ask }))
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
        onClick={() => setStep('name')}
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
      {ZEN_NEW_GARDEN_STARTS.map((c, i) => (
        <button
          key={c.start}
          ref={i === 0 ? firstChoiceRef : undefined}
          type="button"
          disabled={busy}
          data-zen-new-garden-choice={c.start}
          data-zen-new-garden-selected={i === 0 ? '' : undefined}
          onClick={() => create(c.start, c.ask)}
          className="flex w-full flex-col items-start text-left cursor-pointer disabled:cursor-default"
          style={{
            gap: 2,
            padding: '7px 10px',
            borderRadius: 'calc(var(--zen-radius) - 4px)',
            color: 'var(--zen-text)',
            background: i === 0 ? 'var(--zen-surface)' : 'transparent',
            border: `1px solid ${i === 0 ? 'var(--zen-accent)' : 'var(--zen-border)'}`,
          }}
        >
          <span style={{ fontWeight: 600 }}>{c.title}</span>
          <span style={{ fontSize: '0.8em', color: 'var(--zen-text-muted)' }}>{c.detail}</span>
        </button>
      ))}
      {errorLine}
    </div>
  )
}
