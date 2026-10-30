// Typing into the device (sim-keyboard.ts has the rules).
//
// The keys land in a hidden textarea inside the device screen, not on the
// screen <div>: an input method (and Option dead keys) only composes in an
// editable element, and K2's terminal gives focus back to itself unless an
// input or textarea has it. Clicking the screen focuses the sink (the
// screen's own focus is handed on), Esc or a click outside lets it go.
// While it has focus the screen shows a ring and a "Typing into the device"
// hint. Only a viewer who may control the device gets a sink at all, so a
// watch-only login never types.

import { useCallback, useEffect, useRef, useState } from 'react'
import { readText as readClipboardText } from '@tauri-apps/plugin-clipboard-manager'
import { isMacPlatform } from '@/lib/desktop-chrome'
import { commitInput, deviceChords, keyVerdict, pasteInput } from './sim-keyboard'
import type { SimInput } from './sim-stream'

export interface UseSimKeyboardArgs {
  /** This viewer may control the device and it is live. */
  interactive: boolean
  send(input: SimInput): void
  /** ⌘ is the chord key (default: this computer is a Mac). */
  mac?: boolean
}

export interface SimKeyboard {
  focused: boolean
  /** Put the keyboard on the device. */
  focus(): void
  /** The screen <div> got focus: hand it to the sink. */
  onScreenFocus(e: React.FocusEvent<HTMLElement>): void
  /** A click on the screen keeps the keyboard there (no focus bounce). */
  onScreenMouseDown(e: React.MouseEvent<HTMLElement>): void
  /** The sink, the focus ring and the paste notice, inside the screen. */
  overlay: React.JSX.Element | null
  /** "Typing into the device" under the device, while focused. */
  hint: React.JSX.Element | null
}

/** The clipboard's text, read only for an explicit paste. */
async function readClipboard(): Promise<string> {
  try {
    return (await readClipboardText()) ?? ''
  } catch (err) {
    // A web viewer has no Tauri clipboard; the browser's asks the person.
    if (typeof navigator !== 'undefined' && navigator.clipboard?.readText) return navigator.clipboard.readText()
    throw err
  }
}

