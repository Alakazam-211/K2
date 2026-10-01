import { daemonCliGetText } from '@/lib/daemon-cli'
import type { HeartbeatSessionsStore } from '@/stores/heartbeat-sessions'
import { useToastStore } from '@/stores/toast'
import { assertScopeMayWrite, type ServerScope } from '@/kessel/server-scope'

/** Manual Launch / test-fire. Always passes `force=1` so a disabled
 *  heartbeat still runs (scheduler ticks still skip disabled rows). */
export async function launchHeartbeat(
  room: { scope: ServerScope; heartbeats: HeartbeatSessionsStore },
  projectPath: string,
  name: string,
): Promise<boolean> {
  const toast = useToastStore.getState()
  // `heartbeat/launch` is a GET that fires: held to the room's write rules
  // (never from a view-only room; on the allowlist for a usable one).
  assertScopeMayWrite(room.scope, 'heartbeat/launch')
  try {
    const resp = await daemonCliGetText(room.scope, 'heartbeat/launch', {
      project: projectPath,
      name,
      force: '1',
    })
    type LaunchResp = {
      success: boolean
      decision: string
      branch?: string
      reason?: string
    }
    const parsed: LaunchResp = JSON.parse(resp)
    if (!parsed.success) {
      toast.addToast(
        `Launch failed: ${parsed.reason ?? parsed.decision}`,
        'error',
        4000,
      )
      return false
    }
    const branchLabel: Record<string, string> = {
      fresh_fire: 'Fired',
      injected: 'Sent wakeup to running session for',
      resume_and_fire: 'Resumed + fired',
    }
    let verb: string
    if (parsed.branch && parsed.branch.startsWith('workspace_session:')) {
      verb = 'Sent wakeup to pinned chat for'
    } else if (parsed.branch && parsed.branch in branchLabel) {
      verb = branchLabel[parsed.branch]
    } else {
      verb = 'Fired'
    }
    toast.addToast(`${verb} "${name}"`, 'success', 2500)
    void room.heartbeats.getState().refresh(projectPath)
    return true
  } catch (err) {
    toast.addToast(`Launch failed: ${String(err)}`, 'error', 4000)
    return false
  }
}
