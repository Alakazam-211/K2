// Add a workspace the way the nav's Add Workspace button does
// (`Sidebar.tsx` / `IconRail.tsx` `handleAddProject`, kept in step with
// them): the folder pick on the window's server, then the Add Workspace
// dialog, whose confirm adds it and runs the ingest. Used by the setup
// checklist's first-agent row.

import { pickWorkspaceFolder } from '@/lib/pick-workspace-folder'
import { addWorkspaceWithIngest } from '@/lib/add-workspace'
import { useAddWorkspaceDialogStore } from '@/stores/add-workspace-dialog'

export async function startAddWorkspace(): Promise<void> {
  const folderPath = await pickWorkspaceFolder()
  if (!folderPath) return
  useAddWorkspaceDialogStore.getState().open({
    path: folderPath,
    onConfirm: async ({ seedWiki, seedAgentsMd, fanout }) => {
      await addWorkspaceWithIngest(folderPath, { seedWiki, seedAgentsMd, fanout })
    },
  })
}
