import { defineConfig, type Plugin } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'
import { resolve, join, normalize, sep } from 'path'
import { copyFileSync, existsSync, mkdirSync, readFileSync } from 'fs'

/**
 * The Zen widget standard library (prd-zen-user-widgets-v2 UWB12): files
 * live in `src/renderer/zen-lib/<id>@<version>/`, NOT `public/`, so only
 * this desktop config ships them (the hosted web build never carries the
 * ~9 MB). Build: copy each manifest file + its licence into
 * `out/renderer/zen-lib/`. Dev: serve them byte for byte at `/zen-lib/`
 * before Vite's transform middleware (a JS request would otherwise be
 * rewritten as a module).
 */
function zenLibDesktopCopy(): Plugin {
  const src = resolve(__dirname, 'src/renderer/zen-lib')
  const manifestPath = resolve(__dirname, 'src/shared/zen-lib.json')
  const listed = (): string[] => {
    const m = JSON.parse(readFileSync(manifestPath, 'utf8')) as {
      libs: { id: string; version: string; source: string; licenseFile: string; files: { name: string }[] }[]
    }
    return m.libs.flatMap((l) => {
      const dir = `${l.id}@${l.version}`
      const code = l.source === 'bundled' ? l.files.map((f) => `${dir}/${f.name}`) : []
      return [...code, `${dir}/${l.licenseFile}`]
    })
  }
  return {
    name: 'k2-zen-lib-desktop-copy',
    configureServer(server) {
      server.middlewares.use('/zen-lib', (req, res, next) => {
        const rel = decodeURIComponent((req.url ?? '').split('?')[0]).replace(/^\/+/, '')
        if (!listed().includes(rel)) return next()
        const file = normalize(join(src, rel))
        if (!file.startsWith(src + sep) || !existsSync(file)) return next()
        res.setHeader('Content-Type', rel.endsWith('.js') ? 'text/javascript; charset=utf-8' : 'text/plain; charset=utf-8')
        res.setHeader('Cache-Control', 'no-store')
        res.end(readFileSync(file))
      })
    },
    writeBundle(options) {
      const outDir = options.dir ?? resolve(__dirname, 'out/renderer')
      for (const rel of listed()) {
        const from = join(src, rel)
        if (!existsSync(from)) throw new Error(`zen-lib: ${rel} is in src/shared/zen-lib.json but not on disk`)
        const to = join(outDir, 'zen-lib', rel)
        mkdirSync(resolve(to, '..'), { recursive: true })
        copyFileSync(from, to)
      }
    },
  }
}

export default defineConfig({
  plugins: [react(), tailwindcss(), zenLibDesktopCopy()],
  resolve: {
    alias: {
      '@': resolve(__dirname, 'src/renderer'),
      '@shared': resolve(__dirname, 'src/shared')
    }
  },
  root: 'src/renderer',
  build: {
    outDir: '../../out/renderer',
    emptyOutDir: true
  },
  server: {
    port: 5173,
    strictPort: false
  },
  // Prevent Vite from obscuring Rust errors
  clearScreen: false,
  envPrefix: ['VITE_', 'TAURI_']
})
