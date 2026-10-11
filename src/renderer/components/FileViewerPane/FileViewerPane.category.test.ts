import { describe, it, expect } from 'vitest'
import {
  getFileCategory,
  getDefaultViewMode,
  isBinaryPreviewCategory,
  supportsViewToggle,
} from './fileCategory'

describe('getFileCategory — Phase A', () => {
  it('classifies images including heic/avif/tiff', () => {
    expect(getFileCategory('/t/a.png')).toBe('image')
    expect(getFileCategory('/t/a.JPEG')).toBe('image')
    expect(getFileCategory('/t/a.heic')).toBe('image')
    expect(getFileCategory('/t/a.avif')).toBe('image')
    expect(getFileCategory('/t/a.tiff')).toBe('image')
  })
  it('classifies csv/tsv, audio, video, zip', () => {
    expect(getFileCategory('/t/a.csv')).toBe('csv')
    expect(getFileCategory('/t/a.tsv')).toBe('csv')
    expect(getFileCategory('/t/a.mp3')).toBe('audio')
    expect(getFileCategory('/t/a.mp4')).toBe('video')
    expect(getFileCategory('/t/a.zip')).toBe('zip')
  })
  it('classifies mermaid vs diagram sources', () => {
    expect(getFileCategory('/t/a.mmd')).toBe('mermaid')
    expect(getFileCategory('/t/a.mermaid')).toBe('mermaid')
    expect(getFileCategory('/t/a.drawio')).toBe('diagram')
    expect(getFileCategory('/t/a.puml')).toBe('diagram')
    expect(getFileCategory('/t/a.dot')).toBe('diagram')
  })
  it('classifies CAD: .dxf is a 2D drawing, meshes are 3D (.obj is Wavefront, not binary)', () => {
    expect(getFileCategory('/t/plan.dxf')).toBe('cad2d')
    expect(getFileCategory('/t/PLAN.DXF')).toBe('cad2d')
    for (const ext of ['.stl', '.obj', '.3mf', '.ply', '.glb', '.gltf', '.STL']) {
      expect(getFileCategory(`/t/part${ext}`)).toBe('model3d')
    }
    expect(getDefaultViewMode('cad2d')).toBe('rendered')
    expect(getDefaultViewMode('model3d')).toBe('rendered')
    // DXF is text (Preview | Source); meshes load bytes and have no toggle.
    expect(isBinaryPreviewCategory('cad2d')).toBe(false)
    expect(supportsViewToggle('cad2d')).toBe(true)
    expect(isBinaryPreviewCategory('model3d')).toBe(true)
    expect(supportsViewToggle('model3d')).toBe(false)
  })
  it('classifies binary and text defaults', () => {
    expect(getFileCategory('/t/a.wasm')).toBe('binary')
    expect(getFileCategory('/t/a.rs')).toBe('text')
    expect(getFileCategory('/t/notes.md')).toBe('markdown')
    expect(getFileCategory('/t/x.pdf')).toBe('pdf')
    expect(getFileCategory('/t/x.docx')).toBe('docx')
  })
})

describe('view modes and helpers', () => {
  it('defaults previews to rendered where appropriate', () => {
    expect(getDefaultViewMode('csv')).toBe('rendered')
    expect(getDefaultViewMode('mermaid')).toBe('rendered')
    expect(getDefaultViewMode('zip')).toBe('rendered')
    expect(getDefaultViewMode('text')).toBe('raw')
  })
  it('binary preview categories skip text path', () => {
    expect(isBinaryPreviewCategory('image')).toBe(true)
    expect(isBinaryPreviewCategory('zip')).toBe(true)
    expect(isBinaryPreviewCategory('audio')).toBe(true)
    expect(isBinaryPreviewCategory('csv')).toBe(false)
  })
  it('supports view toggle for dual-mode types', () => {
    expect(supportsViewToggle('csv')).toBe(true)
    expect(supportsViewToggle('mermaid')).toBe(true)
    expect(supportsViewToggle('zip')).toBe(false)
    expect(supportsViewToggle('audio')).toBe(false)
  })
})