export function useSimKeyboard({ interactive, send, mac }: UseSimKeyboardArgs): SimKeyboard {
  const isMac = mac ?? isMacPlatform()
  const sinkRef = useRef<HTMLTextAreaElement | null>(null)
  const [focused, setFocused] = useState(false)
  const [notice, setNotice] = useState<string | null>(null)
  const composingRef = useRef(false)
  /** When ⌘V last ran: the native paste event it may still cause is skipped. */
  const chordPasteAtRef = useRef(0)
  const sendRef = useRef(send)
  sendRef.current = send

  const clearSink = (): void => {
    if (sinkRef.current) sinkRef.current.value = ''
  }
  const focus = useCallback((): void => {
    sinkRef.current?.focus({ preventScroll: true })
  }, [])
  // Losing control (another person took over, the stream closed) lets go.
  useEffect(() => {
    if (!interactive) setFocused(false)
  }, [interactive])
  useEffect(() => {
    if (!notice) return
    const t = window.setTimeout(() => setNotice(null), 4000)
    return () => window.clearTimeout(t)
  }, [notice])

  const pasteText = (text: string): void => {
    const r = pasteInput(text)
    if (!r) return
    if ('error' in r) {
      setNotice(r.error)
      return
    }
    sendRef.current(r.send)
  }

  const onKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>): void => {
    const v = keyVerdict(
      {
        key: e.key,
        metaKey: e.metaKey,
        ctrlKey: e.ctrlKey,
        altKey: e.altKey,
        shiftKey: e.shiftKey,
        keyCode: e.keyCode,
        isComposing: e.nativeEvent.isComposing || composingRef.current,
      },
      isMac,
    )
    if (v === 'pass' || v === 'ime') return
    e.preventDefault()
    e.stopPropagation()
    if (v === 'blur') {
      sinkRef.current?.blur()
      return
    }
    if (v === 'paste') {
      chordPasteAtRef.current = performance.now()
      readClipboard()
        .then(pasteText)
        .catch((err: unknown) => {
          console.warn('[mobile] paste: could not read the clipboard:', err)
          setNotice('K2 couldn’t read the clipboard.')
        })
      return
    }
    sendRef.current(v.send)
  }
  const onCompositionStart = (): void => {
    composingRef.current = true
  }
  const onCompositionEnd = (e: React.CompositionEvent<HTMLTextAreaElement>): void => {
    composingRef.current = false
    const m = commitInput(e.data)
    clearSink()
    if (m) sendRef.current(m)
  }
  // Text the keydown path didn't send (the composition's own input events)
  // never reaches the device a second time: the sink is emptied.
  const onInput = (): void => {
    if (!composingRef.current) clearSink()
  }
  const onPaste = (e: React.ClipboardEvent<HTMLTextAreaElement>): void => {
    e.preventDefault()
    // ⌘V already pasted from its keydown; a menu Paste comes only here.
    if (performance.now() - chordPasteAtRef.current < 500) return
    pasteText(e.clipboardData.getData('text/plain') || e.clipboardData.getData('text'))
  }

  const onScreenFocus = (e: React.FocusEvent<HTMLElement>): void => {
    if (interactive && e.target === e.currentTarget) focus()
  }

  const onScreenMouseDown = (e: React.MouseEvent<HTMLElement>): void => {
    if (!interactive || e.button !== 0) return
    e.preventDefault()
    focus()
  }

  const on = interactive && focused
  const overlay = interactive ? (
    <>
      <textarea
        ref={sinkRef}
        aria-label="Type into the device"
        autoCapitalize="off"
        autoComplete="off"
        autoCorrect="off"
        spellCheck={false}
        tabIndex={0}
        // Invisible but focusable: off-canvas in the screen's corner, so an
        // input method's candidate window opens near the device.
        className="pointer-events-none absolute bottom-0 left-0 h-px w-px resize-none overflow-hidden border-0 p-0 opacity-0 outline-none"
        onFocus={() => setFocused(true)}
        onBlur={() => {
          composingRef.current = false
          clearSink()
          setFocused(false)
        }}
        onKeyDown={onKeyDown}
        onCompositionStart={onCompositionStart}
        onCompositionEnd={onCompositionEnd}
        onInput={onInput}
        onPaste={onPaste}
        data-mobile-keys=""
      />
      {on ? <div className="pointer-events-none absolute inset-0 ring-2 ring-inset ring-[var(--color-accent)]" data-mobile-focus-ring="" /> : null}
      {notice ? (
        <div className="pointer-events-none absolute inset-x-0 bottom-[10%] flex justify-center px-3">
          <span className="max-w-full rounded-full bg-black/75 px-3 py-1 text-center text-[11px] text-white/90 backdrop-blur" data-mobile-key-notice="">
            {notice}
          </span>
        </div>
      ) : null}
    </>
  ) : null
  const hint = on ? (
    <div className="pointer-events-none absolute inset-x-0 bottom-2 flex justify-center" data-mobile-typing-hint="">
      <span className="rounded-full border border-[var(--color-border)] bg-[var(--color-bg)]/90 px-2.5 py-0.5 text-[11px] text-[var(--color-text-muted)] backdrop-blur">
        Typing into the device · <kbd className="font-sans">Esc</kbd> to stop
      </span>
    </div>
  ) : null

  return { focused: on, focus, onScreenFocus, onScreenMouseDown, overlay, hint }
}

/** ⋯ help: what the keyboard does while the device has focus. */
export function SimKeyboardHelp(): React.JSX.Element {
  return (
    <div className="mt-1 border-t border-[var(--color-border)] px-3 py-2 text-[11px] leading-snug text-[var(--color-text-muted)]" data-mobile-keyboard-help="">
      <div className="text-xs text-[var(--color-text-primary)]">Keyboard</div>
      <div>Click the screen, then type: keys and paste go to the device. K2’s other shortcuts keep working.</div>
      <dl className="mt-1 grid grid-cols-[auto_1fr] gap-x-2 gap-y-0.5">
        {deviceChords(isMacPlatform()).map((c) => (
          <div key={c.keys} className="contents">
            <dt className="whitespace-nowrap font-mono text-[var(--color-text-primary)]">{c.keys}</dt>
            <dd>{c.does}</dd>
          </div>
        ))}
      </dl>
    </div>
  )
}
