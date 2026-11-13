# Multi-Harness-ADE — Agent Workbench

A lightweight, **agent-native** developer workbench: a Monaco code editor, a Git
visualizer, N embedded CLI terminals, and a Cowork canvas of agent terminals. Built with Tauri, Rust +
React + Vite.

*English is the primary language of this README; a 繁體中文 version follows below.*

> **Design principle — official CLIs own the agent runtime.** The workbench uses
> each vendor's official CLI and its existing authentication. It provides code
> editing, Git, terminals, shared memory, and the Cowork canvas, where you run several
> CLIs side by side, each in its own Git worktree, and decide what gets merged.
> Cross-CLI handoffs use `.project-memory/` (handoff notes + Git).

## Features

- **Monaco editor** — the same editor core as VS Code (via the MIT-licensed
  `monaco-editor`), with edit and diff views, including commit-level diffs from
  the Git panel.
- **Git panel** — status, staging, commit, branch switch, a commit graph, and
  recent-commit / file diffs.
- **Multi-CLI terminals** — each CLI gets its own `xterm.js` terminal tab, run
  natively as a child process via `node-pty`. No keys stored; the CLIs use their
  own subscriptions/auth.
- **Cowork canvas** — a third mode: Claude Code, Codex, Antigravity and your own launchers as interactive terminal nodes on a pannable, zoomable canvas, each in its own worktree. Approval cards, drawn handoffs, commit / merge / discard with previews, and a repository-wide stop. See [Cowork](#cowork).
- **Adjustable layout** — drag the terminal and editor icons in the title bar to swap their sides, and switch each pane on or off.
- **Preview & Documents** — live Markdown / HTML preview that auto-syncs on edit and save, plus built-in document viewing for Word, Excel, PowerPoint, and PDF.
- **Vibe Coding Mode** — an alternative, task-first layout (Settings → Appearance → Work Mode): an icon rail (Sessions / Status / Handoff / Files / Git / Settings) whose panels fade in on hover and pin on click, the agent terminal in the middle, and the live result on the right. Dev-server URLs printed in the terminal (`http://localhost:PORT`) open automatically, and the Handoff panel summarises `.project-memory/handoff.md`. Developer Mode keeps the classic code-first layout.
- **Dashboard & Telemetry** — session list plus token usage scanned from local CLI records (Claude Code, Codex, Antigravity), switchable between **All / 30d / 7d / Today** and split into input / cache read / output. Claude and Codex figures come from the CLIs' own usage records; Antigravity logs carry no token counts, so its figures are character-based estimates and are labelled as such. Includes CLI enable/disable filtering, folder grouping, and one-click workspace switching.
- **Fast Startup & Mount-on-Demand** — 42x accelerated cold startup powered by disk-persisted session caches and lazy-loaded sidebar/central panels.
- **Bilingual i18n** — full interface localization supporting seamless toggling between Strict English and Traditional Chinese.
- **CLI Permissions & Bypass Mode** — toggleable bypass mode skipping interactive approval prompts for Claude Code (`--permission-mode bypassPermissions`), Codex (`--dangerously-bypass-approvals-and-sandbox`), and Antigravity (`--dangerously-skip-permissions`).
- **iPhone Remote Control (LAN)** — watch and answer your CLI agents from an iPhone on the same Wi-Fi: a Home Screen web app lists every terminal, mirrors its output, shows an approval card with one-tap answers, lets you type or start new agents, and sends a notification when an agent needs you. It can also open one of your recent workspaces on the computer and start an agent inside it, fits the terminal to the phone's width automatically, previews the dev server a terminal printed, and remembers up to three computers to switch between. See [iPhone remote control](#iphone-remote-control).
- **Architecture overview** — how the desktop app, the phone app and the remote bridge fit together: [`docs/architecture.html`](docs/architecture.html) (open it in a browser).

## Customize Agent

In **Settings → CLI & Agents → Customize Agent**, enable the option and save a display name, executable (a path or a command on PATH), and arguments (one per line, without shell quotes). The harness then appears in the terminal launchpad, **+** menu, prompt dispatcher, and phone's New Terminal sheet. Disabling it hides these launch options and its extension editor, preserves the saved configuration and files, and leaves existing terminals running.

Optionally set absolute paths for a **Skills directory**, **MCP configuration file**, **Plugins directory**, and **Hooks configuration file**. In **Settings → Extensions → Custom Agent extensions**, choose a kind and open or create native UTF-8 files. For Skills, a typical file is `my-skill/SKILL.md`; for Plugins, use the files your harness expects. MCP and Hooks edit the exact configured file. Paths must match where the harness actually loads extensions; the workbench does not change its discovery rules or translate extension formats. Existing files get a `.workbench-backup` on save; invalid JSON and files changed since opening are rejected. Directory browsing is limited to six levels and 500 files; editing is limited to 1 MB per file.

The executable, arguments, paths, and enable switch are local user settings, never taken from a repository. Custom harnesses use their own authentication and permissions; built-in bypass/default flags are not injected. Cowork, provider session history and token telemetry continue to support the three built-in CLIs. The separate Linux app is unchanged.

## Cowork

Cowork is a canvas mode for running several agents at once in one Git repository. It replaced the earlier meeting and planning Cowork in 2026-11; chair meetings, the task board and background runs are gone.

1. Switch to **Cowork** (Settings → Work Mode, the mode switch on the canvas, or **Terminals → + → Cowork**). A channel called *general* is created the first time; a channel is one canvas with its nodes, handoffs and events.
2. Drag an agent from the left onto the canvas, or press **+** next to it. Each node is the agent's real interactive terminal, started in its own worktree (`.cowork/<channel>/<node>`, branch `cowork/<channel>/<node>`). Tick *New nodes work in the repository root* to run one node directly in the root instead; only one such node per repository.
3. When an agent asks for permission, the node's border changes and an approval card appears under it. **Approve** and **Reject** appear only when the question and its options are recognized; otherwise the card sends you to the terminal. With structured approval (Settings → Cowork) the card shows the change itself. Any failure hands the question back to the CLI's own prompt and never approves.
4. Drag from one node's right edge to another's left edge to create a **handoff**: a draft with the source branch, commits, changed files and uncommitted state, taken from Git. It is never sent automatically. Copy it, or **Paste into terminal**, which uses bracketed paste and never presses Enter.
5. In the inspector on the right: view changes (committed, staged, unstaged, untracked), **Commit all changes** with your message, **Preview** and **Merge** into the branch you pick, **Clean up** a merged worktree, or **Discard** one after a preview that lists every file, including ignored ones. Merge and discard previews are bound to the commits and files they showed: anything that changes in between means previewing again.
6. **Stop** (or Ctrl+Shift+Backspace) sends one interrupt key to every running Cowork node of the repository, in every channel and window, and refuses new nodes, pastes and approvals. **End all** then ends the processes; **Clear stop** only lets the app accept new actions again.

Closing a node ends its process and keeps its worktree. Closing the app keeps the canvas and worktrees; after a restart every node shows as ended and can be started again. Zoom out below half size and nodes show a placeholder instead of a live terminal. Channels are stored in the repository's Git directory (`.git/cowork/`), so they are never committed.

**Settings → Cowork** sets where worktrees go (keep them outside OneDrive-style synced folders) and turns structured approval on per CLI. It is on by default for Claude Code, where it affects only Cowork sessions. Codex asks once, inside Codex, to trust the hook. For Antigravity an entry is added to `~/.gemini/config/hooks.json`; its Approve still passes through Antigravity's own edit review, while Reject works directly. With bypass mode on, no hook is installed.

## Memory sync

`.project-memory/` holds the handoff notes that Claude Code, Codex and Antigravity read and write. **Settings → Memory sync** keeps it in sync between your computers while keeping it out of the public project repository. It is off by default; while off, Workbench does no syncing at all (handoffs stay on this computer, no network, no extra tokens).

1. Turn on the consent switch and choose where memory lives:
   - **GitHub (private repository)** — versioned. Either install the GitHub CLI and let Workbench create the repository, or choose **Without installing gh** and click **Create private repository automatically**, then sign in to GitHub in the window Git opens (Git Credential Manager, bundled with Git for Windows). A fallback opens GitHub's new-repository page with the name and Private pre-filled; paste the URL back. Public repositories are always refused.
   - **OneDrive / Google Drive** — pick a subfolder inside the synced drive; the drive app does the syncing and Workbench only reads and writes that folder. If the app is missing, Workbench pastes the official winget install command into a terminal; if it is installed but not signed in, Workbench opens it for you.
2. Every project you open is linked automatically: its `.project-memory` becomes a link into the shared location (one folder per project), without changing the project repository (memory is ignored through this clone's `.git/info/exclude`; no commit, no `.gitignore` edit, and an `AGENTS.md` the project tracks itself is left alone). Projects that still track memory files, or whose local memory differs from the shared copy (for example a second clone of the same repository), are not linked automatically: Workbench notifies you and you decide under **This project**. GitHub mode pulls when a project opens and syncs in the background every 10 minutes.
3. On another computer, enable memory sync with the same storage; projects link to the existing memory when opened.

Switching storage later migrates linked projects on open and keeps the old copy. See [docs/two-pc-workflow.md](docs/two-pc-workflow.md) for a two-computer walkthrough.

## Installation

### Method 1: One-Line Quick Install (Windows PowerShell)

Run this command in **PowerShell** (no Git or Node.js required):

```powershell
irm https://raw.githubusercontent.com/zkylek1212-k/Multi-Harness-ADE/master/install.ps1 | iex
```

> **What this does:** Automatically fetches the latest release from GitHub, downloads the Windows setup installer (`.exe`), and starts the installation.
>
> *Options:*
> - **Silent install** (no wizard prompt):
>   ```powershell
>   $env:INSTALL_SILENT=1; irm https://raw.githubusercontent.com/zkylek1212-k/Multi-Harness-ADE/master/install.ps1 | iex
>   ```
> - **Download installer only** (saves to current folder without running):
>   ```powershell
>   $env:INSTALL_DOWNLOAD_ONLY=1; irm https://raw.githubusercontent.com/zkylek1212-k/Multi-Harness-ADE/master/install.ps1 | iex
>   ```

### Method 2: Direct Download from GitHub Releases

Prefer manual download? Grab the setup package directly from GitHub:

- 🚀 **[Download Latest Release](https://github.com/zkylek1212-k/Multi-Harness-ADE/releases/latest)**
- Download `Agent-Workbench-<version>-setup.exe` and double-click to install.
- *(Portable version: download `Agent-Workbench-<version>-portable.zip`, extract it, and run `Agent Workbench.exe` directly without installation.)*

---

## Development & Build from Source

For developers who want to contribute or build from source:

### Requirements

- Node.js 18+ (LTS recommended)
- On Windows the terminal uses a prebuilt native binary (`@lydell/node-pty`); no compiler toolchain is required.

### Getting started

```bash
git clone https://github.com/zkylek1212-k/Multi-Harness-ADE.git
cd Multi-Harness-ADE
npm install
npm run dev        # build the page and launch with tauri dev
```

On Windows you can instead **double-click `dev.cmd`** in the repository folder. It installs dependencies on the first run (Node.js LTS required: `winget install OpenJS.NodeJS.LTS`) and starts the dev build, which uses its own settings folder (`%APPDATA%\agent-workbench-dev`) so it runs alongside an installed copy. Renderer changes reload live and main-process changes restart the app, so there is no need to package a build to try a change. `dev.cmd` only calls `dev.ps1` with `-ExecutionPolicy Bypass` for that one run, because Windows blocks running `.ps1` files by default.

## Build & Publish

```bash
npm run typecheck  # TypeScript check, no emit
npm run build:ui   # build the desktop and phone pages
npm run dist       # build an NSIS installer with Tauri
npm run release    # (Maintainers only) one-click build and publish to GitHub Releases
```

> **Note on `npm run release`**: This script is strictly for **project maintainers** with repository write permissions. It uses your local GitHub CLI (`gh`) authentication to upload assets. External contributors cannot publish releases or modify repository assets.

Build config is in `core/apps/desktop/tauri.conf.json`. Publishing needs `TAURI_SIGNING_PRIVATE_KEY` and `WB_UPDATER_PUBKEY`; the private key stays out of the repo.

## Project layout

```
core/            Rust backend: wb-core (one handler per IPC channel), wb-pty, wb-git, wb-proc, wb-secrets
core/apps/desktop  Tauri app: windows, dialogs, and the pipe between each page and wb-core
src/desktop      The page's side of that pipe, window frame and embedded browser element
core/crates/wb-core/src/canvas.rs  Cowork channels, node lifecycle, approvals, handoffs and stop
core/crates/wb-core/src/worktree.rs  node worktrees: create, changes, commit, merge, cleanup, discard
core/apps/wb-hook  the permission hook the CLIs run for structured approval (named pipe to the app)
src/main/remote  LAN remote bridge: HTTPS + WebSocket, local CA, web push, preview proxy
src/main/memsync memory sync core (GitHub / OneDrive / Google Drive) and bundled memory scripts
src/preload      the single IPC contract surface
src/renderer/src React UI (editor / git / terminal / preview / dashboard panels)
src/renderer/remote  the iPhone web app (second Vite entry, served by the bridge)
src/shared       types and logic both sides use (remote protocol, detectors)
docs/architecture.html  end-to-end architecture overview
docs/cowork-architecture.md  the previous Cowork (meetings), kept as a record
.project-memory  shared cross-agent memory (handoff, protocol, decisions); local-only, not tracked in this repository
dev.cmd / dev.ps1  double-click launcher for the dev build (Windows)
```

## iPhone remote control

Settings → **Remote Control** turns on a small HTTPS server inside the app that only
accepts connections from private LAN addresses (10/8, 172.16/12, 192.168/16).
On Windows, allow **Private networks** when the firewall asks the first time.

Three ports are used, starting from the one in Settings (default `47600`): the port itself
serves the phone app and its WebSocket, `port + 1` serves only the plain-HTTP page that
installs the CA certificate, and `port + 2` serves the dev-server preview proxy.

1. **Trust this computer (once).** Scan the first QR code with the iPhone Camera and open
   it in Safari. Download the profile, install it (Settings → General → VPN & Device
   Management), then enable it under Settings → General → About → Certificate Trust
   Settings. iOS only allows Home Screen web apps, service workers and push
   notifications over a trusted HTTPS connection, which is why this step exists.
2. **Install the app.** Open the app URL in Safari → Share → **Add to Home Screen**.
3. **Pair.** Open it from the Home Screen, press *Generate pairing code* on the computer,
   and type the code (or scan the pairing QR code). Home Screen apps don't share storage
   with Safari, so pair from the Home Screen app.
4. Optional: in the app's settings, turn on notifications to get an alert when an agent
   is waiting for approval or a task finishes.

What you can do from the phone:

- **Per workspace.** Each project window on the computer is its own section, with its
  terminals, its handoff notes (`.project-memory/handoff.md` of *that* workspace) and its
  Git status. Recent workspaces that have no window appear under **Other Workspaces** —
  tapping one opens it on the computer and goes straight to *New Terminal*.
- **Read / Terminal / Preview.** *Read* reflows the terminal output for a phone screen
  (soft wraps joined, TUI borders removed); *Terminal* is the real xterm you type into and
  it fits itself to the phone's width by default — there is nothing to scroll sideways
  (turn it off in the ⋯ menu to see the computer's own width); *Preview* appears when the
  terminal printed a local dev-server URL and shows that site, proxied over HTTPS.
- **Approval cards.** When an agent stops to ask, the question and its options become
  buttons, and *Running / Waiting / Idle* is decided by the computer, not by the phone's
  clock.
- **Cowork.** Open **New Terminal → Cowork** to see the channels of that workspace, each
  node's status and its approval card, and the event timeline. From the phone you can approve
  or reject, paste a message into a node and stop. Creating nodes, merging, discarding, ending
  all and clearing a stop stay on the desktop. Approval cards show file names and line counts,
  never code.
- **Up to three computers, one at a time.** The phone remembers up to three computers. The
  title on the home screen is a dropdown that switches between them **without reloading** —
  the app just points its WebSocket at the computer you picked. Add one by its LAN address
  (Settings → *Computers* → Add Computer, or from the pairing screen), pair with it once,
  and rename or forget it from the same list. Two things to know: install each computer's
  certificate on the phone once (the pairing screen links straight to that computer's
  certificate page if it can't reach it), and notifications only arrive from the computer
  that serves the app, because a Web Push subscription is tied to one address.

Security model:

- The local CA carries critical X.509 Name Constraints: it can only sign certificates for
  private IPv4 addresses and `*.local`, so even if its key leaked it could not impersonate
  public websites on your phone. Its private key is encrypted with the OS keychain
  (Windows DPAPI). *Reset certificate and devices* creates a new CA.
- Pairing codes are single-use, expire after 5 minutes, and are invalidated after 10 wrong
  attempts. Devices get a random token; only its SHA-256 is stored. Revoking a device
  disconnects it immediately.
- A paired phone has full terminal control, including starting new agents (which honour
  Bypass Mode) and turning Bypass Mode on or off. Turning it on from the phone asks for
  confirmation and shows a notification on the computer. The title bar shows a phone badge whenever a device is connected, and
  pair / connect / spawn / kill events are written to `remote/audit.log` in the app's
  user-data folder (keystrokes are never logged).
- Push notifications are end-to-end encrypted to the phone (RFC 8291) and relayed by
  Apple's push service, so the computer needs internet access for them; everything else
  stays on the LAN.
- The preview proxy never exposes a dev server on its own. It only forwards to
  `127.0.0.1`, only to a port that one of your terminals actually printed, and every
  request needs a cookie that can only be obtained with a single-use ticket issued over
  the authenticated WebSocket (60-second lifetime). Opening a workspace from the phone is
  restricted to the computer's own recent-workspace list.
- Because the phone can switch computers inside one app, the WebSocket handshake accepts
  two kinds of origin: this computer's own app, and the same app served from another
  private-LAN address. Passing that check only gets a socket — it is still dropped after
  five seconds without a valid device token, and pairing over the socket has the same
  single-use code, 5-minute expiry and 10-attempt lockout as before.

The phone client ships inside the program, built by `npm run build:ui`.

## Configuration notes

- `src/preload/index.ts` is the **single IPC contract** between main and renderer.
- `src/renderer/src/store.ts` holds cross-panel state.
- Theme colors come from CSS variables in `src/renderer/src/styles.css` — do not
  hard-code colors inside panels.
- Per-machine runtime state (`.workbench/settings.json`,
  `.workbench/dashboard-state.json`) is git-ignored; `.workbench/extensions.yaml`
  is the checked-in template. CLI and document-tool paths are auto-detected at
  runtime and default to unspecified until configured in the app.
- Dashboard caches (`dashboard-cache.json`, `usage-cache.json`) live in the app's
  user-data folder, shared by all windows. Each window decides its own
  "Current Workspace"; the main process never uses the focused window to
  attribute sessions.
- `npm run dev` uses its own user-data folder (`agent-workbench-dev`), so a dev
  build can run alongside an installed copy without sharing settings or caches.

## Release Notes & Changelog

See [CHANGELOG.md](CHANGELOG.md) for full version history and release details.

## License

[MIT](LICENSE) © 2026 zkylek1212-k.

## Trademark & affiliation notice

This project is an independent tool and is **not affiliated with, endorsed by, or
sponsored by** Anthropic, OpenAI, Google, or Microsoft. "Claude Code", "Codex",
"Antigravity", "VS Code", and other product names are trademarks of their
respective owners and are used here only nominatively to describe interoperability.
Agent Workbench bundles none of those products; it launches whichever CLIs the
user has installed.

## Third-party software

All bundled runtime dependencies are permissively licensed, including (MIT)
`monaco-editor`, `@monaco-editor/react`, `@xterm/xterm`, `react`, `react-dom`,
`react-markdown`, `rehype-highlight`, `remark-gfm`, `mermaid`, `simple-git`,
`js-yaml`, `@lydell/node-pty`, `ws`, and `qrcode`, plus `node-forge` (BSD-3-Clause,
used to create the remote-control certificates). Their license terms continue to
apply to those components.

The terminal dev-server URL detection (`src/renderer/src/panels/terminal/portDetect.ts`)
is adapted from [AgentsDock](https://github.com/ZhengyiLuo/AgentsDock), licensed under
the Apache License 2.0.

---

# 繁體中文說明

一個輕量、**agent-native** 的開發工作台：Monaco 程式碼編輯器、Git 視覺化面板，以及
N 個內嵌 CLI 終端，以及放 agent 終端的 Cowork 畫布。以 Tauri、Rust + React + Vite 打造。

> **設計原則——由官方 CLI 負責 agent runtime。** 工作台使用各廠商的官方 CLI 與既有登入，
> 提供編輯、Git、終端、共享記憶，以及 Cowork 畫布：多個 CLI 並排、各自在自己的 Git worktree，
> 要合併什麼由你決定。跨 CLI 交接透過 `.project-memory/`（handoff 筆記 + Git）完成。

## 功能

- **Monaco 編輯器**——與 VS Code 同款編輯器核心（採 MIT 授權的 `monaco-editor`），
  提供編輯與 diff 檢視，並支援從 Git 面板開啟 commit 層級的 diff。
- **Git 面板**——狀態、暫存、commit、切換分支、commit graph，以及最近 commit／檔案 diff。
- **多 CLI 終端**——每個 CLI 各有一個 `xterm.js` 終端分頁，透過 `node-pty` 以子行程原生執行。
  不儲存金鑰；CLI 使用其自身的訂閱／驗證。
- **Cowork 畫布**——第三種模式：Claude Code、Codex、Antigravity 與自訂 launcher 以互動式終端節點放在可平移縮放的畫布上，各自一個 worktree。審批卡、拉線交接、預覽後提交／合併／捨棄，以及整個 repo 的停止。詳見下方〈Cowork 使用方式〉。
- **可調整版面**——拖曳標題列的終端與編輯器圖示交換左右位置，並可各自開關。
- **預覽與文件**——Markdown／HTML 即時預覽（編輯與存檔自動同步），並內建 Word、Excel、PowerPoint 與 PDF 檢視器。
- **Vibe Coding 模式**——任務優先的另一種版面（設定 → 外觀 → 工作模式）：最左側圖示列（Sessions／Status／Handoff／Files／Git／Settings）游標移過去就淡入彈出、點擊可固定；中間是 Agent 終端，右側是即時成品。終端輸出的 dev server 網址（`http://localhost:PORT`）會自動開啟，Handoff 面板會整理 `.project-memory/handoff.md`。開發者模式維持原本程式碼優先的版面。
- **儀表板與遙測**——從本機 CLI 紀錄（Claude Code、Codex、Antigravity）掃描 session 清單與 token 用量，可切換**總用量／30 天／7 天／今天**，並拆分輸入／快取讀取／輸出。Claude 與 Codex 取自 CLI 自己的用量紀錄；Antigravity 紀錄沒有 token 欄位，數字為字數估算並明確標示。支援 CLI 啟用連動、資料夾群組分類與一鍵工作區切換。
- **極速啟動與按需掛載**——檔案 mtime 持久化快取與面板按需載入（Mount-on-Demand），開機掃描效能大幅提升 42 倍。
- **雙語系支援**——全系統支援嚴謹英文與繁體中文介面即時無縫切換。
- **CLI 啟動權限與略過模式**——全域開關支援切換 AI 代理（Claude Code、Codex、Antigravity）略過互動式審批確認模式，提升自動化執行流暢度。
- **iPhone 遠端控制（區網）**——在同一個 Wi-Fi 下用 iPhone 查看並回覆 CLI agent：加入主畫面的 App 會列出所有終端、同步顯示輸出、用審批卡片一鍵回覆、可以輸入或開新的 agent，agent 等你回覆時會推播通知。也可以請電腦開啟最近用過的工作區並在裡面開 agent、終端會自動配合手機寬度、預覽終端印出的 dev server，並記住最多三台電腦切換。詳見下方〈iPhone 遠端控制〉。
- **架構總覽**——桌面程式、手機程式與遠端橋接怎麼接在一起：[`docs/architecture.html`](docs/architecture.html)（用瀏覽器開啟）。

## 自訂 Agent

在 **設定 → CLI 與 Agent → Customize Agent（自訂 Agent）** 啟用開關，設定顯示名稱、執行檔（路徑或 PATH 上的指令）與參數（每行一個、不需 shell 引號），再儲存設定。終端啟動畫面、**+** 選單、提示詞派送與手機的新增終端選單就會出現此 harness。關閉後隱藏啟動與擴充編輯入口，保留設定及檔案，已開啟的終端繼續執行。

可選填 **Skills 資料夾、MCP 設定檔、Plugins 資料夾、Hooks 設定檔** 的絕對路徑，再到 **設定 → 擴充 → 自訂 Agent 擴充** 新增或編輯 UTF-8 原生檔案。Skill 常用 `my-skill/SKILL.md`；Plugin 請使用該 harness 要求的檔案。MCP 與 Hooks 直接編輯指定的完整檔案。路徑必須是 harness 實際讀取的位置；工作台不會改寫它的載入規則或轉換擴充格式。儲存既有檔案會保留 `.workbench-backup`；無效 JSON 或開啟後已被其他程式修改的檔案會拒絕儲存。資料夾清單最多六層、500 個檔案，單檔編輯上限 1 MB。

指令、參數、路徑與開關只使用本機全域設定，repo 無法覆寫。自訂 harness 使用自身的登入與權限規則，不會自動注入內建 Agent 的 bypass 或預設旗標。Cowork、供應商歷史紀錄與 token 統計仍支援原本三家 CLI；獨立的 Linux app 未變更。

## Cowork 使用方式

Cowork 是在同一個 Git repo 同時跑多個 agent 的畫布模式。2026-11 起取代原本的會議／規劃版 Cowork；主席會議、任務看板與背景執行已移除。

1. 切到 **Cowork**（設定 → 工作模式、畫布上方的模式切換，或 **Terminals → + → Cowork**）。第一次會自動建立名為 *general* 的 Channel；一個 Channel 就是一張畫布，包含節點、交接與事件。
2. 把左欄的 agent 拖到畫布，或按旁邊的 **+**。每個節點就是該 agent 真正的互動式終端，在自己的 worktree（`.cowork/<channel>/<node>`、分支 `cowork/<channel>/<node>`）啟動。勾選「新節點直接在 repo 根目錄工作」可讓節點直接在根目錄跑，同一個 repo 只允許一個。
3. Agent 要求權限時，節點邊框變色，下方出現審批卡。只有認得出問題與選項時才有「允許／拒絕」，否則請到終端回答。開啟結構化審批（設定 → Cowork）時卡片會顯示改動內容。任何故障都交還給 CLI 自己的提示，不會自動放行。
4. 從一個節點的右緣拉到另一個節點的左緣就建立**交接**：由 Git 取得來源分支、commits、改動檔案與未提交狀態組成的草稿，不會自動送出。可以複製，或按「貼入終端」（bracketed paste，不送 Enter）。
5. 右欄 Inspector：查看改動（已提交、已暫存、未暫存、未追蹤），輸入訊息「提交全部改動」，**預覽**後**合併**到你選的分支，**清理**已合併的 worktree，或在列出所有檔案（含忽略檔）的預覽後**捨棄**。合併與捨棄的預覽綁定當下的 commit 與檔案，之間有任何變動都要重新預覽。
6. **停止**（或 Ctrl+Shift+Backspace）對這個 repo 所有執行中的 Cowork 節點（跨 Channel 與視窗）送一次中斷鍵，並拒絕新節點、貼入與審批放行。**全部結束**才會結束行程；**解除停止**只讓 App 重新接受操作。

關閉節點會結束行程、保留 worktree。關閉 App 會保留畫布與 worktree；重開後所有節點顯示為已結束，可重新啟動。縮小到一半以下時節點改顯示靜態畫面。Channel 存在 repo 的 Git 目錄（`.git/cowork/`），不會被 commit。

**設定 → Cowork** 設定 worktree 的位置（請放在 OneDrive 這類同步資料夾外），以及各家的結構化審批。Claude Code 預設開啟，只影響 Cowork 的 session。Codex 第一次要在 Codex 裡信任這個 hook。Antigravity 會在 `~/.gemini/config/hooks.json` 加一筆；允許仍要經過 Antigravity 自己的改檔審查，拒絕可直接生效。Bypass 模式下不掛 hook。

## 記憶同步

`.project-memory/` 是 Claude Code、Codex、Antigravity 共同讀寫的交接筆記。**設定 → 記憶同步**讓它在你的多台電腦間保持一致，同時不放進公開的專案 repo。預設關閉；關閉時 Workbench 完全不做同步（交接只存在本機，不連網路、不多花 token）。

1. 打開同意開關，選擇記憶放哪裡：
   - **GitHub（私人 repo）**——有版本紀錄。可以安裝 GitHub CLI 讓 Workbench 建立 repo，或選**不安裝 gh**，按**自動建立私人 repo**，再在 Git 跳出的視窗登入 GitHub（Git for Windows 內建的 Git Credential Manager）。備用方式會開啟名稱與 Private 都已預填的 GitHub 建立頁，建好後貼回網址。公開 repo 一律拒絕。
   - **OneDrive／Google Drive**——在雲端硬碟的同步資料夾裡選一個子資料夾，由雲端硬碟程式負責同步，Workbench 只讀寫那個資料夾。沒安裝時，Workbench 會把官方 winget 安裝指令貼進終端；已安裝但沒登入時，會幫你開啟程式登入。
2. 之後打開的每個專案都會自動接上：`.project-memory` 變成指向共用位置的連結（每個專案一個資料夾），不會修改專案 repo（只寫這個 clone 的 `.git/info/exclude`，不產生提交、不改 `.gitignore`，專案自己追蹤的 `AGENTS.md` 也不碰）。repo 仍追蹤記憶檔、或本機記憶和共用的不同（例如同一個 repo 的另一個 clone）時不會自動接上，Workbench 會通知你到**這個專案**區塊決定。GitHub 模式在打開專案時拉取，並每 10 分鐘背景同步。
3. 另一台電腦開啟記憶同步並選同一個存放位置，打開專案時就會接上既有的記憶。

之後換存放位置，已連結的專案會在打開時搬過去並保留舊的一份。兩台電腦的完整流程見 [docs/two-pc-workflow.md](docs/two-pc-workflow.md)。

## 安裝指南

### 方法一：PowerShell 一鍵快速安裝（Windows 推薦）

在 **PowerShell** 貼上並執行以下單行指令（無需 Git 或 Node.js）：

```powershell
irm https://raw.githubusercontent.com/zkylek1212-k/Multi-Harness-ADE/master/install.ps1 | iex
```

> **說明：** 自動向 GitHub 取得最新版本安裝包（`.exe`），下載並自動啟動安裝精靈。
>
> *進階選項：*
> - **靜默自動安裝**（不彈出安裝引導畫面）：
>   ```powershell
>   $env:INSTALL_SILENT=1; irm https://raw.githubusercontent.com/zkylek1212-k/Multi-Harness-ADE/master/install.ps1 | iex
>   ```
> - **僅下載安裝檔到本地**（儲存至當前目錄，不立即執行）：
>   ```powershell
>   $env:INSTALL_DOWNLOAD_ONLY=1; irm https://raw.githubusercontent.com/zkylek1212-k/Multi-Harness-ADE/master/install.ps1 | iex
>   ```

### 方法二：直接自 GitHub Releases 下載

- 🚀 **[前往最新發行頁面（GitHub Releases）](https://github.com/zkylek1212-k/Multi-Harness-ADE/releases/latest)**
- 下載 `Agent-Workbench-<版本>-setup.exe` 雙擊即可安裝。
- *(免安裝綠色版：下載 `Agent-Workbench-<版本>-portable.zip`，解壓後直接雙擊 `Agent Workbench.exe` 即可使用)*

---

## 開發與自原始碼建置

若您欲參與開發或進行除錯：

### 需求

- Node.js 18+（建議 LTS）
- Windows 上終端使用預編譯原生二進位（`@lydell/node-pty`），無需編譯工具鏈。

### 快速開始

```bash
git clone https://github.com/zkylek1212-k/Multi-Harness-ADE.git
cd Multi-Harness-ADE
npm install
npm run dev        # 建置頁面並以 tauri dev 啟動
```

Windows 上也可以直接**雙擊 repo 資料夾裡的 `dev.cmd`**：第一次會自動安裝相依套件（需要 Node.js LTS：`winget install OpenJS.NodeJS.LTS`），然後啟動開發版。開發版使用獨立的設定資料夾（`%APPDATA%\agent-workbench-dev`），可以和已安裝的版本同時開。改畫面會即時刷新、改主行程會自動重啟，試改動不用重新打包。`dev.cmd` 只是用 `-ExecutionPolicy Bypass` 執行一次 `dev.ps1`，因為 Windows 預設禁止直接執行 `.ps1`。

## 建置與發佈 (Build & Publish)

```bash
npm run typecheck  # TypeScript型別檢查，不輸出
npm run build:ui   # 建置桌面與手機頁面
npm run dist       # 用 Tauri 產生 NSIS 安裝檔
npm run release    # （僅限專案維護者）一鍵自動編譯、打包並發布至 GitHub Releases
```

> **關於 `npm run release` 的權限說明**：此指令為**專案維護者專用**，執行時會校驗本機 GitHub CLI (`gh`) 身分憑證。未獲授權的外部貢獻者（無倉庫寫入權限者）無法發布 Release，亦無法更動任何 GitHub 上的專案發行檔案。

建置設定見 `core/apps/desktop/tauri.conf.json`。發布需要 `TAURI_SIGNING_PRIVATE_KEY` 與 `WB_UPDATER_PUBKEY`；私鑰不進 repo。

## 專案結構

```
core/            Rust 後端：wb-core（每個 IPC 頻道一個處理函式）、wb-pty、wb-git、wb-proc、wb-secrets
core/apps/desktop  Tauri app：視窗、對話框，以及頁面與 wb-core 之間的通道
src/desktop      頁面這端的通道、視窗框與內嵌瀏覽器元素
core/crates/wb-core/src/canvas.rs  Cowork Channel、節點生命週期、審批、交接與停止
core/crates/wb-core/src/worktree.rs  節點 worktree：建立、改動、提交、合併、清理、捨棄
core/apps/wb-hook  CLI 執行的結構化審批 hook（以 named pipe 連到 App）
src/main/remote  區網遠端橋接：HTTPS + WebSocket、本機 CA、Web Push、預覽代理
src/main/memsync 記憶同步核心（GitHub／OneDrive／Google Drive）與內建的記憶腳本
src/preload      唯一的 IPC 契約介面
src/renderer/src React UI（editor / git / terminal / preview / dashboard 面板）
src/renderer/remote  iPhone 端網頁 App（第二個 Vite entry，由橋接層提供）
src/shared       兩端共用的型別與邏輯（遠端協定、各種偵測）
docs/architecture.html  端到端架構總覽
docs/cowork-architecture.md  舊版 Cowork（會議）的紀錄
.project-memory  跨 agent 共享記憶（handoff、protocol、decisions）；僅存本機，不納入版控
dev.cmd / dev.ps1  Windows 雙擊啟動開發版
```

## iPhone 遠端控制

設定 → **遠端控制** 會在 app 內啟動一個小型 HTTPS 伺服器，只接受私有區網位址（10/8、172.16/12、192.168/16）連線。Windows 第一次開啟時防火牆會詢問，請允許「私人網路」。

會用到三個 port，從設定中的那個往上數（預設 `47600`）：該 port 本身提供手機 App 與 WebSocket，`port + 1` 只提供安裝 CA 憑證的純 HTTP 頁面，`port + 2` 提供 dev server 預覽代理。

1. **信任這台電腦（只需一次）**：用 iPhone 相機掃第一個 QR code，在 Safari 開啟。下載描述檔並安裝（設定 → 一般 → VPN 與裝置管理），再到 設定 → 一般 → 關於本機 → 憑證信任設定 打開完全信任。iOS 只允許在「可信任的 HTTPS」下使用主畫面 App、Service Worker 與推播，所以需要這一步。
2. **安裝 App**：用 Safari 開啟 App 網址 → 分享 → **加入主畫面**。
3. **配對**：從主畫面開啟，在電腦上按「產生配對碼」後輸入（或掃配對 QR code）。主畫面 App 與 Safari 不共用儲存空間，請在主畫面 App 內配對。
4. 選用：在 App 的設定開啟通知，agent 等待審批或任務結束時會收到提醒。

手機上能做什麼：

- **以工作區為單位**：電腦上每個專案視窗在手機上就是一個區塊，各自有自己的終端、交接筆記（**該工作區**的 `.project-memory/handoff.md`）與 Git 狀態。最近用過但目前沒有視窗的工作區列在**其他工作區**，點一下讓電腦開起來並直接跳到「新增終端」。
- **閱讀／終端／預覽**：「閱讀」把終端輸出整理成手機讀得懂的樣子（接回軟換行、去掉 TUI 框線）；「終端」是真正可以打字的 xterm，預設自動配合手機寬度，不需要左右拖（在 ⋯ 選單關掉就回到電腦的寬度）；「預覽」在終端印出本機 dev server 網址時出現，把那個網站以 HTTPS 代理過來顯示。
- **審批卡片**：agent 停下來問你時，問題與選項會變成按鈕；「執行中／等你回覆／閒置」由電腦判定，不是用手機的時鐘去猜。
- **Cowork**：從「新增終端 → Cowork」查看該工作區的 Channel、各節點狀態與審批卡，以及事件時間軸。手機上可以允許或拒絕、把訊息貼進節點，以及停止；建立節點、合併、捨棄、全部結束與解除停止只在桌面。審批卡只顯示檔名與增減行數，不顯示程式碼。
- **最多三台電腦、一次連一台**：手機會記住最多三台電腦，主畫面的標題就是切換用的下拉選單，**切換不會重新載入**——App 只是把 WebSocket 指向你選的那台。用區網位址新增（設定 →「電腦」→ 新增電腦，或在配對畫面直接加），各配對一次，同一份清單也能改名與移除。兩件要知道的事：每台電腦的憑證要在手機上各裝一次（連不到時配對畫面會直接給你那台的憑證安裝頁連結）；通知只會來自送來這個 App 的那台電腦，因為 Web Push 訂閱綁在單一網址上。

安全設計：

- 本機 CA 帶 critical 的 X.509 Name Constraints，只能簽私有 IPv4 與 `*.local`；即使私鑰外洩也簽不出能在手機上冒充公開網站的憑證。私鑰以 OS 金鑰（Electron `safeStorage`）加密。「重設憑證與所有裝置」會產生新的 CA。
- 配對碼一次性、5 分鐘失效、錯 10 次作廢。裝置取得隨機 token，本機只存 SHA-256；撤銷裝置會立即斷線。
- 已配對的手機擁有完整終端控制權，包括開新的 agent（會套用 Bypass 模式）與開關 Bypass 模式；從手機開啟 Bypass 需先確認，電腦上會跳出通知。有裝置連線時標題列會顯示手機標示；配對、連線、開關終端等事件記錄在 app 使用者資料夾的 `remote/audit.log`（不記錄任何輸入內容）。
- 推播內容以 RFC 8291 端對端加密給手機，經 Apple 推播服務轉送，因此推播需要電腦能連網；其餘流量都只在區網內。
- 預覽代理本身不會把任何 dev server 曝露出去：它只連 `127.0.0.1`、只連「你的終端真的印出過」的 port，而且每個請求都需要 cookie，那個 cookie 只能用經認證的 WebSocket 發出的一次性 ticket（60 秒有效）換到。從手機開啟工作區也只限電腦自己的最近清單。
- 因為手機能在同一個 App 裡切換電腦，WebSocket 握手接受兩種來源：這台電腦自己的 App，以及同一個 App 由區網上另一個私有位址提供。通過這關只是拿到一條連線——五秒內沒有有效的裝置 token 一樣會被斷開，而走連線的配對仍是一次性配對碼、5 分鐘失效、錯 10 次作廢。

手機端頁面隨程式一起打包，由 `npm run build:ui` 建置。。

## 設定備註

- `src/preload/index.ts` 是主行程與 renderer 之間的**唯一 IPC 契約**。
- `src/renderer/src/store.ts` 保存跨面板狀態。
- 主題顏色來自 `src/renderer/src/styles.css` 的 CSS 變數——請勿在面板內寫死顏色。
- 每台機器各自的 runtime state（`.workbench/settings.json`、`.workbench/dashboard-state.json`）
  已被 git 忽略；`.workbench/extensions.yaml` 為納入版控的範本。CLI 與文件工具路徑於執行時
  自動偵測，在 app 內設定前預設為未指定（unspecified）。
- 儀表板快取（`dashboard-cache.json`、`usage-cache.json`）放在 app 的使用者資料夾，所有視窗共用。
  「當前工作區」由各視窗自行判斷；主行程不會以「目前聚焦的視窗」來歸屬 session。
- `npm run dev` 使用獨立的使用者資料夾（`agent-workbench-dev`），可與已安裝版本同時執行，互不共用設定與快取。

## 版本紀錄與變更日誌

請參閱 [CHANGELOG.md](CHANGELOG.md) 了解詳細的版本歷程與更新內容。

## 授權

[MIT](LICENSE) © 2026 zkylek1212-k。

## 商標與關聯聲明

本專案為獨立工具，**與 Anthropic、OpenAI、Google、Microsoft 無任何關聯、亦未獲其背書或贊助**。
"Claude Code"、"Codex"、"Antigravity"、"VS Code" 等產品名稱為各自所有者之商標，於此僅作說明
互通性之用（nominative use）。本工作台不捆綁上述任何產品，只啟動使用者自行安裝的 CLI。

## 第三方軟體

所有捆綁的 runtime 依賴皆為寬鬆授權（MIT），包含 `monaco-editor`、`@monaco-editor/react`、
`@xterm/xterm`、`react`、`react-dom`、`react-markdown`、`rehype-highlight`、`remark-gfm`、
`mermaid`、`simple-git`、`js-yaml`、`@lydell/node-pty`、`ws`、`qrcode`，以及 `node-forge`（BSD-3-Clause，
用於產生遠端控制憑證）。這些元件仍受其各自授權條款約束。

終端 dev server 網址偵測（`src/renderer/src/panels/terminal/portDetect.ts`）改寫自
[AgentsDock](https://github.com/ZhengyiLuo/AgentsDock)，採 Apache License 2.0 授權。
