import { useState, useEffect } from 'react'
// Plan B — project-icon lookup is host-aware daemon data: route through
// the `/cli/projects/get-icon` HTTP layer (local OR remote) instead of
// the localhost-pinned Tauri `projects_get_icon` invoke proxy.
import { daemonCliGet } from '@/lib/daemon-cli'

// Cache icon results across component instances
const iconCache = new Map<string, { found: boolean; dataUrl: string | null }>()

interface ProjectAvatarProps {
  projectPath: string
  projectName: string
  projectColor: string
  projectId?: string
  iconUrl?: string | null
  size?: number
}

export default function ProjectAvatar({
  projectPath,
  projectName,
  projectColor,
  projectId,
  iconUrl: iconUrlProp,
  size = 28
}: ProjectAvatarProps): React.JSX.Element {
  const [iconUrl, setIconUrl] = useState<string | null>(() => {
    if (iconUrlProp) return iconUrlProp
    // Check cache synchronously to avoid flash
    const cached = iconCache.get(projectPath)
    return cached?.found && cached.dataUrl ? cached.dataUrl : null
  })
  const [loaded, setLoaded] = useState(() => {
    return !!iconUrlProp || iconCache.has(projectPath)
  })

  // Sync prop changes
  useEffect(() => {
    if (iconUrlProp) {
      setIconUrl(iconUrlProp)
      setLoaded(true)
    }
  }, [iconUrlProp])

  useEffect(() => {
    // If iconUrl was provided via prop, skip the query
    if (iconUrlProp) return

    // Check cache first
    const cached = iconCache.get(projectPath)
    if (cached) {
      if (cached.found && cached.dataUrl) {
        setIconUrl(cached.dataUrl)
      }
      setLoaded(true)
      return
    }

    let cancelled = false
    daemonCliGet<{ found: boolean; dataUrl: string | null }>('projects/get-icon', { path: projectPath, project_id: projectId })
      .then((result) => {
        iconCache.set(projectPath, result)
        if (!cancelled && result.found && result.dataUrl) {
          setIconUrl(result.dataUrl)
        }
        if (!cancelled) setLoaded(true)
      })
      .catch(() => {
        iconCache.set(projectPath, { found: false, dataUrl: null })
        if (!cancelled) setLoaded(true)
      })

    return () => {
      cancelled = true
    }
  }, [projectPath, iconUrlProp, projectId])

  const firstLetter = projectName.charAt(0).toUpperCase()

  if (iconUrl) {
    return (
      <span
        className="flex-shrink-0"
        style={{
          width: size,
          height: size,
          border: `2px solid ${projectColor}`,
          overflow: 'hidden',
          display: 'block',
        }}
      >
        <img
          src={iconUrl}
          alt={projectName}
          style={{
            width: '100%',
            height: '100%',
            objectFit: 'cover',
            objectPosition: 'center',
            display: 'block',
          }}
        />
      </span>
    )
  }

  return (
    <span className="flex-shrink-0" style={{ width: size, height: size }}>
      <span
        className="flex items-center justify-center"
        style={{
          width: size,
          height: size,
          backgroundColor: loaded ? projectColor : 'var(--color-bg-elevated)',
          color: '#ffffff',
          fontSize: size * 0.5,
          fontWeight: 700,
          lineHeight: 1,
          fontFamily: 'inherit',
          display: 'flex'
        }}
      >
        {firstLetter}
      </span>
    </span>
  )
}
