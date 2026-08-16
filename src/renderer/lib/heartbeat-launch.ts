import { daemonCliGetText } from '@/lib/daemon-cli'
import { useHeartbeatSessionsStore } from '@/stores/heartbeat-sessions'
import { useToastStore } from '@/stores/toast'

/** Manual Launch / test-fire. Always passes `force=1` so a disabled
 *  heartbeat still runs (scheduler ticks still skip disabled rows). */
export async function launchHeartbeat(projectPath: string, name: string): Promise<boolean> {
  const toast = useToastStore.getState()
  try {
    const resp = await daemonCliGetText('heartbeat/launch', {
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
    void useHeartbeatSessionsStore.getState().refresh(projectPath)
    return true
  } catch (err) {
    toast.addToast(`Launch failed: ${String(err)}`, 'error', 4000)
    return false
  }
}
