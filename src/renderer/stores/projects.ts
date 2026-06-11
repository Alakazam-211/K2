import { create } from 'zustand'
import { emit } from '@tauri-apps/api/event'
// Plan B — projects/workspaces/sections are host-aware daemon data: route
// them through the `/cli/*` HTTP layer (local OR remote) instead of the
// localhost-pinned Tauri `projects_*`/`workspaces_*`/`sections_*` invoke
// proxy. The old Tauri shims emitted `sync:projects` from Rust on each
// mutation so other windows re-fetch; we now re-emit that event from the
// renderer after each successful mutation (see `emitProjectsChanged`).
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
// #672 — canonical daemon-owned Active. Gestures route through the
// activate/pin/dismiss daemon routes (capability-gated); the renderer no
// longer derives or mutates Active as truth — the daemon is authoritative.
import { serverSupports } from '@/lib/server-capabilities'
import { useActiveStore } from '@/stores/active'
// Phase 2 Unit 7a — settings live in the daemon.
import { settingsGet, settingsUpdate } from '@/lib/daemon-settings'
// Phase 2.5 fix (finding #547) — daemon-reconnect retry bus.
import { onDaemonConnected } from '@/lib/daemon-reconnect'
// #625 — re-restore the active project/workspace against the NEW host on
// a host switch.
import { onActiveHostChange } from '@/stores/connect-host'
import { onProjectsChanged } from '@/stores/session-events'
import { useGitInitDialogStore } from './git-init-dialog'
import { useToastStore } from './toast'
import { useTabsStore, ensurePinnedAgentTabForMode, registerActiveProjectIdGetter, registerActivateProject } from './tabs'
import { useFocusGroupsStore } from './focus-groups'
import { useSettingsStore } from './settings'

// #657 — hand tabs.ts a lazy reader for `activeProjectId` so the
// dismiss-reap path can honor "never reap the foreground workspace"
// without a static projects→tabs→projects import cycle. Registered at
// module load; safe to call before the store has any projects (returns
// the initial `null`).
registerActiveProjectIdGetter(() => useProjectsStore.getState().activeProjectId)

// #672 — register the canonical open/attach⇒activate gesture so tabs.ts's
// chat-surfacing chokepoint (`subscribeForActiveWorkspace`) can call it
// without a static projects→tabs→projects import cycle (PRD §4.3.1).
// Defined below (hoisted function); safe to reference here.
registerActivateProject((projectId: string) => activateProject(projectId))

// Debounce touchInteraction to avoid excessive DB writes (5 min per project)
const TOUCH_DEBOUNCE_MS = 5 * 60 * 1000
const _lastTouchMap = new Map<string, number>()

// #672 — canonical-Active gesture: opening/focusing a workspace is an
// ACTIVATION (PRD §4.3.1 — the load-bearing invariant that makes daemon-
// side Active-only reaping safe). We POST projects/activate to the daemon,
// which bumps lastInteractionAt + recomputes the canonical set + broadcasts
// the delta back. DEDUPED on the foreground project id (mirror of
// TerminalPane's `lastSentActiveRef`): only POST when the foreground
// workspace actually changes, so per-frame/focus churn can never reproduce
// the #603 set_active storm. Optimistic local echo into useActiveStore for
// snappiness; reconciles to the daemon snapshot/delta.
let _lastActivatedProjectId: string | null = null

/** Reset the activate dedup so the next setActiveProject re-POSTs even to
 *  the same id (used on host switch — a new daemon must see the activation
 *  fresh). */
function resetActivateDedup(): void {
  _lastActivatedProjectId = null
}

/**
 * The open/attach⇒activate gesture (PRD §4.3.1). Call from every path that
 * surfaces a workspace's chat to the client. Capability-gated + deduped.
 */
