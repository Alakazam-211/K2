// The ONE place K2 renders HTML it did not author. Every `<iframe srcDoc>` in
// the renderer goes through here so it always gets the per-frame CSP meta and
// the profile's sandbox tokens (prd-html-frame-csp-v1.md F6). A source-scan
// test (`html-frame-surfaces.test.ts`) fails if a new srcDoc/iframe skips it.
import { useMemo } from 'react'
import { frameSandbox, frameSrcDoc, type FrameProfile } from '@/lib/frame-csp'

export function HtmlFrame({
  html,
  profile,
  title,
  className,
  testId,
}: {
  html: string
  profile: FrameProfile
  title: string
  className?: string
  testId?: string
}): React.JSX.Element {
  const srcDoc = useMemo(() => frameSrcDoc(html, profile), [html, profile])
  return (
    <iframe
      title={title}
      srcDoc={srcDoc}
      sandbox={frameSandbox(profile)}
      referrerPolicy="no-referrer"
      className={className}
      data-testid={testId}
    />
  )
}
