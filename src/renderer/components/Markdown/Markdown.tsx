import ReactMarkdown, { type Options } from 'react-markdown'

/**
 * Drop-in wrapper around `react-markdown` that bypasses the default
 * URL transformer's whitelist so app-specific URL schemes
 * (`message:`, `slack:`, `vscode:`, `cursor:`, `obsidian:`,
 * `facetime:`, etc.) survive into the rendered `<a>` href.
 *
 * react-markdown v9+ ships a `defaultUrlTransform` that only allows
 * `http`, `https`, `mailto`, `tel`, `irc`, `xmpp`. Anything else gets
 * stripped — the rendered link has an empty href, so clicking it
 * silently navigates the webview to the current page (= the whole
 * K2 window appears to "reload"). The capture-phase click handler
 * in `lib/external-link-handler.ts` only sees the empty href and has
 * nothing to forward to LaunchServices.
 *
 * The transform here keeps react-markdown's protection against
 * `javascript:` and `data:` (still genuine XSS vectors inside the
 * webview) but otherwise passes URLs through. The
 * `external-link-handler` then routes any non-http(s) scheme to the
 * macOS opener so Mail / Slack / VSCode / etc. open natively.
 *
 * Use this everywhere you'd previously import `react-markdown`
 * directly. Same prop shape — no other call-site changes needed.
 */

function safeUrlTransform(url: string): string {
  // Strip the same dangerous schemes the default transform blocks.
  // Everything else flows through unmodified — the click handler in
  // lib/external-link-handler.ts decides whether to navigate inside
  // the webview (http/https) or hand off to the OS (everything else).
  const lower = url.trim().toLowerCase()
  if (lower.startsWith('javascript:') || lower.startsWith('data:') || lower.startsWith('vbscript:')) {
    return ''
  }
  return url
}

export default function Markdown(props: Options): React.JSX.Element {
  return <ReactMarkdown urlTransform={safeUrlTransform} {...props} />
}