export function activateProject(projectId: string): void {
  if (!projectId) return
  if (!serverSupports('canonical-active')) return
  // Dedup: skip when the foreground project is unchanged.
  if (_lastActivatedProjectId === projectId) return
  _lastActivatedProjectId = projectId
  // Optimistic echo — the daemon's active_changed delta reconciles this.
  useActiveStore.getState().echoActive(projectId)
  void daemonCliPost('projects/activate', { projectId }).catch((e) =>
    console.warn('[projects] activate failed:', e),
  )
}

/** Phase 2.5 fix (finding #547) — flips on the first successful
 *  `fetchProjects` call. Gates `lastActiveProjectId` /
 *  `lastActiveWorkspaceId` writes so an early-boot workspace
 *  switch can't overwrite real values with the user's pre-restore
 *  selection. See panels.ts for the longer rationale. */
let hasLoadedFromDaemon = false

/**
 * Plan B cross-window sync: the old Tauri `projects_*` / `sections_*`
 * mutation commands emitted `sync:projects` from Rust so OTHER windows
 * (focus windows, second main window) re-fetch their project list. Now
 * that the renderer talks to the daemon directly, that Rust-side emit no
 * longer fires — so we emit the SAME event from the renderer after each
 * successful mutation. `useWindowSync.ts` listens for it and calls
 * `fetchProjects()`. Fire-and-forget; a failed emit (non-Tauri/web) is
 * non-fatal — only cross-window refresh is affected.
 */
function emitProjectsChanged(): void {
  void emit('sync:projects').catch((e) =>
    console.warn('[projects] sync:projects emit failed:', e),
  )
}

interface Workspace {
  id: string
  projectId: string
  sectionId: string | null
  type: string
  branch: string | null
  name: string
  tabOrder: number
  worktreePath: string | null
  navVisible: number
  createdAt: number
}

export interface WorkspaceSection {
  id: string
  projectId: string
  name: string
  color: string | null
  isCollapsed: number
  tabOrder: number
  createdAt: number
}

interface Project {
  id: string
  name: string
  path: string
  color: string
  tabOrder: number
  lastOpenedAt: number | null
  worktreeMode: number
  iconUrl: string | null
  focusGroupId: string | null
  pinned: number
  manuallyActive: number
  lastInteractionAt: number | null
  createdAt: number
  agentEnabled: number
  heartbeatEnabled: number
  agentMode: string // 'off' | 'agent' | 'manager' | 'coordinator' | 'pod'
  stateId: string | null
  heartbeatMode: string // 'off' | 'scheduled' | 'hourly'
  heartbeatSchedule: string | null // JSON
  heartbeatLastFire: string | null // ISO8601
}

export interface ProjectWithWorkspaces extends Project {
  workspaces: Workspace[]
  sections: WorkspaceSection[]
}

interface ProjectsState {
  projects: ProjectWithWorkspaces[]
  activeProjectId: string | null
  activeWorkspaceId: string | null

  fetchProjects: () => Promise<void>
  addProject: (path: string) => Promise<void>
  removeProject: (id: string) => Promise<void>
  setActiveProject: (id: string | null) => void
  setActiveWorkspace: (projectId: string, workspaceId: string) => void
  reorderProjects: (ids: string[]) => Promise<void>
  renameProject: (id: string, name: string) => Promise<void>
  createSection: (projectId: string, name: string, color?: string) => Promise<void>
  deleteSection: (id: string) => Promise<void>
  renameSection: (id: string, name: string) => Promise<void>
  updateSection: (id: string, updates: { name?: string; color?: string | null; isCollapsed?: number }) => Promise<void>
  assignWorkspaceToSection: (workspaceId: string, sectionId: string | null) => Promise<void>
  touchInteraction: (projectId: string) => void
  setManuallyActive: (projectId: string, active: boolean) => Promise<void>
}

