// 假 CLI：固定速率輸出帶顏色的文字，模擬高輸出 agent。argv: <bytes/sec> <seconds>
const rate = Number(process.argv[2] || 262144)
const seconds = Number(process.argv[3] || 30)
const line = '\x1b[32mINFO\x1b[0m compiling module \x1b[36msrc/renderer/app.tsx\x1b[0m ' + 'x'.repeat(40) + '\r\n'
const chunk = line.repeat(Math.max(1, Math.floor((rate / 20) / line.length)))
const end = Date.now() + seconds * 1000
const timer = setInterval(() => {
  if (Date.now() >= end) {
    clearInterval(timer)
    return
  }
  process.stdout.write(chunk)
}, 50)
