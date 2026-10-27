// Loads the Seti woff for the Files drawer and keeps retrying until it lands.
//
// WebKit (WKWebView on macOS, checked on 27.0.1) never fetches an
// `@font-face` again once its first fetch failed: the face stays `error`,
// `document.fonts.load()` rejects again without a new request, and every
// glyph draws as a hollow box until the page reloads. So one failed fetch
// (an app or daemon restart, a webview recovery) used to leave the drawer
// on boxes for the rest of the session. Retrying means a NEW `@font-face`
// rule (new style text), which WebKit treats as a new face and fetches.

export const SETI_FONT_STYLE_ID = 'k2-seti-font-face'
export const SETI_FONT_FAMILY = 'k2-seti'
/** Backoff after the n-th failure: 1s, 2s, 4s … capped at 30s. */
export const SETI_FONT_RETRY_CAP_MS = 30_000

type FontLoader = { load(font: string): Promise<unknown[]> }

export interface SetiFontEnv {
  document: Pick<Document, 'getElementById' | 'createElement' | 'head'> & {
    fonts?: FontLoader
  }
  setTimeout: (fn: () => void, ms: number) => unknown
}

type State = 'idle' | 'loading' | 'ready' | 'waiting'

export interface SetiFontLoader {
  ensure(): void
  readonly state: State
  readonly failures: number
}

export function setiFontRetryDelay(failures: number): number {
  return Math.min(SETI_FONT_RETRY_CAP_MS, 1000 * 2 ** Math.max(0, failures - 1))
}

function faceCss(url: string): string {
  return `
@font-face {
  font-family: '${SETI_FONT_FAMILY}';
  src: url('${url}') format('woff');
  font-weight: normal;
  font-style: normal;
  font-display: block;
}
`.trim()
}

export function createSetiFontLoader(url: string, env: SetiFontEnv): SetiFontLoader {
  let state: State = 'idle'
  let failures = 0

  // A fresh rule every attempt: replacing the style text drops the errored
  // face and adds a new one that WebKit will fetch.
  function writeFace(): void {
    const doc = env.document
    let style = doc.getElementById(SETI_FONT_STYLE_ID) as HTMLStyleElement | null
    if (!style) {
      style = doc.createElement('style')
      style.id = SETI_FONT_STYLE_ID
      doc.head.appendChild(style)
    }
    // Attempt marker in a comment so the text always changes.
    style.textContent = `/* attempt ${failures} */\n${faceCss(url)}`
  }

  function failed(): void {
    failures += 1
    state = 'waiting'
    env.setTimeout(() => {
      state = 'idle'
      ensure()
    }, setiFontRetryDelay(failures))
  }

  function ensure(): void {
    if (state !== 'idle') return
    writeFace()
    const fonts = env.document.fonts
    if (!fonts) {
      state = 'ready'
      return
    }
    state = 'loading'
    fonts.load(`16px '${SETI_FONT_FAMILY}'`).then(
      (faces) => {
        // No matching face means the rule never took; treat as a miss.
        if (faces.length > 0) {
          state = 'ready'
          failures = 0
        } else {
          failed()
        }
      },
      () => failed(),
    )
  }

  return {
    ensure,
    get state() {
      return state
    },
    get failures() {
      return failures
    },
  }
}
