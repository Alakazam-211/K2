// Ticket HTML brief — the pure srcdoc builder (prd-ticket-html-brief-v1
// H12–H14, H38). The daemon already CLEANED the HTML on write (H11); this
// module only presents it:
//
//   - Links are taken OUT of the frame. An inert frame (no scripts) can't
//     catch a click, and Windows WebView2 doesn't guard subframe
//     navigation, so every `<a>` loses its `href` and gets a `[n]` marker.
//     The parent lists the full URLs below the frame and opens them in the
//     system browser (H13).
//   - The K2 brief stylesheet is themed from the current app scheme (H14),
//     never `bg-white`.
//
// The result goes through `HtmlFrame` with the `inert` profile, which puts
// the per-frame CSP meta first (after the doctype) and sets `sandbox=""`.
// This module never decides what is allowed (H27): it does not strip tags.

export interface BriefTokens {
  /** `dark` | `light` — drives `color-scheme` inside the frame. */
  scheme: 'dark' | 'light'
  bg: string
  surface: string
  text: string
  textSecondary: string
  muted: string
  border: string
  accent: string
  font: string
}

export interface BriefLink {
  /** 1-based marker shown in the frame as `[n]`. */
  n: number
  url: string
}

export interface BriefSrcDoc {
  frameHtml: string
  links: BriefLink[]
}

const DARK_FALLBACK: BriefTokens = {
  scheme: 'dark',
  bg: '#0a0a0a',
  surface: '#141414',
  text: '#e4e4e7',
  textSecondary: '#a1a1aa',
  muted: '#71717a',
  border: '#2a2a2a',
  accent: '#3b82f6',
  font: "Menlo, Monaco, 'SF Mono', Consolas, monospace",
}

const LIGHT_FALLBACK: BriefTokens = {
  scheme: 'light',
  bg: '#fafafa',
  surface: '#ffffff',
  text: '#18181b',
  textSecondary: '#3f3f46',
  muted: '#71717a',
  border: '#e4e4e7',
  accent: '#2563eb',
  font: "Menlo, Monaco, 'SF Mono', Consolas, monospace",
}

export function fallbackBriefTokens(scheme: 'dark' | 'light'): BriefTokens {
  return scheme === 'light' ? LIGHT_FALLBACK : DARK_FALLBACK
}

/** A CSS value read from the app's computed style may only carry what a
 *  color or font-family needs. Anything that could close the `<style>` or
 *  open a new rule falls back to the default. */