export const useProjectsStore = create<ProjectsState>((set, get) => ({
  projects: [],
  activeProjectId: null,
  activeWorkspaceId: null,

  fetchProjects: async () => {
    try {
      // GET query params are snake_case (the daemon reads `project_id`);
      // the camelCase Project/Workspace/Section response shapes match the
      // Rust structs' `#[serde(rename_all = "camelCase")]` so no remap.
      const projectList = await daemonCliGet<Project[]>('projects/list')

      // Fetch workspaces and sections for each project
      const projectsWithWorkspaces: ProjectWithWorkspaces[] = await Promise.all(
        projectList.map(async (project: Project) => {
          const ws = await daemonCliGet<Workspace[]>('workspaces/list', { project_id: project.id })
          let secs: WorkspaceSection[] = []
          try {
            secs = await daemonCliGet<WorkspaceSection[]>('sections/list', { project_id: project.id })
          } catch {
            // sections table might not exist yet
          }
          return {
            ...project,
            workspaces: ws,
            sections: secs
          }
        })
      )

      // Preserve object identity for unchanged projects to avoid unnecessary re-renders
      const prev = get().projects
      const merged = projectsWithWorkspaces.map((next) => {
        const existing = prev.find((p) => p.id === next.id)
        if (existing && JSON.stringify(existing) === JSON.stringify(next)) {
          return existing // same reference — React skips re-render
        }
        return next
      })
      set({ projects: merged })

      // If active project was deleted (e.g. in another window), clear selection
      const state = get()
      if (state.activeProjectId && !projectsWithWorkspaces.find((p) => p.id === state.activeProjectId)) {
        if (projectsWithWorkspaces.length > 0) {
          const first = projectsWithWorkspaces[0]
          set({ activeProjectId: first.id, activeWorkspaceId: first.workspaces[0]?.id ?? null })
        } else {
          set({ activeProjectId: null, activeWorkspaceId: null })
        }
      }

      // If we have projects but no active one, try to restore last session
      if (!get().activeProjectId && projectsWithWorkspaces.length > 0) {
        let restoredProject = null
        let restoredWorkspaceId: string | null = null

        // 0.39.0: read from the already-loaded settings store instead of
        // re-invoking `settings_get`. Saves ~20-50ms on boot. The store
        // initializes via `useSettingsStore.getState().fetchSettings()`
        // at module import (settings.ts:312), so by the time projects
        // hydrate, the cached snapshot is usually ready. If the store
        // hasn't finished its initial fetch yet (rare boot-race), fall
        // back to the direct invoke so we don't lose the last-session
        // restore — but in the common case we skip the second roundtrip.
        try {
          const settingsState = useSettingsStore.getState()
          let lastActiveProjectId = settingsState.lastActiveProjectId
          let lastActiveWorkspaceId = settingsState.lastActiveWorkspaceId
          if (!settingsState.loaded) {
            // Settings hydration hasn't completed yet — invoke directly
            // so we don't miss the restore. This path runs once per app
            // session at most.
            const settings = await settingsGet()
            lastActiveProjectId = settings.lastActiveProjectId ?? null
            lastActiveWorkspaceId = settings.lastActiveWorkspaceId ?? null
          }
          if (lastActiveProjectId) {
            const savedProject = projectsWithWorkspaces.find((p) => p.id === lastActiveProjectId)
            if (savedProject) {
              restoredProject = savedProject
              // Restore exact workspace, or fall back to first
              restoredWorkspaceId = lastActiveWorkspaceId
                && savedProject.workspaces.find((w) => w.id === lastActiveWorkspaceId)
                ? lastActiveWorkspaceId
                : savedProject.workspaces[0]?.id ?? null
            }
          }
        } catch { /* settings read failed, fall back to first */ }

        // Fall back to first project if restore failed
        if (!restoredProject) {
          restoredProject = projectsWithWorkspaces[0]
          restoredWorkspaceId = restoredProject.workspaces[0]?.id ?? null
        }

        set({
          activeProjectId: restoredProject.id,
          activeWorkspaceId: restoredWorkspaceId
        })

        // Load saved layout for the restored workspace.
        //
        // #658 — cold-boot ordering. AWAIT the layout restore before
        // running the pinned-tab ensure. On a cold boot the in-memory
        // `workspaceLayouts` cache is still empty (it loads async via
        // `loadWorkspaceSessionsFromDb`), so `loadLayoutForWorkspace`
        // takes its DB-load branch and resolves only after
        // `restoreLayout` has set tabs + activeTabId + the restored chat
        // sessionId. Without the await, `ensurePinnedAgentTabForMode`'s
        // deferred `ensureSystemAgentTabs` ran FIRST, found no restored
        // pinned tab, and created a fresh sessionless one that never
        // became the active tab — so the landed-on workspace's Chat tab
        // sat invisible (isTabVisible=false) and never spawned its
        // session until a manual refresh. Awaiting makes restoreLayout
        // win; the subsequent ensure then reconciles the already-active,
        // sessionId-carrying tab in place (idempotent).
        if (restoredWorkspaceId) {
          const workspace = restoredProject.workspaces.find((w) => w.id === restoredWorkspaceId)
          const cwd = workspace?.worktreePath ?? restoredProject.path ?? '~'
          await useTabsStore.getState().loadLayoutForWorkspace(restoredProject.id, restoredWorkspaceId, cwd)
          ensurePinnedAgentTabForMode(restoredProject.agentMode, restoredProject.path)
        }
      }
      // Phase 2.5 fix (finding #547): flip the persist gate ONLY
      // after a successful projects fetch. Subsequent
      // setActiveProject/setActiveWorkspace calls (the writers below)
      // now allow their `settingsUpdate` calls.
      hasLoadedFromDaemon = true
    } catch (err) {
      console.error('[projects] fetchProjects failed:', err)
    }
  },

  addProject: async (path: string) => {
    try {
      // Capture the new project's ID from the backend result (untagged enum:
      // NeedsGitInit has needsGitInit field, Project has id field)
      const result = await daemonCliPost<Record<string, unknown>>('projects/add-from-path', { path })
      // The old Tauri `projects_add_from_path` emitted `sync:projects`. A
      // git-init-needed result short-circuits below WITHOUT a DB write, so
      // only emit on the real add path (after fetchProjects, below).

      // Check if the folder needs git initialization
      if (result && 'needsGitInit' in result) {
        console.log('[projects] Opening git init dialog for:', result.path)
        useGitInitDialogStore.getState().open(result.path as string, result.name as string)
        return
      }

      // The result IS the Project object — extract its ID for reliable lookup
      const newProjectId = (result as any)?.id as string | undefined

      // Stash the workspace we're leaving BEFORE fetchProjects changes active IDs
      const preState = get()
      const tabsStore = useTabsStore.getState()

      if (preState.activeProjectId && preState.activeWorkspaceId) {
        tabsStore.stashWorkspace(`${preState.activeProjectId}:${preState.activeWorkspaceId}`)
      } else if (useTabsStore.getState().tabs.length > 0) {
        tabsStore.clearAllTabs()
      }

      // Mirror the old Tauri `projects_add_from_path` Rust-side
      // `sync:projects` emit so other windows pick up the new project.
      emitProjectsChanged()

      await get().fetchProjects()

      const state = get()
      // Find the new project by its ID from the backend result
      const newProject = newProjectId
        ? state.projects.find((p) => p.id === newProjectId)
        : state.projects[state.projects.length - 1]

      if (newProject) {
        // Assign to the active focus group so it's visible in the current view
        const focusState = useFocusGroupsStore.getState()
        const targetGroupId = focusState.focusGroupsEnabled ? focusState.activeFocusGroupId : null
        if (targetGroupId) {
          try {
            await daemonCliPost('focus-groups/assign', { projectId: newProject.id, focusGroupId: targetGroupId })
            // The old Tauri `focus_groups_assign_project` emitted BOTH
            // `sync:focus-groups` and `sync:projects` (the project's
            // focusGroupId changed). Mirror both.
            void emit('sync:focus-groups').catch(() => {})
            emitProjectsChanged()
            // Optimistic local update so sidebar shows it immediately
            set({
              projects: get().projects.map((p) =>
                p.id === newProject.id ? { ...p, focusGroupId: targetGroupId } : p
              )
            })
          } catch {
            // Non-fatal — project was still added
          }
        }

        const newWorkspaceId = newProject.workspaces[0]?.id ?? null
        set({
          activeProjectId: newProject.id,
          activeWorkspaceId: newWorkspaceId
        })

        if (newWorkspaceId) {
          const cwd = newProject.workspaces[0]?.worktreePath ?? newProject.path ?? '~'
          const newKey = `${newProject.id}:${newWorkspaceId}`
          // #681 (Bug A) — AWAIT restoreWorkspace before ensuring the
          // pinned tabs (mirrors the #658 cold-boot ordering). For a
          // brand-new workspace restoreWorkspace falls through to the slow
          // path → loadLayoutForWorkspace clears tabs + sets the workspace
          // key; awaiting guarantees ensurePinnedAgentTabForMode then
          // creates the Chat + Inbox tabs (Chat active) deterministically,
          // so they show on first open instead of only after a switch-away.
          await tabsStore.restoreWorkspace(newKey, cwd)
          ensurePinnedAgentTabForMode(newProject.agentMode, newProject.path)
        } else {
          tabsStore.clearAllTabs()
        }
      }

      useToastStore.getState().addToast('Workspace added', 'success')

      // Onboarding (Adopt / Start Fresh / Do it later) is now handled
      // inline by AddWorkspaceDialog before this fn runs — the user
      // has already picked one of the three options by the time
      // addProject is called. No follow-up modal needed.
    } catch (err) {
      console.error('[projects] addProject failed:', err)
      useToastStore.getState().addToast('Failed to add workspace', 'error')
    }
  },

  removeProject: async (id: string) => {
    try {
      const tabsStore = useTabsStore.getState()

      // Clean up background workspaces for this project
      for (const key of Object.keys(tabsStore.backgroundWorkspaces)) {
        if (key.startsWith(`${id}:`)) {
          tabsStore.clearBackgroundWorkspace(key)
        }
      }
      // Delete saved layouts from DB (no sync emit — layouts aren't a
      // cross-window-synced surface; the projects refresh below covers it).
      daemonCliPost('workspace-layouts/delete', { projectId: id, workspaceId: null }).catch((e) => console.warn('[projects] workspace-layouts/delete failed:', e))

      await daemonCliPost('projects/delete', { id })
      emitProjectsChanged()

      const state = get()
      if (state.activeProjectId === id) {
        tabsStore.clearAllTabs()
        set({ activeProjectId: null, activeWorkspaceId: null })
      }

      await get().fetchProjects()

      // Select first remaining project if active was deleted
      const updated = get()
      if (!updated.activeProjectId && updated.projects.length > 0) {
        const first = updated.projects[0]
        const firstWorkspaceId = first.workspaces[0]?.id ?? null
        set({
          activeProjectId: first.id,
          activeWorkspaceId: firstWorkspaceId
        })

        if (firstWorkspaceId) {
          const cwd = first.workspaces[0]?.worktreePath ?? first.path ?? '~'
          const newKey = `${first.id}:${firstWorkspaceId}`
          // #681 (Bug A) — ensure pinned tabs after the layout load resolves
          // (consistent with the other open paths).
          await tabsStore.restoreWorkspace(newKey, cwd)
          ensurePinnedAgentTabForMode(first.agentMode, first.path)
        }
      }

      useToastStore.getState().addToast('Workspace removed', 'info')
    } catch (err) {
      console.error('[projects] removeProject failed:', err)
    }
  },

  setActiveProject: (id: string | null) => {
    const state = get()
    const tabsStore = useTabsStore.getState()

    // Stash current workspace (PTYs stay alive in background)
    if (state.activeProjectId && state.activeWorkspaceId) {
      tabsStore.stashWorkspace(`${state.activeProjectId}:${state.activeWorkspaceId}`)
    } else if (tabsStore.tabs.length > 0) {
      // No tracked workspace but tabs exist — clear them so the next workspace starts clean
      tabsStore.clearAllTabs()
    }

    if (id === null) {
      set({ activeProjectId: null, activeWorkspaceId: null })
      return
    }

    const project = state.projects.find((p) => p.id === id)
    if (project) {
      // #672 — opening/focusing a workspace is an ACTIVATION (PRD §4.3.1).
      // POST projects/activate so the daemon keeps it in the canonical
      // Active set and its daemon-side reaper won't touch it. Deduped on
      // the foreground id; capability-gated. (Replaces the #657 renderer
      // dismiss-reap cancel — the daemon now owns reaping.)
      activateProject(id)
      const newWorkspaceId = project.workspaces[0]?.id ?? null
      set({
        activeProjectId: id,
        activeWorkspaceId: newWorkspaceId
      })

      // Persist for session restore (Phase 2.5 fix #547: gated so a
      // pre-load click doesn't clobber the real last-active values).
      if (hasLoadedFromDaemon) {
        settingsUpdate({ lastActiveProjectId: id, lastActiveWorkspaceId: newWorkspaceId }).catch(() => {})
      } else {
        console.warn('[projects] persist suppressed: projects not yet loaded from daemon')
      }

      if (newWorkspaceId) {
        const cwd = project.workspaces[0]?.worktreePath ?? project.path ?? '~'
        const newKey = `${id}:${newWorkspaceId}`
        // #681 (Bug A) — ensure pinned tabs only AFTER restoreWorkspace's
        // slow-path layout load finishes (mirrors #658 / addProject). The
        // live fast path resolves immediately; the brand-new / no-saved-
        // layout path resolves after the workspace key + cleared tabs are
        // in place, so Chat + Inbox are created deterministically (Chat
        // active) on first open rather than only after a switch-away. The
        // store contract stays `=> void`; we chain rather than await so the
        // signature doesn't change for callers.
        void tabsStore.restoreWorkspace(newKey, cwd).then(() => {
          ensurePinnedAgentTabForMode(project.agentMode, project.path)
        })
      }

      // P1.B — clicking a project in the icon rail is a real interaction:
      // reset its 24h Active window and surface it in the Active Bar.
      // `setActiveWorkspace` already did this; `setActiveProject` (the
      // project-icon-rail click) did not, so a click alone never kept the
      // workspace Active. touchInteraction is debounced to 5min/project.
      get().touchInteraction(id)
    }
  },

  setActiveWorkspace: (projectId: string, workspaceId: string) => {
    const state = get()

    // Skip if already on this workspace — prevents unnecessary stash/restore
    if (state.activeProjectId === projectId && state.activeWorkspaceId === workspaceId) return

    const tabsStore = useTabsStore.getState()

    // #672 — activating any workspace is an ACTIVATION (PRD §4.3.1): POST
    // projects/activate so the daemon keeps it canonically Active and its
    // reaper spares it. Deduped on the foreground id; capability-gated.
    activateProject(projectId)

    // Stash current workspace (PTYs stay alive in background)
    if (state.activeProjectId && state.activeWorkspaceId) {
      tabsStore.stashWorkspace(`${state.activeProjectId}:${state.activeWorkspaceId}`)
    } else if (tabsStore.tabs.length > 0) {
      tabsStore.clearAllTabs()
    }

    set({
      activeProjectId: projectId,
      activeWorkspaceId: workspaceId
    })

    // Persist for session restore (Phase 2.5 fix #547: gated).
    if (hasLoadedFromDaemon) {
      settingsUpdate({ lastActiveProjectId: projectId, lastActiveWorkspaceId: workspaceId }).catch(() => {})
    } else {
      console.warn('[projects] persist suppressed: projects not yet loaded from daemon')
    }

    // 24h Active Bar persistence — bumping lastInteractionAt on every
    // workspace activation means a workspace stays in the Active Bar
    // for 24h after the user last touched it, then expires. The
    // touchInteraction call is debounced to 5min/project so rapid
    // tab-switching doesn't hammer the DB.
    get().touchInteraction(projectId)

    const project = state.projects.find((p) => p.id === projectId)
    const workspace = project?.workspaces.find((w) => w.id === workspaceId)
    const cwd = workspace?.worktreePath ?? project?.path ?? '~'
    const newKey = `${projectId}:${workspaceId}`
    // #681 (Bug A) — ensure pinned tabs only AFTER restoreWorkspace's
    // slow-path layout load finishes (mirrors #658 / addProject /
    // setActiveProject). For a brand-new / never-opened workspace this is
    // what makes the pinned Chat + Inbox (Chat active) appear on first
    // open instead of only after switching away and back.
    void tabsStore.restoreWorkspace(newKey, cwd).then(() => {
      if (project) {
        ensurePinnedAgentTabForMode(project.agentMode, project.path)
      }
    })
  },

  reorderProjects: async (ids: string[]) => {
    try {
      await daemonCliPost('projects/reorder', { ids })
      emitProjectsChanged()
      await get().fetchProjects()
    } catch (err) {
      console.error('[projects] reorderProjects failed:', err)
    }
  },

  renameProject: async (id: string, name: string) => {
    try {
      await daemonCliPost('projects/update', { id, name })
      emitProjectsChanged()
      await get().fetchProjects()
    } catch (err) {
      console.error('[projects] renameProject failed:', err)
    }
  },

  // sections_* mutations did NOT emit a cross-window sync from the old
  // Tauri shims (workspace_sections.rs has no `app.emit`), so no
  // `emitProjectsChanged()` here — the local `fetchProjects()` refresh is
  // the only contract.
  createSection: async (projectId: string, name: string, color?: string) => {
    try {
      await daemonCliPost('sections/create', { projectId, name, color })
      await get().fetchProjects()
    } catch (err) {
      console.error('[projects] createSection failed:', err)
    }
  },

  deleteSection: async (id: string) => {
    try {
      await daemonCliPost('sections/delete', { id })
      await get().fetchProjects()
    } catch (err) {
      console.error('[projects] deleteSection failed:', err)
    }
  },

  renameSection: async (id: string, name: string) => {
    try {
      await daemonCliPost('sections/update', { id, name })
      await get().fetchProjects()
    } catch (err) {
      console.error('[projects] renameSection failed:', err)
    }
  },

  updateSection: async (id: string, updates: { name?: string; color?: string | null; isCollapsed?: number }) => {
    try {
      await daemonCliPost('sections/update', { id, ...updates })
      await get().fetchProjects()
    } catch (err) {
      console.error('[projects] updateSection failed:', err)
    }
  },

  assignWorkspaceToSection: async (workspaceId: string, sectionId: string | null) => {
    try {
      await daemonCliPost('sections/assign', { workspaceId, sectionId })
      await get().fetchProjects()
    } catch (err) {
      console.error('[projects] assignWorkspaceToSection failed:', err)
    }
  },

  touchInteraction: async (projectId: string) => {
    const now = Date.now()
    const last = _lastTouchMap.get(projectId) || 0
    if (now - last < TOUCH_DEBOUNCE_MS) return
    _lastTouchMap.set(projectId, now)
    // Optimistic local update
    set((state) => ({
      projects: state.projects.map((p) =>
        p.id === projectId ? { ...p, lastInteractionAt: Math.floor(now / 1000) } : p
      )
    }))
    // Write to DB — await so subsequent fetchProjects picks up the value.
    // projects_touch_interaction did NOT emit a cross-window sync.
    try {
      await daemonCliPost('projects/touch-interaction', { id: projectId })
    } catch (err) {
      console.warn('[projects] touchInteraction failed:', err)
    }
  },

  setManuallyActive: async (projectId: string, active: boolean) => {
    try {
      // #672 — pin/unpin is a canonical-Active gesture. On a daemon that
      // supports it, route through projects/pin so the daemon sets
      // manually_active, recomputes the canonical set, and broadcasts the
      // delta (the renderer mirrors it). Fall back to the legacy
      // projects/update write against an un-updated daemon.
      if (serverSupports('canonical-active')) {
        // Optimistic echo so the bar reflects the pin immediately; the
        // daemon's active_changed delta reconciles it.
        if (active) useActiveStore.getState().echoActive(projectId)
        else useActiveStore.getState().echoInactive(projectId)
        await daemonCliPost('projects/pin', { projectId, pinned: active })
      } else {
        await daemonCliPost('projects/update', { id: projectId, manuallyActive: active ? 1 : 0 })
      }
      emitProjectsChanged()
      await get().fetchProjects()
    } catch (err) {
      console.error('[projects] setManuallyActive failed:', err)
    }
  }
}))

