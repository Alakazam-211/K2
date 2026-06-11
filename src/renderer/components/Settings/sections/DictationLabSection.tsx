// 0.37.9 — Dev-only Dictation Lab.
//
// Multiple input variants side-by-side, each instrumented with a
// full event tap. Press Fn-Fn (or your configured Apple Dictation
// shortcut) on each one; the live log shows EVERY event the
// element fires during engagement so we can isolate which
// config/attribute/handler combo causes engagement to hang vs.
// engage cleanly.
//
// Gated on `import.meta.env.DEV`. Production never sees this section.
//
// Event taps capture:
//   - focus / blur (when does AppKit hand off the input?)
//   - input (with InputEvent.inputType + data + isComposing)
//   - compositionstart / compositionupdate / compositionend
//   - keydown / keyup
//   - selectionchange (document-level; filtered by activeElement)
//   - element bounding rect snapshot on focus and on every input
//
// Plus a small textarea-positioning sandbox so we can experiment
// with the cursor-tracking dance the v2 terminal uses, in
// isolation from the rest of the terminal pipeline.

import { useCallback, useEffect, useRef, useState } from 'react'
import type { SettingEntry } from '../searchManifest'

// Web Speech API types — not in lib.dom.d.ts because the spec is
// non-standard. WebKit ships `webkitSpeechRecognition`; we declare
// the minimum surface we use so TS is happy.
interface SpeechRecognition extends EventTarget {
  continuous: boolean
  interimResults: boolean
  lang: string
  start(): void
  stop(): void
  onstart: ((this: SpeechRecognition, ev: Event) => void) | null
  onend: ((this: SpeechRecognition, ev: Event) => void) | null
  onresult:
    | ((this: SpeechRecognition, ev: SpeechRecognitionEvent) => void)
    | null
  onerror:
    | ((this: SpeechRecognition, ev: SpeechRecognitionErrorEvent) => void)
    | null
}
interface SpeechRecognitionEvent extends Event {
  results: SpeechRecognitionResultList
  resultIndex: number
}
type SpeechRecognitionResultList = {
  readonly length: number
  item(index: number): SpeechRecognitionResult
  [index: number]: SpeechRecognitionResult
}
type SpeechRecognitionResult = {
  readonly length: number
  item(index: number): SpeechRecognitionAlternative
  [index: number]: SpeechRecognitionAlternative
  isFinal: boolean
}
type SpeechRecognitionAlternative = {
  transcript: string
  confidence: number
}
interface SpeechRecognitionErrorEvent extends Event {
  error: string
}
declare const SpeechRecognition: { new (): SpeechRecognition } | undefined

type LogEntry = {
  ts: number
  source: string
  event: string
  detail: string
}

const MAX_LOG_LINES = 500

function formatRect(r: DOMRect | undefined): string {
  if (!r) return '<no rect>'
  return `(${r.left.toFixed(0)},${r.top.toFixed(0)} ${r.width.toFixed(0)}x${r.height.toFixed(0)})`
}

function formatInputEvent(e: InputEvent): string {
  const parts: string[] = []
  parts.push(`type=${e.inputType ?? '?'}`)
  if (e.data !== null && e.data !== undefined) parts.push(`data=${JSON.stringify(e.data)}`)
  if ('isComposing' in e) parts.push(`isComposing=${(e as InputEvent & { isComposing?: boolean }).isComposing ?? false}`)
  return parts.join(' ')
}

function formatCompositionEvent(e: CompositionEvent): string {
  return `data=${JSON.stringify(e.data ?? '')}`
}

function formatKeyboardEvent(e: KeyboardEvent): string {
  const mods: string[] = []
  if (e.metaKey) mods.push('M')
  if (e.ctrlKey) mods.push('C')
  if (e.altKey) mods.push('A')
  if (e.shiftKey) mods.push('S')
  if (e.isComposing) mods.push('IME')
  return `key=${JSON.stringify(e.key)} code=${e.code}${mods.length ? ' ' + mods.join('+') : ''}`
}

