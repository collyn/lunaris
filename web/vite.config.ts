import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import { readFileSync } from 'node:fs'

// The version of the page: the workspace's (Cargo.toml [workspace.package]).
const appVersion = (() => {
  try {
    const toml = readFileSync(new URL('../Cargo.toml', import.meta.url), 'utf8')
    return /\[workspace\.package\][^[]*?\nversion\s*=\s*"([^"]+)"/.exec(toml)?.[1] ?? ''
  } catch {
    return ''
  }
})()

// https://vite.dev/config/
export default defineConfig({
  plugins: [react()],
  base: './',
  define: { __APP_VERSION__: JSON.stringify(appVersion) },
})
