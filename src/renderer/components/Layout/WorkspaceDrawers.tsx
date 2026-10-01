// The workspace drawers (left + right TabbedPanels: Files, Changes, chat
// history, Workspace). One definition for every room shell — the Agents
// page, Home (same shell, Home roster in the sidebar), and Focus windows.

import FileTree from '@/components/FileTree/FileTree'
import ChangesPanel from '@/components/ChangesPanel/ChangesPanel'
import ChatHistory from '@/components/ChatHistory/ChatHistory'
import WorkspacePanel from '@/components/WorkspacePanel/WorkspacePanel'
import TabbedPanel from '@/components/TabbedPanel/TabbedPanel'
import { usePanelsStore } from '@/stores/panels'

export function LeftPanelContent({ rootPath, header }: { rootPath?: string; header?: React.ReactNode }): React.JSX.Element {
  const tabs = usePanelsStore((s) => s.leftPanelTabs)
  const activeTab = usePanelsStore((s) => s.leftPanelActiveTab)
  const setActiveTab = usePanelsStore((s) => s.setLeftPanelActiveTab)
  const width = usePanelsStore((s) => s.leftPanelWidth)
  const setWidth = usePanelsStore((s) => s.setLeftPanelWidth)

  if (tabs.length === 0) return <></>

  return (
    <TabbedPanel
      tabs={tabs}
      activeTab={activeTab}
      onTabChange={setActiveTab}
      width={width}
      onWidthChange={setWidth}
      resizeSide="right"
      header={header}
    >
      {activeTab === 'files' && rootPath && <FileTree rootPath={rootPath} />}
      {activeTab === 'changes' && <ChangesPanel />}
      {/* #7: bind ChatHistory to THIS panel's host workspace (rootPath),
          not the globally-active workspace. Without the prop it would
          resolve from global pointers and show another workspace's chats. */}
      {activeTab === 'history' && <ChatHistory projectPath={rootPath} />}
      {activeTab === 'workspace' && <WorkspacePanel />}
    </TabbedPanel>
  )
}

export function RightPanelContent({ rootPath, header }: { rootPath?: string; header?: React.ReactNode }): React.JSX.Element {
  const tabs = usePanelsStore((s) => s.rightPanelTabs)
  const activeTab = usePanelsStore((s) => s.rightPanelActiveTab)
  const setActiveTab = usePanelsStore((s) => s.setRightPanelActiveTab)
  const width = usePanelsStore((s) => s.rightPanelWidth)
  const setWidth = usePanelsStore((s) => s.setRightPanelWidth)

  if (tabs.length === 0) return <></>

  return (
    <TabbedPanel
      tabs={tabs}
      activeTab={activeTab}
      onTabChange={setActiveTab}
      width={width}
      onWidthChange={setWidth}
      resizeSide="left"
      header={header}
    >
      {activeTab === 'files' && rootPath && <FileTree rootPath={rootPath} />}
      {activeTab === 'changes' && <ChangesPanel />}
      {/* #7: bind ChatHistory to THIS panel's host workspace (rootPath),
          not the globally-active workspace. See LeftPanelContent. */}
      {activeTab === 'history' && <ChatHistory projectPath={rootPath} />}
      {activeTab === 'workspace' && <WorkspacePanel />}
    </TabbedPanel>
  )
}