function useEventTap(
  ref: React.RefObject<HTMLInputElement | HTMLTextAreaElement | null>,
  source: string,
  push: (entry: Omit<LogEntry, 'ts'>) => void,
): void {
  useEffect(() => {
    const el = ref.current
    if (!el) return

    const log = (event: string, detail: string): void => {
      push({ source, event, detail })
    }

    const onFocus = (): void => {
      const rect = el.getBoundingClientRect()
      log(
        'focus',
        `rect=${formatRect(rect)} value=${JSON.stringify((el as HTMLInputElement).value).slice(0, 40)} disabled=${(el as HTMLInputElement).disabled}`,
      )
    }
    const onBlur = (): void => {
      log('blur', `next=${document.activeElement?.tagName ?? 'null'}`)
    }
    const onInput = (e: Event): void => {
      const ie = e as InputEvent
      const rect = el.getBoundingClientRect()
      log(
        'input',
        `${formatInputEvent(ie)} value=${JSON.stringify((el as HTMLInputElement).value).slice(0, 60)} rect=${formatRect(rect)}`,
      )
    }
    const onCompositionStart = (e: Event): void => {
      log('compositionstart', formatCompositionEvent(e as CompositionEvent))
    }
    const onCompositionUpdate = (e: Event): void => {
      log('compositionupdate', formatCompositionEvent(e as CompositionEvent))
    }
    const onCompositionEnd = (e: Event): void => {
      log('compositionend', formatCompositionEvent(e as CompositionEvent))
    }
    const onKeyDown = (e: Event): void => {
      log('keydown', formatKeyboardEvent(e as KeyboardEvent))
    }
    const onKeyUp = (e: Event): void => {
      log('keyup', formatKeyboardEvent(e as KeyboardEvent))
    }

    el.addEventListener('focus', onFocus)
    el.addEventListener('blur', onBlur)
    el.addEventListener('input', onInput)
    el.addEventListener('compositionstart', onCompositionStart)
    el.addEventListener('compositionupdate', onCompositionUpdate)
    el.addEventListener('compositionend', onCompositionEnd)
    el.addEventListener('keydown', onKeyDown)
    el.addEventListener('keyup', onKeyUp)
    return () => {
      el.removeEventListener('focus', onFocus)
      el.removeEventListener('blur', onBlur)
      el.removeEventListener('input', onInput)
      el.removeEventListener('compositionstart', onCompositionStart)
      el.removeEventListener('compositionupdate', onCompositionUpdate)
      el.removeEventListener('compositionend', onCompositionEnd)
      el.removeEventListener('keydown', onKeyDown)
      el.removeEventListener('keyup', onKeyUp)
    }
  }, [ref, source, push])
}

function CaseRow({
  id,
  label,
  hint,
  children,
}: {
  id: string
  label: string
  hint?: string
  children: React.ReactNode
}): React.JSX.Element {
  return (
    <div
      data-settings-id={id}
      className="border border-[var(--color-border)] p-3 mb-2"
    >
      <div className="flex items-baseline justify-between mb-2">
        <div>
          <div className="text-xs font-medium text-[var(--color-text-primary)]">
            {label}
          </div>
          {hint && (
            <div className="text-[10px] text-[var(--color-text-muted)] mt-0.5">
              {hint}
            </div>
          )}
        </div>
        <code className="text-[9px] text-[var(--color-text-muted)] tabular-nums">
          {id}
        </code>
      </div>
      {children}
    </div>
  )
}

