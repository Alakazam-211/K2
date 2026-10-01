// A window-level surface about the window's OWN server (Settings, the
// Projects page, Feedback, the AI file editor) that embeds a room component
// (a terminal, a file viewer, a browser, the heartbeats drawer) runs it in
// the primary room — explicitly, at the mount (MS2). Never used inside a
// room: a room's own components get their room from its shell.

import type { ReactNode } from 'react'
import { RoomProvider } from '@/components/Room/RoomContext'
import { primaryRoom } from '@/stores/room'

export function PrimaryRoom({ children }: { children: ReactNode }): React.JSX.Element {
  return <RoomProvider room={primaryRoom()}>{children}</RoomProvider>
}
