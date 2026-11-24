// Wiki location: when this workspace's wiki is a link to a folder elsewhere
// (an Obsidian vault, say), the owner approves that location once so API
// keys and agent templates may add notes to it. The daemon owns the rule
// and the approval (`/cli/wiki/location`); this row only shows it and sends
// the owner's Approve. Nothing shows for a wiki that is a plain folder.

import React, { useCallback, useEffect, useState } from 'react'
import { Button } from '@/components/ui'
import { approveWikiLocation, fetchWikiLocation, type WikiLocation } from '@/components/Wiki/wiki-api'
import { SaveError, SettingRow, SettingsGroup, useSaveStatus } from '../controls/SettingControls'

export function WikiLocationPanel({ projectPath }: { projectPath: string }): React.JSX.Element | null {
  const [loc, setLoc] = useState<WikiLocation | null>(null)
  const save = useSaveStatus()

  const load = useCallback(() => {
    fetchWikiLocation(projectPath).then(setLoc, () => setLoc(null))
  }, [projectPath])
  useEffect(load, [load])

  if (!loc || !loc.linked) return null
  const needsApproval = loc.status === 'not_approved' || loc.status === 'changed'
  const approve = (): void => {
    approveWikiLocation(projectPath).then(
      (next) => {
        setLoc(next)
        save.saved()
      },
      (err) => save.failed(err),
    )
  }

  return (
    <SettingsGroup title="Wiki location" card settingId="projects.wiki-location">
      <SettingRow
        label={loc.target ?? 'A folder that isn’t there'}
        hint={loc.lines[0] ?? ''}
        testId="wiki-location-row"
        below={<SaveError state={save.state} />}
      >
        {needsApproval && (
          <Button onClick={approve} data-testid="wiki-location-approve">
            Approve
          </Button>
        )}
      </SettingRow>
    </SettingsGroup>
  )
}
