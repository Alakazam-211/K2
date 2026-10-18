// Fail loud: every DOM overlay that can sit over the Browser tab's native
// page either claims its box in the overlay arbiter (lib/browser-overlay.ts)
// or says here why it never covers a live Browser page. A new portal, fixed
// layer, menu or dialog that does neither fails this test.
import { readdirSync, readFileSync, statSync } from 'node:fs'
import { join, relative } from 'node:path'
import { describe, expect, it } from 'vitest'

const ROOT = join(__dirname, '..')

/** Source shapes that make a layer above the page. */
const OVERLAY_PATTERNS: RegExp[] = [
  /createPortal\(/,
  /\bfixed inset-0\b/,
  /position:\s*'fixed'/,
  /className=["`{][^"`]*?\bfixed\b/,
  /role="(menu|listbox|dialog)"/,
]

/** A file that uses one of these claims (or renders a primitive that does). */
const CLAIMS: RegExp[] = [
  /useBrowserOverlay\(/,
  /<BrowserOverlayClaim\b/,
  /beginBrowserMotion\(/,
  /<DialogScrim\b/,
  /<DialogFrame\b/,
  // hooks/useAnchoredMenu.ts claims the menu it portals.
  /useAnchoredMenu\b/,
]

/**
 * Overlays that never cover a LIVE Browser page, and why. The page is live
 * only on a room page (Agents / Home with a room), with Settings closed, Zen
 * off, and its own tab visible (`browserPaneVisible` + `useIsTabVisible`).
 */
const NO_CLAIM_NEEDED: Record<string, string> = {
  // Page-level shells: they replace the room, they do not float over it.
  'App.tsx': 'the app root frame itself',
  'components/ConnectionGate.tsx': 'connection gate replaces the window before a room exists',
  'components/RemoteSignIn.tsx': 'sign-in screen replaces the window before a room exists',
  'components/BackgroundTerminalSpawner.tsx': 'off-screen terminal host (negative offset, invisible)',
  'components/Home/room/HomeRoomsHost.tsx': 'hosts the rooms themselves (the Browser lives inside it)',
  'components/AgentPane/PinnedChatRetainer.tsx': 'docks pinned chats into their pane slots, not above other panes',
  // Inline in a column that never reaches a pane.
  'components/Home/HomeAddPanels.tsx': 'Add Agent / Add Workspace panels open inside the Home sidebar column',
  'components/ui/SearchableAgentList.tsx': 'an inline list inside its host panel (the host claims if it floats)',
  // Settings / Projects / Feedback / Wiki pages: the room (and every
  // Browser tab) is hidden while they are open.
  'components/Settings/SettingsSearchModal.tsx': 'Settings is open: Browser tabs are hidden',
  'components/Settings/sections/AddConnectionPicker.tsx': 'Settings is open: Browser tabs are hidden (an inline panel in Connections)',
  'components/Settings/sections/ApiTokensSection.tsx': 'Settings is open: Browser tabs are hidden',
  'components/Settings/sections/DataSourcesBatchDialog.tsx': 'Settings is open: Browser tabs are hidden',
  'components/Settings/sections/DataSourceRow.tsx': 'Settings is open: Browser tabs are hidden',
  'components/Settings/sections/FanoutConfirmModal.tsx': 'Settings is open: Browser tabs are hidden',
  'components/Settings/sections/HeartbeatsSection.tsx': 'Settings is open: Browser tabs are hidden',
  'components/Settings/sections/EmailHostingSection.tsx': 'Settings is open: Browser tabs are hidden',
  'components/Settings/shared/AgentAccountsColumn.tsx': 'Settings is open: Browser tabs are hidden',
  'components/Settings/shared/LlmLoginSheet.tsx': 'Settings is open: Browser tabs are hidden',
  'components/Projects/ProjectsPage.tsx': 'Projects page: the room is hidden',
  'components/Projects/ProjectChatPanel.tsx': 'Projects page: the room is hidden',
  'components/Feedback/FeedbackPage.tsx': 'Feedback page: the room is hidden',
  'components/Feedback/TicketBoard.tsx': 'Feedback page: the room is hidden',
  'components/Feedback/TicketCard.tsx': 'Feedback page: the room is hidden',
  'components/Feedback/TicketWindow.tsx': 'Feedback page: the room is hidden',
  'components/Feedback/BriefFrame.tsx': 'Feedback page: the room is hidden',
  'components/Feedback/FeedbackItemView.tsx': 'Feedback page: the room is hidden',
  'components/Wiki/WikiPage.tsx': 'Wiki page: the room is hidden',
  // Zen covers the window; PageLive is false so Browser tabs hide.
  'components/Zen/ZenAddAgent.tsx': 'Zen is shown: Browser tabs are hidden',
  'components/Zen/ZenChromeCluster.tsx': 'Zen is shown: Browser tabs are hidden',
  'components/Zen/ZenRoot.tsx': 'Zen is shown: Browser tabs are hidden',
  'components/Zen/widgets/ZenOverlay.tsx': 'Zen is shown: Browser tabs are hidden',
  'components/Zen/widgets/ZenGardenEmptyWidget.tsx': 'Zen is shown: Browser tabs are hidden',
  'components/Zen/widgets/ZenGuestRoomsSheet.tsx': 'Zen is shown: Browser tabs are hidden (a Garden widget\'s guests.setRooms confirm)',
}

function walk(dir: string, out: string[]): void {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name)
    if (statSync(p).isDirectory()) {
      walk(p, out)
    } else if (/\.tsx$/.test(name) && !/\.test\.tsx$/.test(name)) {
      out.push(p)
    }
  }
}

function overlayFiles(): { path: string; claims: boolean }[] {
  const files: string[] = []
  walk(ROOT, files)
  return files
    .map((p) => ({ path: relative(ROOT, p), src: readFileSync(p, 'utf8') }))
    .filter(({ src }) => OVERLAY_PATTERNS.some((re) => re.test(src)))
    .map(({ path, src }) => ({ path, claims: CLAIMS.some((re) => re.test(src)) }))
}

describe('Browser overlay coverage', () => {
  it('finds overlay sources (the scan is not empty)', () => {
    expect(overlayFiles().length).toBeGreaterThan(20)
  })

  it('every overlay claims over the Browser page or says why it never covers a live one', () => {
    const unexplained = overlayFiles()
      .filter((f) => !f.claims && !(f.path in NO_CLAIM_NEEDED))
      .map((f) => f.path)
    expect(unexplained).toEqual([])
  })

  it('every exemption still names a real overlay that does not claim', () => {
    const files = new Map(overlayFiles().map((f) => [f.path, f]))
    const stale = Object.keys(NO_CLAIM_NEEDED).filter((p) => {
      const f = files.get(p)
      return !f || f.claims
    })
    expect(stale).toEqual([])
  })
})