// Initialize on import
useProjectsStore.getState().fetchProjects()

// Phase 2.5 fix (finding #547): retry the load if the daemon
// (re)connects after a failed initial fetch. Plan B — projects-list now
// goes through the host-aware `daemonCliGet('projects/list')` HTTP layer
// (local OR remote), and the last-active-session restore inside
// fetchProjects calls `settingsGet()`; a failure in either causes the
// catch-all `[projects] fetchProjects failed` log seen in the original
// finding #547 repro. Re-running once the daemon's WS comes up gets the
// user a real restore instead of "first project".
onDaemonConnected(() => {
  if (hasLoadedFromDaemon) return
  useProjectsStore.getState().fetchProjects()
})

// #625 — on a real active-host CHANGE, re-run the active-project /
// active-workspace restore against the NEW host. The projects LIST
// already re-fetches on the `<App key={hostKey}>` remount, but this store
// is a module singleton: its `activeProjectId` / `activeWorkspaceId`
// survive the remount, so the restore branch inside `fetchProjects` (which
// only runs when `!activeProjectId`) would be SKIPPED and the OLD host's
// active selection would leak.
//
// We clear the active selection FIRST so the restore branch re-runs, and
// drop `hasLoadedFromDaemon` so the early restore writes don't fire against
// the new host before its baseline lands (fetchProjects flips the gate back
// to true after a successful restore). `fetchProjects` reads `activeHost`
// at call time via the host-aware `daemonCli*` / `settingsGet()` layer, and
// `onActiveHostChange` fires AFTER the flip, so the restore targets the new
// host's projects + its last-active selection.
// 0.39.45 (GH #18/#26) — daemon push: the registered project set changed
// (another window, the CLI's `k2so workspace create`, an onboarding flow,
// or a remote client added/removed a project). Re-fetch so the nav +
// Settings update WITHOUT a manual window reload. This replaces reliance
// on the local-only Tauri `sync:projects` event, which never fires for
// CLI/remote mutations and doesn't exist for K2 Connect clients.
onProjectsChanged(() => {
  void useProjectsStore.getState().fetchProjects()
})

onActiveHostChange(() => {
  hasLoadedFromDaemon = false
  // #672 — reset the activate dedup so the restore against the NEW host
  // re-POSTs projects/activate for the landed-on workspace (a different
  // daemon must see the activation fresh).
  resetActivateDedup()
  useProjectsStore.setState({ activeProjectId: null, activeWorkspaceId: null })
  void useProjectsStore.getState().fetchProjects()
})