export function DictationLabSection(): React.JSX.Element {
  const [log, setLog] = useState<LogEntry[]>([])
  const [autoScroll, setAutoScroll] = useState(true)
  // The log scroll container — we set scrollTop directly so we
  // never call scrollIntoView (which can steal focus from the
  // input the user is mid-typing in, breaking controlled inputs
  // C/D/F/G/H while leaving uncontrolled A/B/E alone).
  const logScrollRef = useRef<HTMLDivElement | null>(null)

  const push = useCallback((entry: Omit<LogEntry, 'ts'>) => {
    setLog((prev) => {
      const next = [...prev, { ts: performance.now(), ...entry }]
      if (next.length > MAX_LOG_LINES) next.splice(0, next.length - MAX_LOG_LINES)
      return next
    })
  }, [])

  useEffect(() => {
    if (!autoScroll) return
    const el = logScrollRef.current
    if (!el) return
    // Direct scrollTop assignment — no focus side effects, no
    // viewport math, just "scroll this container to the bottom."
    el.scrollTop = el.scrollHeight
  }, [log, autoScroll])

  // ── Test cases ─────────────────────────────────────────────
  // Each ref is independent so taps don't cross. Order roughly
  // walks from "minimal" → "all the gotchas we've fought."
  const refBare = useRef<HTMLInputElement>(null)
  const refAttrs = useRef<HTMLInputElement>(null)
  const refControlled = useRef<HTMLInputElement>(null)
  const refTransformer = useRef<HTMLInputElement>(null)
  const refDisabled = useRef<HTMLInputElement>(null)
  const refKeydown = useRef<HTMLInputElement>(null)
  const refTextareaBare = useRef<HTMLTextAreaElement>(null)
  const refTextareaCursorish = useRef<HTMLTextAreaElement>(null)

  // Light controlled state for the variants that need it.
  const [controlledValue, setControlledValue] = useState('')
  const [transformerValue, setTransformerValue] = useState('')
  const [keydownValue, setKeydownValue] = useState('')
  const [textareaValue, setTextareaValue] = useState('')
  const [textareaCursorishValue, setTextareaCursorishValue] = useState('')

  // ── Web Speech API (separate from Apple Dictation) ─────────────
  // Dictation = system-level shortcut → text input service. Web
  // Speech = JS API → Apple STT via WebKit. Different surfaces; the
  // Web Speech path doesn't rely on responder-chain hand-off and
  // works in WKWebView the same as Safari. We use it as the
  // backstop voice surface while we keep diagnosing why
  // Fn-Fn engagement is flaky in this embedded WebView.
  const [speechRecognition, setSpeechRecognition] = useState<SpeechRecognition | null>(null)
  const [speechSupported, setSpeechSupported] = useState(false)
  const [speechListening, setSpeechListening] = useState(false)
  const [speechInterim, setSpeechInterim] = useState('')
  const [speechFinal, setSpeechFinal] = useState('')
  const [speechError, setSpeechError] = useState<string | null>(null)
  const speechAreaRef = useRef<HTMLTextAreaElement>(null)

  useEffect(() => {
    const SR =
      (window as Window & {
        SpeechRecognition?: typeof SpeechRecognition
        webkitSpeechRecognition?: typeof SpeechRecognition
      }).SpeechRecognition ??
      (window as Window & {
        SpeechRecognition?: typeof SpeechRecognition
        webkitSpeechRecognition?: typeof SpeechRecognition
      }).webkitSpeechRecognition
    if (!SR) {
      setSpeechSupported(false)
      return
    }
    setSpeechSupported(true)
    const sr = new SR()
    sr.continuous = true
    sr.interimResults = true
    sr.lang = 'en-US'

    sr.onstart = () => {
      setSpeechListening(true)
      setSpeechError(null)
      push({
        source: 'I.webspeech',
        event: 'speech.onstart',
        detail: 'recognition started',
      })
    }
    sr.onend = () => {
      setSpeechListening(false)
      push({
        source: 'I.webspeech',
        event: 'speech.onend',
        detail: 'recognition ended',
      })
    }
    sr.onerror = (event) => {
      const errEvent = event as SpeechRecognitionErrorEvent
      setSpeechError(errEvent.error)
      push({
        source: 'I.webspeech',
        event: 'speech.onerror',
        detail: `error=${errEvent.error}`,
      })
    }
    sr.onresult = (event) => {
      let interim = ''
      let final = ''
      const results = event.results
      for (let i = event.resultIndex; i < results.length; i++) {
        const result = results[i]
        const transcript = result?.[0]?.transcript ?? ''
        if (result?.isFinal) {
          final += transcript
        } else {
          interim += transcript
        }
      }
      if (interim) {
        setSpeechInterim(interim)
        push({
          source: 'I.webspeech',
          event: 'speech.interim',
          detail: JSON.stringify(interim).slice(0, 80),
        })
      }
      if (final) {
        setSpeechFinal((prev) => (prev ? prev + ' ' : '') + final.trim())
        setSpeechInterim('')
        push({
          source: 'I.webspeech',
          event: 'speech.final',
          detail: JSON.stringify(final).slice(0, 80),
        })
      }
    }
    setSpeechRecognition(sr)
    return () => {
      try {
        sr.stop()
      } catch {
        // already stopped
      }
    }
  }, [push])

  const toggleSpeech = useCallback((): void => {
    if (!speechRecognition) return
    if (speechListening) {
      speechRecognition.stop()
    } else {
      try {
        setSpeechInterim('')
        speechRecognition.start()
      } catch (err) {
        setSpeechError(String(err))
      }
    }
  }, [speechRecognition, speechListening])

  const clearSpeechTranscript = useCallback((): void => {
    setSpeechInterim('')
    setSpeechFinal('')
    setSpeechError(null)
  }, [])

  // Bare inputs (no React state, no controlled value).
  useEventTap(refBare, 'A.bare', push)
  useEventTap(refAttrs, 'B.attrs', push)
  useEventTap(refControlled, 'C.controlled', push)
  useEventTap(refTransformer, 'D.toLowerCase', push)
  useEventTap(refDisabled, 'E.disabled-flicker', push)
  useEventTap(refKeydown, 'F.onkeydown-enter', push)
  useEventTap(refTextareaBare, 'G.textarea', push)
  useEventTap(refTextareaCursorish, 'H.textarea-1cell', push)

  // Disabled-flicker variant: simulate the AgentDisplayNameField
  // pattern where `disabled={!ready || busy}` flips on first mount.
  const [disabledReady, setDisabledReady] = useState(false)
  useEffect(() => {
    const t = setTimeout(() => setDisabledReady(true), 50)
    return () => clearTimeout(t)
  }, [])

  // Selectionchange tap is document-level; filter by activeElement
  // so we only log when one of OUR test elements is focused.
  useEffect(() => {
    const refs = [
      refBare, refAttrs, refControlled, refTransformer,
      refDisabled, refKeydown, refTextareaBare, refTextareaCursorish,
    ]
    const onSel = (): void => {
      const ae = document.activeElement
      if (!ae) return
      for (const r of refs) {
        if (r.current === ae) {
          const sel = (ae as HTMLInputElement).selectionStart
          const end = (ae as HTMLInputElement).selectionEnd
          push({
            source: ae.getAttribute('data-tap-source') ?? '?',
            event: 'selectionchange',
            detail: `start=${sel} end=${end}`,
          })
          break
        }
      }
    }
    document.addEventListener('selectionchange', onSel)
    return () => document.removeEventListener('selectionchange', onSel)
  }, [push])

  const clearLog = (): void => setLog([])
  const copyLog = (): void => {
    const text = log
      .map(
        (e) =>
          `${e.ts.toFixed(0).padStart(8)} ${e.source.padEnd(20)} ${e.event.padEnd(20)} ${e.detail}`,
      )
      .join('\n')
    navigator.clipboard.writeText(text).catch(() => {
      console.warn('[dictation-lab] clipboard write failed')
    })
  }

  return (
    <div className="flex gap-4 h-full min-h-0">
      {/* Left column — test inputs (scrolls independently). */}
      <div className="flex-1 min-w-0 overflow-y-auto pr-2">
        <div className="mb-4">
          <h3 className="text-xs font-medium text-[var(--color-text-primary)]">
            Dictation Lab
          </h3>
          <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5">
            Dev-only. Each row is a different input config; press your
            Apple Dictation shortcut (Fn-Fn / Globe) on each to see how
            AppKit engages. The log on the right captures focus/blur,
            input, composition*, keydown/up, selectionchange, and the
            element's bounding rect at focus time.
          </p>
        </div>

      <CaseRow
        id="dict-A-bare"
        label="A. Bare uncontrolled input"
        hint="No React state, no extra attributes — closest to a static HTML form."
      >
        <input
          ref={refBare}
          data-tap-source="A.bare"
          type="text"
          placeholder="dictate here"
          className="w-full px-2 py-1 text-xs bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-[var(--color-text-primary)] focus:outline-none focus:border-[var(--color-accent)]"
        />
      </CaseRow>

      <CaseRow
        id="dict-B-attrs"
        label="B. Bare + standard attrs"
        hint="spellCheck=false, autoComplete/autoCorrect/autoCapitalize=off."
      >
        <input
          ref={refAttrs}
          data-tap-source="B.attrs"
          type="text"
          placeholder="dictate here"
          spellCheck={false}
          autoComplete="off"
          autoCorrect="off"
          autoCapitalize="off"
          className="w-full px-2 py-1 text-xs bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-[var(--color-text-primary)] focus:outline-none focus:border-[var(--color-accent)]"
        />
      </CaseRow>

      <CaseRow
        id="dict-C-controlled"
        label="C. Controlled input (passthrough)"
        hint="value + setState on every change, no transform."
      >
        <input
          ref={refControlled}
          data-tap-source="C.controlled"
          type="text"
          placeholder="dictate here"
          value={controlledValue}
          onChange={(e) => setControlledValue(e.target.value)}
          className="w-full px-2 py-1 text-xs bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-[var(--color-text-primary)] focus:outline-none focus:border-[var(--color-accent)]"
        />
      </CaseRow>

      <CaseRow
        id="dict-D-transformer"
        label="D. Controlled with toLowerCase mid-flight"
        hint="Mimics the original AgentDisplayNameField anti-pattern. Value is mutated on every keystroke."
      >
        <input
          ref={refTransformer}
          data-tap-source="D.toLowerCase"
          type="text"
          placeholder="dictate here"
          value={transformerValue}
          onChange={(e) => setTransformerValue(e.target.value.toLowerCase())}
          className="w-full px-2 py-1 text-xs bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-[var(--color-text-primary)] focus:outline-none focus:border-[var(--color-accent)]"
        />
      </CaseRow>

      <CaseRow
        id="dict-E-disabled"
        label="E. disabled flicker on mount"
        hint="disabled=true for 50ms then false — mimics async-load gating."
      >
        <input
          ref={refDisabled}
          data-tap-source="E.disabled-flicker"
          type="text"
          placeholder="dictate here"
          disabled={!disabledReady}
          className="w-full px-2 py-1 text-xs bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-[var(--color-text-primary)] focus:outline-none focus:border-[var(--color-accent)] disabled:opacity-60"
        />
      </CaseRow>

      <CaseRow
        id="dict-F-keydown"
        label="F. Controlled + onKeyDown(Enter)"
        hint="onKeyDown handler that intercepts Enter — mimics the AgentDisplayNameField save-on-Enter pattern."
      >
        <input
          ref={refKeydown}
          data-tap-source="F.onkeydown-enter"
          type="text"
          placeholder="dictate here, hit Enter"
          value={keydownValue}
          onChange={(e) => setKeydownValue(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') {
              push({ source: 'F.onkeydown-enter', event: 'enter-pressed', detail: `value=${JSON.stringify(keydownValue)}` })
            }
          }}
          className="w-full px-2 py-1 text-xs bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-[var(--color-text-primary)] focus:outline-none focus:border-[var(--color-accent)]"
        />
      </CaseRow>

      <CaseRow
        id="dict-G-textarea"
        label="G. Bare textarea (multi-line)"
        hint="Default textarea attrs. AppKit treats textarea + input differently for some clients."
      >
        <textarea
          ref={refTextareaBare}
          data-tap-source="G.textarea"
          placeholder="dictate here"
          value={textareaValue}
          onChange={(e) => setTextareaValue(e.target.value)}
          rows={3}
          className="w-full px-2 py-1 text-xs bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-[var(--color-text-primary)] focus:outline-none focus:border-[var(--color-accent)] resize-none"
        />
      </CaseRow>

      <CaseRow
        id="dict-H-textarea-1cell"
        label="H. Textarea sized 1 cell, opacity 0"
        hint="Mimics the v2 terminal shadow textarea: 9×16px, transparent, but in the layout flow."
      >
        <div
          style={{
            position: 'relative',
            height: 40,
            border: '1px dashed var(--color-border)',
            background: 'var(--color-bg)',
          }}
        >
          <textarea
            ref={refTextareaCursorish}
            data-tap-source="H.textarea-1cell"
            value={textareaCursorishValue}
            onChange={(e) => setTextareaCursorishValue(e.target.value)}
            spellCheck={false}
            autoComplete="off"
            autoCorrect="off"
            autoCapitalize="off"
            aria-label="Terminal-style shadow input"
            style={{
              position: 'absolute',
              left: 12,
              top: 12,
              width: 9,
              height: 16,
              opacity: 0,
              border: 0,
              outline: 'none',
              padding: 0,
              margin: 0,
              resize: 'none',
              whiteSpace: 'nowrap',
              overflow: 'hidden',
              color: 'transparent',
              background: 'transparent',
              caretColor: 'transparent',
            }}
          />
          <div
            style={{
              position: 'absolute',
              left: 12,
              top: 12,
              width: 9,
              height: 16,
              border: '1px solid var(--color-accent)',
              pointerEvents: 'none',
              opacity: 0.4,
            }}
            aria-hidden="true"
          />
          <div
            className="text-[10px] text-[var(--color-text-muted)] absolute"
            style={{ left: 30, top: 14 }}
          >
            ← invisible 1-cell textarea (click the box to focus, then dictate)
          </div>
          <button
            onClick={() => refTextareaCursorish.current?.focus()}
            className="absolute inset-0 cursor-pointer"
            aria-label="Focus 1-cell textarea"
            style={{ background: 'transparent', border: 0 }}
          />
        </div>
        <div className="text-[10px] text-[var(--color-text-muted)] mt-2">
          current value: <code>{JSON.stringify(textareaCursorishValue).slice(0, 80)}</code>
        </div>
      </CaseRow>

      {/* ── Case I — Web Speech API (the working alt to Fn-Fn) ── */}
      <CaseRow
        id="dict-I-webspeech"
        label="I. Web Speech API (webkitSpeechRecognition)"
        hint="Programmatic JS API. Bypasses NSTextInputClient/Apple Dictation entirely. WebKit ships this; we just call .start() / .stop()."
      >
        {!speechSupported ? (
          <div className="text-[10px] text-red-400">
            Web Speech API not available in this WebView build.
          </div>
        ) : (
          <div className="space-y-2">
            <div className="flex items-center gap-2">
              <button
                onClick={toggleSpeech}
                className={`px-3 py-1.5 text-[10px] font-medium border transition-colors no-drag cursor-pointer ${
                  speechListening
                    ? 'bg-red-500/15 border-red-500/40 text-red-300 animate-pulse'
                    : 'bg-[var(--color-accent)]/10 border-[var(--color-accent)]/30 text-[var(--color-accent)] hover:bg-[var(--color-accent)]/20'
                }`}
              >
                {speechListening ? '● Listening (click to stop)' : '🎤 Start listening'}
              </button>
              <button
                onClick={clearSpeechTranscript}
                className="px-2 py-1 text-[10px] bg-[var(--color-bg)] border border-[var(--color-border)] hover:border-[var(--color-text-muted)] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] transition-colors no-drag cursor-pointer"
              >
                Clear
              </button>
              {speechError && (
                <span className="text-[10px] text-red-400">
                  error: {speechError}
                </span>
              )}
            </div>
            <textarea
              ref={speechAreaRef}
              value={
                speechFinal +
                (speechInterim ? (speechFinal ? ' ' : '') + speechInterim : '')
              }
              readOnly
              rows={3}
              placeholder="Click 'Start listening' and speak. Final transcript accumulates here; the in-flight (italicized) interim is what WebKit's STT thinks you're saying right now."
              className="w-full px-2 py-1 text-xs bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-[var(--color-text-primary)] focus:outline-none focus:border-[var(--color-accent)] resize-none"
            />
            <div className="text-[10px] text-[var(--color-text-muted)]">
              {speechListening
                ? 'Listening… speak naturally. Pauses are OK; results stream as you talk.'
                : 'Microphone needs to be granted (Settings → Permissions → Microphone). First start may prompt.'}
            </div>
          </div>
        )}
      </CaseRow>

        <p className="mt-3 text-[10px] text-[var(--color-text-muted)]">
          How to use: focus an input, press your Apple Dictation shortcut
          (typically Fn-Fn or Globe). Watch the log on the right. A
          clean engagement produces a <code>compositionstart</code>{' '}
          within ~1–2 seconds of focus, then{' '}
          <code>compositionupdate</code>s as you speak, and a single{' '}
          <code>compositionend</code> on stop. A failed engagement
          produces NO composition events at all (just the macOS chime
          without anything else firing). Compare row by row to isolate
          which configuration breaks.
        </p>
      </div>

      {/* Right column — sticky live event log. */}
      <div className="flex-shrink-0 w-[480px] flex flex-col min-h-0 sticky top-0 self-start max-h-full">
        <div className="border border-[var(--color-border)] flex flex-col min-h-0 h-full">
        <div className="flex items-center justify-between px-3 py-2 border-b border-[var(--color-border)] bg-[var(--color-bg-elevated)]">
          <div className="flex items-center gap-3">
            <span className="text-xs font-medium text-[var(--color-text-primary)]">
              Event log
            </span>
            <span className="text-[10px] text-[var(--color-text-muted)] tabular-nums">
              {log.length} / {MAX_LOG_LINES}
            </span>
          </div>
          <div className="flex items-center gap-2">
            <label className="flex items-center gap-1 text-[10px] text-[var(--color-text-muted)]">
              <input
                type="checkbox"
                checked={autoScroll}
                onChange={(e) => setAutoScroll(e.target.checked)}
                className="cursor-pointer"
              />
              auto-scroll
            </label>
            <button
              onClick={copyLog}
              className="px-2 py-1 text-[10px] bg-[var(--color-bg)] border border-[var(--color-border)] hover:border-[var(--color-text-muted)] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] transition-colors no-drag cursor-pointer"
            >
              Copy log
            </button>
            <button
              onClick={clearLog}
              className="px-2 py-1 text-[10px] bg-[var(--color-bg)] border border-[var(--color-border)] hover:border-red-400 text-[var(--color-text-secondary)] hover:text-red-400 transition-colors no-drag cursor-pointer"
            >
              Clear
            </button>
          </div>
        </div>
        <div
          ref={logScrollRef}
          className="overflow-y-auto flex-1"
          style={{
            fontFamily: 'ui-monospace, SFMono-Regular, Menlo, monospace',
          }}
        >
          {log.length === 0 ? (
            <div className="px-3 py-6 text-[10px] text-[var(--color-text-muted)] text-center">
              No events yet. Click an input above and try dictating.
            </div>
          ) : (
            <table className="w-full text-[10px] tabular-nums">
              <tbody>
                {log.map((e, i) => (
                  <tr key={i} className="border-b border-[var(--color-border)]/40 last:border-b-0">
                    <td className="px-2 py-0.5 text-[var(--color-text-muted)] whitespace-nowrap">
                      {e.ts.toFixed(0).padStart(7)}
                    </td>
                    <td className="px-2 py-0.5 text-[var(--color-accent)] whitespace-nowrap">
                      {e.source}
                    </td>
                    <td className="px-2 py-0.5 text-[var(--color-text-primary)] whitespace-nowrap">
                      {e.event}
                    </td>
                    <td className="px-2 py-0.5 text-[var(--color-text-secondary)]">
                      {e.detail}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>
        </div>
      </div>
    </div>
  )
}

// Visible search-manifest entries — only included in DEV builds via
// the flag below. Production users shouldn't see these.
export const DICTATION_LAB_MANIFEST: SettingEntry[] = import.meta.env.DEV
  ? [
      {
        id: 'dict-A-bare',
        section: 'dictation-lab',
        label: 'Dictation Lab — A. Bare uncontrolled',
        description: 'Test variant: no React state, no extra attrs.',
        keywords: ['dictation', 'lab', 'voice', 'debug', 'dev'],
      },
    ]
  : []
