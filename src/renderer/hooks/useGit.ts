import { useState, useEffect, useRef, useCallback } from 'react'
// Plan B — git status/diff are host-aware daemon data: route them through
// the `/cli/git/*` HTTP layer (local OR remote) instead of the
// localhost-pinned Tauri `git_*` invoke proxy. GET params are snake_case
// (`path`); the JSON response shapes match the Rust structs as-is.
import { daemonCliGet } from '@/lib/daemon-cli'
import { GIT_POLL_INTERVAL } from '@shared/constants'

// ── Types ────────────────────────────────────────────────────────────────────

interface GitInfo {
  isRepo: boolean
  currentBranch: string
  ahead: number
  behind: number
  changedFiles: number
  untrackedFiles: number
}

interface ChangedFile {
  path: string
  status: 'modified' | 'added' | 'deleted' | 'untracked'
  staged: boolean
}

interface UseGitInfoResult {
  data: GitInfo | null
  loading: boolean
  error: string | null
  refetch: () => void
}

interface UseGitChangesResult {
  data: ChangedFile[]
  loading: boolean
  error: string | null
  refetch: () => void
}

// ── useGitInfo ───────────────────────────────────────────────────────────────

export function useGitInfo(projectPath?: string): UseGitInfoResult {
  const [data, setData] = useState<GitInfo | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const intervalRef = useRef<ReturnType<typeof setInterval> | null>(null)

  const fetch = useCallback(async () => {
    if (!projectPath) {
      setData(null)
      return
    }

    try {
      setLoading((prev) => (prev ? prev : true))
      const result = await daemonCliGet<GitInfo>('git/info', { path: projectPath })
      setData(result)
      setError(null)
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to fetch git info')
    } finally {
      setLoading(false)
    }
  }, [projectPath])

  useEffect(() => {
    fetch()

    // Auto-refresh every 5 seconds
    intervalRef.current = setInterval(fetch, GIT_POLL_INTERVAL)

    return () => {
      if (intervalRef.current) {
        clearInterval(intervalRef.current)
      }
    }
  }, [fetch])

  return { data, loading, error, refetch: fetch }
}

// ── useGitChanges ────────────────────────────────────────────────────────────

export function useGitChanges(projectPath?: string): UseGitChangesResult {
  const [data, setData] = useState<ChangedFile[]>([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const intervalRef = useRef<ReturnType<typeof setInterval> | null>(null)

  const fetch = useCallback(async () => {
    if (!projectPath) {
      setData([])
      return
    }

    try {
      setLoading((prev) => (prev ? prev : true))
      const result = await daemonCliGet<ChangedFile[]>('git/changes', { path: projectPath })
      setData(result)
      setError(null)
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to fetch git changes')
    } finally {
      setLoading(false)
    }
  }, [projectPath])

  useEffect(() => {
    fetch()

    intervalRef.current = setInterval(fetch, GIT_POLL_INTERVAL)

    return () => {
      if (intervalRef.current) {
        clearInterval(intervalRef.current)
      }
    }
  }, [fetch])

  return { data, loading, error, refetch: fetch }
}
