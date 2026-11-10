// The Mail page's frame for mail HTML: K2's HtmlFrame with the `inert`
// profile (no script, no network, no same-origin; img-src data: blob: only).
// @k2/mail-reader hands it a full document it already cleaned (the daemon
// cleaned it first).

import { HtmlFrame } from '@/components/HtmlFrame/HtmlFrame'

export function InertMailFrame({ html, title }: { html: string; title: string }): React.JSX.Element {
  return <HtmlFrame title={title} html={html} profile="inert" className="w-full h-full border-0" testId="mail-reader-html" />
}

/** The `renderHtml` slot @k2/mail-reader takes. */
export function renderInertMail(props: { html: string; title: string }): React.JSX.Element {
  return <InertMailFrame html={props.html} title={props.title} />
}
