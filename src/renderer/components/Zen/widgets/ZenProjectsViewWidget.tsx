// Rosson 2026-10-04 — the nav rail's Projects view in Garden 1: one
// centred line, "Coming soon — or build a new one yourself!". "Build a new
// one yourself" is the empty Garden's Ask my agent (`ZenAskMyAgent`): a
// chooser of agents on this computer, then that agent's conversation in
// place with a drafted message (never sent).

import type { ZenWidgetProps } from '../zen-registry'
import { ZEN_GARDEN_HINT, ZenAskMyAgent } from './ZenGardenEmptyWidget'

export const ZEN_PROJECTS_SOON = 'Coming soon'
export const ZEN_PROJECTS_BUILD = 'build a new one yourself'
/** The whole line, as Rosson wrote it. */
export const ZEN_PROJECTS_TEXT = `${ZEN_PROJECTS_SOON} — or ${ZEN_PROJECTS_BUILD}!`

/** The draft "build a new one yourself" puts in the message box. */
export function zenProjectsAskDraft(garden: { id: string; name: string }): string {
  return `Build me a Zen Projects view in my Garden "${garden.name}" (id ${garden.id}) that shows: \n${ZEN_GARDEN_HINT}`
}

export function ZenProjectsViewWidget(props: ZenWidgetProps): React.JSX.Element {
  return (
    <ZenAskMyAgent
      {...props}
      kind="projects-view"
      draft={zenProjectsAskDraft}
      intro={(ask, phase) => (
        <div data-zen-projects-soon="" style={{ fontSize: '1.15em', fontWeight: 600 }}>
          {ZEN_PROJECTS_SOON} — or{' '}
          <button
            type="button"
            data-zen-projects-build=""
            disabled={phase !== 'empty'}
            onClick={ask}
            className="cursor-pointer disabled:cursor-default"
            style={{
              padding: 0,
              font: 'inherit',
              color: 'var(--zen-accent)',
              background: 'transparent',
              textDecoration: 'underline',
              textUnderlineOffset: 3,
            }}
          >
            {ZEN_PROJECTS_BUILD}
          </button>
          !
        </div>
      )}
    />
  )
}
