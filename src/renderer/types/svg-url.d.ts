declare module '*.svg?url' {
  const url: string
  export default url
}

declare module '*.svg?raw' {
  const svg: string
  export default svg
}

declare module '*.woff?url' {
  const url: string
  export default url
}

declare module '*.woff2?url' {
  const url: string
  export default url
}

declare module '*.png' {
  const src: string
  export default src
}
