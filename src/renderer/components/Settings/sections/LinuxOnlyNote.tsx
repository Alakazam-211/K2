// The one note a Linux-only core shows when the daemon says it can't run
// here (Email Hosting, Chat / Voice, Database, Apps): one plain sentence in
// an info Callout, under the page header, instead of an all-caps banner.
// The page decides from the daemon's `supported` flag, never the platform.

import React from 'react'
import { Callout } from '@/components/ui'

/** "<what> runs on Linux servers. …" `what` is the feature in sentence
 *  case, as a person would name it: "Email hosting", "Chat and voice". */
export function LinuxOnlyNote({ what, testId = 'linux-only-note' }: { what: string; testId?: string }): React.JSX.Element {
  return (
    <Callout tone="info" role="note" className="type-hint leading-relaxed" data-testid={testId}>
      {what} runs on Linux servers. On this Mac the page shows sample data, so you can see how it works; connect to a
      Linux server to set it up.
    </Callout>
  )
}
