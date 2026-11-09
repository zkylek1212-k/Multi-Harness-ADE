// A stand-in CLI for terminal checks. Every line starting with @ is base64 to print; anything else (arrow keys) is ignored.
// The first line of its output file records the arguments it was started with.
import fs from 'node:fs'

if (process.env.PTY_SCRIPT_ARGS) fs.appendFileSync(process.env.PTY_SCRIPT_ARGS, JSON.stringify(process.argv.slice(2)) + '\n')
process.stdin.setRawMode?.(true)
process.stdin.resume()
let buf = ''
process.stdin.on('data', (d) => {
  buf += d.toString()
  let i
  while ((i = buf.search(/[\r\n]/)) >= 0) {
    const line = buf.slice(0, i)
    buf = buf.slice(i + 1)
    const at = line.indexOf('@')
    if (at >= 0) process.stdout.write(Buffer.from(line.slice(at + 1), 'base64'))
  }
})
