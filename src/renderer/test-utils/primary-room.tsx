// Render a room component inside the REAL primary room (Home M3): today's
// tabs, projects and active-agents stores on `primaryScope()`. For suites
// that run the real stores; suites that mock them build a `testRoom`.

import type { ReactElement } from 'react'
import type { RenderOptions, RenderResult } from '@testing-library/react'
import { primaryRoom } from '@/stores/room'
// The primary room reads the window's projects and active-agents stores;
// they register themselves with it when loaded.
import '@/stores/projects'
import '@/stores/active-agents'
import '@/stores/presence'
import '@/stores/active'
import { renderInRoom } from '@/test-utils/room'

export function renderInPrimaryRoom(ui: ReactElement, options?: RenderOptions): RenderResult {
  return renderInRoom(primaryRoom(), ui, options)
}