const SAFE_CSS_VALUE = /^[#a-zA-Z0-9(),.%\s'"_-]{1,200}$/

function safe(value: string | undefined, fallback: string): string {
  const v = (value ?? '').trim()
  return v && SAFE_CSS_VALUE.test(v) ? v : fallback
}

/** Read the brief tokens off the live app theme (`data-scheme` + the
 *  `--color-*` custom properties on `<html>`). */
export function readBriefTokens(root: HTMLElement = document.documentElement): BriefTokens {
  const scheme = root.getAttribute('data-scheme') === 'light' ? 'light' : 'dark'
  const fb = fallbackBriefTokens(scheme)
  const cs = getComputedStyle(root)
  const v = (name: string): string => cs.getPropertyValue(name)
  return {
    scheme,
    bg: safe(v('--color-bg'), fb.bg),
    surface: safe(v('--color-bg-surface'), fb.surface),
    text: safe(v('--color-text-primary'), fb.text),
    textSecondary: safe(v('--color-text-secondary'), fb.textSecondary),
    muted: safe(v('--color-text-muted'), fb.muted),
    border: safe(v('--color-border'), fb.border),
    accent: safe(v('--color-accent'), fb.accent),
    font: safe(v('--font-ui'), fb.font),
  }
}

/** The K2 brief stylesheet. Semantic tags plus the three hooks agents may
 *  use: `.k2-need` (highlighted "What I need from you"), `.k2-options`
 *  (an `ol` of choices), `.k2-callout`. */
export function briefStylesheet(tokens: BriefTokens): string {
  const t: BriefTokens = {
    scheme: tokens.scheme === 'light' ? 'light' : 'dark',
    bg: safe(tokens.bg, fallbackBriefTokens(tokens.scheme).bg),
    surface: safe(tokens.surface, fallbackBriefTokens(tokens.scheme).surface),
    text: safe(tokens.text, fallbackBriefTokens(tokens.scheme).text),
    textSecondary: safe(tokens.textSecondary, fallbackBriefTokens(tokens.scheme).textSecondary),
    muted: safe(tokens.muted, fallbackBriefTokens(tokens.scheme).muted),
    border: safe(tokens.border, fallbackBriefTokens(tokens.scheme).border),
    accent: safe(tokens.accent, fallbackBriefTokens(tokens.scheme).accent),
    font: safe(tokens.font, fallbackBriefTokens(tokens.scheme).font),
  }
  return [
    `:root{color-scheme:${t.scheme};--bg:${t.surface};--text:${t.text};--text2:${t.textSecondary};--muted:${t.muted};--border:${t.border};--accent:${t.accent};}`,
    `html,body{margin:0;padding:0;background:var(--bg);color:var(--text);}`,
    `body.k2-brief{font-family:${t.font};font-size:13px;line-height:1.55;padding:14px 18px;overflow-wrap:anywhere;}`,
    `h1,h2,h3,h4{color:var(--text);line-height:1.3;margin:1.1em 0 .45em;}`,
    `h1{font-size:1.35em}h2{font-size:1.18em}h3{font-size:1.05em}h4{font-size:1em}`,
    `body.k2-brief>:first-child{margin-top:0}`,
    `p{margin:.55em 0}ul,ol{margin:.5em 0;padding-left:1.5em}li{margin:.2em 0}`,
    `a{color:var(--accent);text-decoration:underline;cursor:default}`,
    `sup.k2-link-ref{color:var(--muted);font-size:.75em;margin-left:.15em}`,
    `code,kbd,pre{font-family:inherit;font-size:.95em}`,
    `code,kbd{background:color-mix(in srgb,var(--text) 8%,transparent);padding:.05em .3em}`,
    `pre{background:color-mix(in srgb,var(--text) 6%,transparent);border:1px solid var(--border);padding:.6em .8em;overflow:auto;white-space:pre}`,
    `pre code{background:none;padding:0}`,
    `blockquote{margin:.6em 0;padding:.2em .9em;border-left:3px solid var(--border);color:var(--text2)}`,
    `hr{border:0;border-top:1px solid var(--border);margin:1em 0}`,
    `table{border-collapse:collapse;margin:.6em 0;max-width:100%}`,
    `th,td{border:1px solid var(--border);padding:.3em .6em;text-align:left;vertical-align:top}`,
    `th{background:color-mix(in srgb,var(--text) 6%,transparent)}`,
    `img{max-width:100%;height:auto}`,
    `figure{margin:.8em 0}figcaption{color:var(--muted);font-size:.9em}`,
    `details{margin:.5em 0}summary{cursor:pointer}`,
    `mark{background:color-mix(in srgb,var(--accent) 30%,transparent);color:inherit}`,
    `.k2-need{border:1px solid var(--accent);background:color-mix(in srgb,var(--accent) 10%,transparent);padding:.4em 1em;margin:.9em 0}`,
    `.k2-need>:first-child{margin-top:.3em}`,
    `ol.k2-options{padding-left:1.8em}ol.k2-options>li{padding:.15em 0}`,
    `.k2-callout{border-left:3px solid var(--accent);background:color-mix(in srgb,var(--text) 5%,transparent);padding:.4em .9em;margin:.7em 0}`,
  ].join('\n')
}

const LINK_SCHEMES = /^(https?:|mailto:)/i

/**
 * Build the frame document for a cleaned brief. Pure apart from the
 * DOMParser it uses for an INERT parse in the parent (a DOMParser document
 * has no browsing context: no scripts run, nothing loads).
 */
export function buildBriefSrcDoc(html: string, tokens: BriefTokens): BriefSrcDoc {
  const doc = new DOMParser().parseFromString(
    `<!doctype html><html><head></head><body>${html}</body></html>`,
    'text/html',
  )
  const links: BriefLink[] = []
  for (const a of Array.from(doc.body.querySelectorAll('a'))) {
    const href = (a.getAttribute('href') ?? '').trim()
    a.removeAttribute('href')
    a.removeAttribute('target')
    a.removeAttribute('ping')
    if (!href || !LINK_SCHEMES.test(href)) continue
    const n = links.length + 1
    links.push({ n, url: href })
    a.setAttribute('data-k2-link', String(n))
    const sup = doc.createElement('sup')
    sup.className = 'k2-link-ref'
    sup.textContent = `[${n}]`
    a.appendChild(sup)
  }
  const frameHtml =
    '<!doctype html><html><head><meta charset="utf-8"><style>' +
    briefStylesheet(tokens) +
    '</style></head><body class="k2-brief">' +
    doc.body.innerHTML +
    '</body></html>'
  return { frameHtml, links }
}
