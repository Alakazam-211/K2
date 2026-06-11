declare module '*.svg?url' {
  const url: string
  export default url
}

declare module '*.svg?raw' {
  const svg: string
  export default svg
}
