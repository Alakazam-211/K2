/**
 * The stores that first load with the app itself (App.tsx), which the
 * connection gate imports only once the daemon answers. Like
 * boot-stores.ts, but for stores the entry (index.tsx) never imports:
 * their first loads ran when App.tsx's imports were evaluated, so
 * App.tsx calls `bootAppStores()` there.
 *
 * Kept apart from boot-stores.ts so the entry does not pull these
 * stores in before the gate.
 */
import { bootPanels } from './stores/panels'

export function bootAppStores(): void {
  bootPanels()
}
