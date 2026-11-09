// Page entry for the Tauri build: the platform first, then window.api, then the same app.
import { installPlatform } from './platform'

await installPlatform()
await import('../preload/index')
await import('../renderer/src/main')
