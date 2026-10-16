// Build-time stand-in for `three/webgpu` (prd-wiki-3d-view-v1 §6.5).
//
// `three-render-objects` (under react-force-graph-3d) statically imports
// `WebGPURenderer` from `three/webgpu`, which pulls ~165 KB gz of WebGPU
// renderer into the Wiki 3D chunk. react-force-graph-3d never forwards
// `useWebGPU`, so the import is never used at runtime. Both Vite configs
// alias `three/webgpu` here. Rolldown needs the NAMED export to exist.
export const WebGPURenderer = undefined
