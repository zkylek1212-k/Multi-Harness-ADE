// 讓 node --experimental-strip-types 能載入 app 的模組：app 原始碼的相對 import 不寫副檔名
// （給 bundler 用），這裡在找不到時補上 .ts。只給 scripts/check-*.mts 用。
import { registerHooks } from 'node:module'

registerHooks({
  resolve(specifier, context, next) {
    try {
      return next(specifier, context)
    } catch (e) {
      const relative = specifier.startsWith('./') || specifier.startsWith('../')
      if (relative && !/\.[cm]?[jt]s$/.test(specifier)) {
        try {
          return next(`${specifier}.ts`, context)
        } catch {
          return next(`${specifier}/index.ts`, context)
        }
      }
      throw e
    }
  }
})
