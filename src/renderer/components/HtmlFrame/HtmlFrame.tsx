// The ONE place K2 renders HTML it did not author. Every `<iframe srcDoc>` in
// the renderer goes through here so it always gets the per-frame CSP meta and
// the profile's sandbox tokens (prd-html-frame-csp-v1.md F6). A source-scan
// test (`html-frame-surfaces.test.ts`) fails if a new srcDoc/iframe skips it.
//
// The `widget` profile (custom Garden widgets, prd-zen-user-widgets-v2 UW13)
// also takes the bundle's `nonce` and K2's `prelude` (theme style, runtime
// and libraries, already nonced), and hands the caller the iframe element
// and its `load` so the frame host can open its private message port.
import { useMemo } from 'react'
import { frameSandbox, frameSrcDoc, type FrameProfile } from '@/lib/frame-csp'

export function HtmlFrame({
  html,
  profile,
  title,
  className,
  testId,
  nonce,
  prelude,
  frameRef,
  onLoad,
  style,
}: {
  html: string
  profile: FrameProfile
  title: string
  className?: string
  testId?: string
  /** `widget` only: the per-load nonce every allowed script carries. */
  nonce?: string
  /** `widget` only: K2's nonced head content, before the widget's HTML. */
  prelude?: string
  /** The iframe element (null on unmount). */
  frameRef?: (el: HTMLIFrameElement | null) => void
  onLoad?: (el: HTMLIFrameElement) => void
  style?: React.CSSProperties
}): React.JSX.Element {
  const srcDoc = useMemo(() => frameSrcDoc(html, profile, { nonce, prelude }), [html, profile, nonce, prelude])
  return (
    <iframe
      ref={frameRef}
      title={title}
      srcDoc={srcDoc}
      sandbox={frameSandbox(profile)}
      referrerPolicy="no-referrer"
      className={className}
      data-testid={testId}
      data-frame-profile={profile}
      onLoad={onLoad ? (e) => onLoad(e.currentTarget) : undefined}
      style={style}
    />
  )
}
