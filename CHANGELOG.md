# Changelog

## [0.2.0] - Unreleased

### Changed / 變更
- Windows 版改用 Tauri 與 Rust 後端，不再使用 Electron。介面與手機頁面不變，設定、會議紀錄與已配對的手機會沿用。
- The Windows app now runs on Tauri with a Rust backend instead of Electron. The interface and phone page are unchanged; settings, meeting records and paired phones carry over.
- 安裝檔由 83 MB 降到約 9 MB；記憶體用量與先前相近，執行環境需要 Microsoft Edge WebView2。
- The installer drops from 83 MB to about 9 MB. Memory use is about the same as before, and the app needs the Microsoft Edge WebView2 runtime.

### Upgrading / 升級
- 0.1.x 不會自動更新到這一版，請手動下載安裝；安裝時會移除舊版，資料會保留。
- 0.1.x does not auto-update to this version. Download and install it by hand; the installer removes the old version and keeps your data.
- 之後的更新改用簽章驗證。
- Updates from here on are signature-checked.

## [0.1.48] - 2026-11-06

### Added / 新增
- 手機遠端控制支援 NordVPN Meshnet 與 Tailscale：「設定 → 遠端控制」新增「允許透過 mesh VPN 連線」開關，預設關閉。打開後，mesh 位址會出現在連線 IP 清單。只有走 NordLynx／Tailscale 網卡進來的連線會被放行。（PR #63）
- Phone remote control now works over NordVPN Meshnet and Tailscale: Settings → Remote Control has an "Allow connections over a mesh VPN" switch, off by default. When on, the mesh address appears in the connection IP list. Only connections that arrive on a NordLynx or Tailscale interface are accepted.
- 第一次打開這個開關會更換本機憑證（會先跳確認）：每支手機都要重新安裝才能再連線，已配對的裝置不會解除配對。
- Switching it on for the first time replaces this computer's certificate, after a confirmation: every phone must reinstall it before it can connect again. Paired devices stay paired.

## [0.1.47] - 2026-10-30

### Added / 新增
- Antigravity 也改用內附的 shared-project-memory plugin（1.3.0），和 Claude Code／Codex 同一套：開新 session 時由 hook 自動載入最新記憶，並提供 `/handoff`。開啟設定時，已啟用的 Antigravity 若還沒裝、版本較舊，或裝的是 GitHub 版，會一起詢問是否安裝或換成內附版。（PR #60）
- Antigravity now uses the bundled shared-project-memory plugin (1.3.0), the same way as Claude Code and Codex: a SessionStart hook loads the live memory and `/handoff` is available. When Settings opens, an enabled Antigravity that lacks the plugin, has an older copy, or has the GitHub install is included in the install offer.

### Changed / 變更
- `/handoff` 改成 plugin 的 skill，三家都能用：Claude Code 與 Antigravity 打 `/handoff`，Codex 打 `$handoff`。（PR #60）
- `/handoff` is now a plugin skill, so all three agents have it: `/handoff` in Claude Code and Antigravity, `$handoff` in Codex.
## [0.1.46] - 2026-10-29

### Added / 新增
- Workbench 內附 shared-project-memory plugin（1.2.0）。開啟設定時，若已啟用的 Claude Code／Codex 還沒裝、版本較舊，或裝的是 GitHub 版，會詢問是否安裝或換成內附版：先裝好新版並確認成功，才移除舊版。拒絕時會說明交接會少什麼；「設定 → 記憶同步」隨時可以安裝。（PR #58）
- Agent Workbench ships the shared-project-memory plugin (1.2.0). When Settings opens, it offers to install it into every enabled Claude Code / Codex that lacks it, has an older copy, or has the GitHub install. The new one is installed and verified before the old one is removed. Declining explains what the handoff loses; Settings → Memory sync can install it any time.
- 沒裝 plugin 時，Claude Code 也會從 `.claude/rules/shared-memory.md`（只在本機 clone）載入記憶快照；裝了 plugin 就移除這個檔，避免重複載入。（PR #58）
- Without the plugin, Claude Code now loads a memory snapshot from `.claude/rules/shared-memory.md` (local to the clone); the file is removed once the plugin is installed so memory is not loaded twice.

### Changed / 變更
- plugin 改在 Workbench repo 的 `plugins/shared-project-memory/` 維護，記憶腳本只有這一份來源；GitHub 的 ShareProjectMem 已封存。（PR #58）
- The plugin is now maintained in `plugins/shared-project-memory/` of this repository and is the only source of the memory scripts; the GitHub ShareProjectMem repository is archived.

## [0.1.45] - 2026-10-29

### Added / 新增
- 設定 → 記憶同步：Claude Code 沒有安裝 shared-project-memory plugin 時，會顯示提示和「安裝 plugin」按鈕（把安裝指令貼進新的終端）。記憶的存放與同步仍由 Workbench 負責；Claude Code 開局載入記憶和 `/handoff` 要靠這個 plugin。（PR #56）
- Settings → Memory sync warns when Claude Code does not have the shared-project-memory plugin, with an "Install the plugin" button that pastes the install commands into a new shell. Workbench still stores and syncs the memory; the plugin loads it at Claude Code startup and provides `/handoff`.

### Changed / 變更
- 記憶腳本（`commit-handoff.sh`、`status.sh`、`sync-entry-files.sh`）與 shared-project-memory plugin v1.1.0 完全相同，兩邊對「記憶絕不進專案 repo」的規則一致。（PR #56）
- The memory scripts are now identical to shared-project-memory plugin v1.1.0, so both agree that memory never goes into the project repository.

## [0.1.44] - 2026-10-29

### Changed / 變更
- 記憶同步的「自動接上專案」不再修改專案 repo：只寫入這個 clone 的 `.git/info/exclude`，不會產生 commit，也不改 `.gitignore`。repo 仍追蹤記憶檔，或本機記憶和共用的不同（例如同一個 repo 的另一個 clone）時，不會自動接上，會發一次通知，請你到 設定 → 記憶同步 → 這個專案 決定。（PR #52）
- Memory sync auto-link no longer changes the project repository: it only writes this clone's `.git/info/exclude` (no commit, no `.gitignore` edit). Projects that still track memory files, or whose local memory differs from the shared copy, are not linked automatically. You get a notification and decide under Settings → Memory sync → This project.

### Fixed / 修復
- 專案自己追蹤的 `AGENTS.md`、`.agents/rules/shared-memory.md` 不會再被取消追蹤，也不會被寫入交接內容，避免記憶變成專案 repo 的待提交改動。（PR #52）
- An `AGENTS.md` or `.agents/rules/shared-memory.md` that the project tracks itself is never untracked or written with the handoff, so memory can no longer become a pending change of the project repository.
- 儀表板每個會話的「封存」按鈕沒有作用（封存和還原都無效）。（PR #54，Issue #53）
- The per-session Archive button on the Dashboard did nothing, for both archive and restore.

## [0.1.43] - 2026-10-29

### Fixed / 修復
- 從空白工作區（預設啟動狀態）第一次切換資料夾時，現在也會接續該專案最後使用的 CLI。只有啟動時從後端還原的那一次不算切換。（PR #50）
- Switching folders for the first time from the empty startup workspace now resumes that project's last CLI; only the startup restore from the backend is skipped.

### Changed / 變更
- 從儀表板切換資料夾後會留在儀表板，不再跳到檔案頁。「開啟資料夾」與「Clone」仍會切到檔案頁。（PR #50）
- Switching folders from the dashboard keeps the dashboard open; Open folder and Clone still go to Files.

## [0.1.42] - 2026-10-28

### Added / 新增
- **記憶同步**：設定 → 記憶同步，可把 `.project-memory/`（AI 交接筆記）放在私人 GitHub repo、OneDrive 或 Google Drive，在多台電腦間同步，不會進入公開的專案 repo。預設關閉；關閉時不同步也不連網。公開 repo 一律拒絕。（PR #48，原 #47）
- **Memory sync**: Settings → Memory sync keeps `.project-memory/` in a private GitHub repository or a OneDrive / Google Drive folder across computers, never in the public project repository. Off by default and offline while off; public repositories are refused.
- 拖曳標題列的終端／編輯器圖示可交換左右位置，並可各自開關；檔案樹新增「在檔案總管開啟專案資料夾」；切換工作區時選取該專案最近使用的 CLI。
- Drag the terminal/editor icons to swap sides and toggle each pane; open the project folder from the file tree; switching workspace selects that project's last CLI.
- Cowork：任務會顯示主席的指派理由；agent 改到任務範圍外的檔案時不會 commit，任務會停下讓你處理；下游任務會收到前置任務的改動清單與交接內容。
- Cowork shows each task's assignment reason, holds out-of-scope changes for review, and passes structured handoffs to dependent tasks.

### Changed / 變更
- 儀表板的會話紀錄與 token 掃描改在 worker thread 執行；CLI 偵測與 `--help` 探測不再卡住主行程。新的 app 圖示。
- Dashboard scanning runs in a worker thread and CLI detection no longer blocks the main process. New app icon.
- CLI 安裝改為把官方安裝指令貼進可見的終端，失敗時改用 npm（Codex：winget 再 npm）；shell 安裝不再隱藏 404。
- CLI installs paste the official command into a visible terminal and fall back to npm (Codex: winget, then npm); shell installs no longer hide a 404.

### Security / 安全
- 公開 repo 的歷史已移除 `.project-memory/`、`AGENTS.md`、`.agents/`。pre-commit 會擋下這些路徑（包含 force-add）。記憶同步關閉時也會替換舊版 `commit-handoff.sh`（舊版會用 `git add -f` 把記憶提交進專案 repo）。
- Project memory and generated entry files were removed from the public history. The pre-commit hook now blocks them, including force-adds. The legacy `commit-handoff.sh`, which force-added memory into the project repository, is replaced even when memory sync is off.

### Notes / 備註
- 記憶同步只有本機自動化測試，尚未實測真正的 GitHub／OneDrive／Google Drive 設定流程。Linux 維持獨立的 v0.1.0。
- Memory sync was covered by local automated checks only; real GitHub / OneDrive / Google Drive setup has not been tested. Linux remains the separate v0.1.0.

## [0.1.41] - 2026-10-16

### Added / 新增
- **Customize Agent**：預設關閉，可指定自己的 harness 名稱、執行指令與參數；啟用後才顯示桌面／手機啟動入口及 Skills／MCP／Plugins／Hooks 原生檔案新增與編輯。設定僅存本機，檔案編輯包含備份、外部變更檢查與路徑保護。（PR #45）
- **Custom harness support**: opt-in terminal launch and native Skills/MCP/Plugins/Hooks editing, with local-only configuration, backups and file conflict/path checks. Supports one custom harness; extension formats and discovery paths follow that harness.

### Changed / 變更
- 手機 Cowork 改從「新增終端 → Cowork」開啟，並補齊桌面的模式、主席、模型／推理強度、摘要 Agent、摘要時機、專案基線與預算選項；支援略過失敗發言人、將結論轉成專案提案及刪除已結束會議。（PR #44）
- Phone Cowork now opens through New Terminal and matches desktop meeting options, including models/effort, chair, recorder and limits, plus failed-speaker recovery and conclusion-to-project conversion.

### Fixed / 修復
- 修復手機提高 Cowork 預算後未重試，以及無回傳資料的成功操作被誤判為失敗。（PR #44）
- 修復新版 Claude Code 模型名稱顯示成說明文字；正確顯示模型版本。（PR #44）

### Notes / 備註
- 自訂 harness 不會自動套用內建 Agent 的 bypass 或預設旗標；Cowork、供應商歷史及用量統計仍支援原本三家 CLI。Linux 維持獨立的 v0.1.0。

## [0.1.40] - 2026-10-09

### Added / 新增
- 手機版 Cowork：每個工作區多一個 Cowork 入口，可查看會議列表、開新會議（模式／與會者／主席，沿用桌面設定的模型）、即時跟著討論與任務板，並依階段追問、核准、重試、背景執行、暫停／繼續、合併與清理；取消需確認。任務板編輯與模型選擇仍只在桌面。（PR #42）
- 獨立 Linux 版（`apps/linux`，v0.1.0）與 Ubuntu CI 打包 AppImage／deb／tar.gz；Windows 版程式與安裝檔不變。（PR #41）

### Notes / 備註
- 在桌面開的會議，手機列表要重新整理或重連後才會出現。

## [0.1.39] - 2026-10-08

### Added / 新增
- Cowork 多 agent 會議：Discussion 公開依序討論與追問、指定摘要 agent 保存累積紀錄、主席總結，以及可編輯的轉 Project 提案。
- Project 專案規劃、獨立審查、任務板與使用者核准後背景執行；支援 worktree 任務相依／資源排程、結果檢視、合併與清理。
- Cowork 多分頁，入口整合至終端 `+` 選單與 New Terminal 卡片；可同時討論／規劃，同一 repository 的實際執行限制一場。
- Cowork 設定包含參與者、主席、摘要 agent、完整版本模型、自動／手動推理強度及會議預算；README 補上使用方式與雙語功能架構。
- 開發者模式預設收合中央 Preview，標題列提供一鍵切換按鈕。（PR #35）

### Fixed / 修復
- Cowork Windows Claude CLI 啟動、取消確認視窗主題、agent 卡片對齊與 Settings 窄視窗／中英文排版；移除模型 Custom 輸入選項。（PR #36）
- 手機審批改以目前已解析的終端畫面偵測，避免舊輸出誤觸發、空白選項與提示消失後殘留審批卡片。（PR #37）

## [0.1.38] - 2026-10-07

### Fixed / 修復
- 桌面終端微軟注音重複輸入：組字緩衝區滿了會先送出前段、後段繼續組字，xterm 5.5 以舊位置切 textarea，把還在組字的後段一起送出、之後又送一次。組字事件改由 `imeGuard` 全權處理，只送 `compositionend.data`，組字預覽照常顯示。
- 手機端 Claude Code / Antigravity 畫面疊成好幾份、分隔線斷成數行：
  - 手機 snapshot 原本是用現在寬度重播原始輸出紀錄；改為 main 為每個 PTY 維護 headless xterm，snapshot 為其序列化畫面，輸出與尺寸變更依同一順序送到桌面與手機。
  - Windows ConPTY 改尺寸後會重畫可見區：改尺寸前先清可見區，舊畫面不再被 reflow 推進捲動歷史；Claude / Antigravity 會整段重印對話，連捲動歷史一起清，只留一份。
  - 桌面改尺寸合併為停止拖動後一次，窗格小於 20×5 時不縮 PTY。

## [0.1.37] - 2026-10-07

### Fixed / 修復
- 手機端 Claude Code 終端：Claude Code 的 fullscreen TUI（`"tui": "fullscreen"`）會進入備用螢幕＋滑鼠模式，手機只剩一頁、無 scrollback。現在啟動 Claude 時自動注入 `CLAUDE_CODE_DISABLE_ALTERNATE_SCREEN=1`（等同 Codex 的 `--no-alt-screen`；launcher 自訂 env 優先）。
- 手機審批面板只顯示「1 / 2 / 3」：Claude 把游標停在 spinner 列（選項上方），解析只往游標上方讀而抓不到選項；改為讀到畫面最後一列有內容處，選項文字恢復顯示。
- 移除 v0.1.36 的 `ClaudeHistoryStream`：它把每次清螢幕轉成分隔線＋整頁空行，造成手機畫面重複畫面與大段空白。

## [0.1.36] - 2026-10-05

### Added / 新增
- 終端跨 Agent 歷史回溯強化（DEC-007）：
  - **Codex CLI**：自動注入 `--no-alt-screen` 通用參數，停用備用螢幕模式（Alternate Screen Mode），切換為 inline 串流輸出，完整保留 xterm scrollback 歷史。
  - **Claude Code CLI**：在手機端終端視圖實作 `ClaudeHistoryStream`，攔截 React Ink 產生的 ANSI 清螢幕序列（`\x1b[2J\x1b[H`），轉化為向 scrollback 推進換行並自動繪製淡色橫向回合分隔線（`─`），使前幾回合的交談與工具輸出自然保留在卷軸緩衝區中，隨時可向上滑動回顧多回合交談紀錄。
  - 手機端 upstream prompt 輸入框、快捷按鍵（如 Tab、Ctrl+C）與審批按鈕走獨立管道，完全不受影響，保持 100% 正常操作。

## [0.1.35] - 2026-10-02

### Fixed / 修復
- 手機 Terminal 維持可讀字級：先以桌面欄／列座標解析 ANSI，再將文字依手機寬度換行，保留色彩、文字樣式、游標及上下滑動；移除整張桌面畫布縮放，無需左右滑動，也不改變桌面 PTY 尺寸。
- 修復切換 Terminal／Status／File／Preview、重新開啟會話及桌面尺寸更新後的殘缺畫面；保留歷史閱讀位置與新輸出自動跟隨，並驗證中文、emoji、長網址及 ANSI 相對游標重畫。

## [0.1.34] - 2026-10-02

### Fixed / 修復
- 手機端 Terminal 畫布縮放與游標座標修復：保留桌面 PTY 原生 ANSI 欄位幾何（鏡像 120/160 欄寬），移除導致維度錯位的客戶端強制換行；採用 CSS transform 等比縮放畫布（Scale-to-Fit）適配手機螢幕寬度，完全杜絕 TUI 互動進度列與長分隔線造成的文字重疊、欄位錯位與重複印出問題；外層容器維持 `overflow-x: hidden` 無需水平滑動，並保留順暢的原生垂直滑動查閱交談歷史紀錄。（PR #28）

## [0.1.33] - 2026-10-02

### Fixed / 修復
- 手機端 Terminal 自適應寬度與體驗重構：採用 `@xterm/addon-fit` 依手機螢幕寬度動態適配欄數，長行自動軟折行，字體大小清晰好讀；徹底移除左右水平滑動（`overflow-x: hidden`、只保留垂直手勢），手機端查看完全不影響電腦端 PTY 尺寸；保留順暢的原生垂直慣性滾動，可隨時往上滑動查閱完整交談歷史。（PR #27）

## [0.1.32] - 2026-10-02

### Added / 新增
- 手機端工作區分組摺疊：在首頁以原生 `<details>`/`<summary>` 摺疊各工作區之終端卡片、快捷按鍵與新增按鈕，提供 44px 觸控標靶、終端計數與待審批標籤，並於手機端本地持久化保存摺疊偏好。（PR #26）

### Fixed / 修復
- 手機端終端捲動與排版修復：改用原生雙軸捲動表面，不縮減桌面 PTY 尺寸即可在手機端完整閱讀歷史輸出；修復 CLI 清除畫面（CSI 3 J）時滾動歷史遺失問題，並強化輸出排版自動捲動跟隨。（PR #25）

## [0.1.31] - 2026-10-01

### Added / 新增
- 手機端新增 Status 面板與唯讀 File 檔案預覽（支援 Markdown、PDF、HTML 與純文字）。
- 手機端快捷鍵縮小並固定單排。

## [0.1.30] - 2026-10-01

### Changed / 變更
- Vibe 模式配色改為與 Dev 模式一致：移除 Vibe 專用的紫色主色，漸層與 Agent 工作中光暈改跟隨目前主題的主色。

## [0.1.29] - 2026-10-01

### Added / 新增
- Vibe 模式 Status 面板顯示目前工作區的 Agent 背景任務（Claude、Codex、Antigravity），含執行中／完成／失敗／取消狀態與經過時間。

### Fixed / 修復
- 修復 Agent 修改的檔案一多，Status 面板每列被壓扁到看不見文字的問題（改為整個面板捲動）。

## [0.1.28] - 2026-10-01

### Fixed / 修復
- 修復桌面終端中文組字被 xterm 提早送出、結束組字時再次送出的問題；輸入法直接輸入標點時只送出新文字，並清除已提交的隱藏輸入框內容。
- 手機終端改用 xterm FitAddon 的實際字元尺寸調整欄數，直橫向、重連及桌面尺寸變更後皆重新排版，移除橫向捲動與切回桌面寬度的選項。
- 首頁 Grid 與子卡片皆限制可用寬度；手機設定顯示介面版本並提供重新載入按鈕，避免桌面更新後仍使用已開啟的舊頁面。
- 新增 Electron／Chromium 回歸檢查，涵蓋組字重送、原生組字提交、標點、Enter、貼上及手機不同寬度與像素密度。

## [0.1.27] - 2026-10-01

### Fixed / 修復
- 修復手機遠端終端未能依可用寬度自動縮放，以及長 session 名稱撐寬首頁的問題。
- 修復中文輸入法組字時按鍵被誤判為 Ctrl+V。

## [0.1.26] - 2026-10-01

### Fixed
- Wrap Windows Claude Code MCP servers launched with `npx` in `cmd.exe /c` to prevent startup connection failures.

All notable changes to the Agent Workbench project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [0.1.25] - 2026-10-01

### Added / 新增
- Remote Control 設定頁可選擇手機連線用的 Wi-Fi、有線網路或 VPN IP；憑證安裝網址、App 網址與配對 QR 都使用所選位址，並驗證配對目標是目前偵測到的網卡位址。

## [0.1.24] - 2026-10-01

### Added / 新增
- **手機預覽成品**（Session → 預覽）：終端印出本機 dev server 網址時出現「預覽」分頁，直接在 App 裡看跑起來的畫面。橋接層在 `port + 2` 開一台 HTTPS 反向代理——手機的 `localhost` 是手機自己、dev server 多半只綁 `127.0.0.1`、而 https 頁面不能框住 http，所以必須代理；給它自己一個 origin，被預覽頁面的根目錄相對路徑才會解對，不必改寫 HTML。HMR 的 WebSocket 一併轉發。
- **手機可選工作區**：首頁新增「其他工作區」，列出最近用過但目前沒開視窗的工作區；點一下讓電腦開起來，並直接跳出「新增終端」。
- **手機記住最多三台電腦**（設定 → 電腦）：標題就是切換用的下拉選單，**切換不重新載入**；用區網位址新增、可改名與移除，每台各自配對一次。

### Changed / 變更
- **終端自動配合手機寬度**：進「終端」分頁就把 pty 的欄數改成手機放得下的寬度（字級回到 12–13px），不再需要左右拖；離開時還給電腦。轉向或鍵盤收合後重算一次。
- **「執行中／閒置」改由電腦判定**並在狀態翻轉時廣播（新的 `activity` 訊息），不再由手機拿自己的時鐘去比對桌面寫的時間戳。
- `agents/*.yaml` 裡只是把內建 CLI 換個名字的 launcher，不再在手機上重複列出。
- 遠端橋接現在使用三個 port：`port`（App + WebSocket + API）、`port + 1`（只提供 CA 憑證安裝頁）、`port + 2`（預覽代理）。

### Fixed / 修復
- **中文輸入法打字時文字被重複送出**：組字進行中若程式去搶終端的 focus（Dashboard 跳分頁、拖拉交接、分頁顯示、尺寸重算），Chromium 會中止組字並把未送出的字留在 xterm 的隱藏 textarea 裡，下一個組字鍵就會把它當成新輸入再送一次。組字中一律不搶 focus、不改尺寸。
- **Ctrl+V 貼上兩次**：自訂按鍵處理只回傳 `false` 並不會取消瀏覽器的預設貼上，而 xterm 自己也監聽 textarea 的 `paste`。另移除 Electron 的預設選單（Windows／Linux）：它註冊的 `Ctrl+Z`／`Ctrl+A` 等編輯快速鍵會動到 xterm 的隱藏 textarea，`Ctrl+Z` 會把剛打的字還原後再送進終端。
- Markdown 的 shell 指令按「送到終端」時，不再可能落進正在執行的 agent 提示字元。
- 憑證涵蓋範圍改為逐項比對：`192.168.1.1` 不會再被 `…1.10` 誤判成已涵蓋而不重簽。
- Dashboard 掃描 CLI 紀錄檔時加上界限（候選數、路徑長度、往上層數、標題解析視窗、單檔大小），異常的紀錄檔不再卡住主行程。

### Security / 安全
- **工作區檔案一律當成不受信任的輸入**（打開別人的 repo 不該由那個 repo 決定要執行什麼）：
  - `.workbench/settings.json` 不再能改 CLI 解析到哪個執行檔、也不能開啟 Bypass 模式——這些只認使用者的全域設定。
  - `agents/*.yaml` 的 launcher 只能指名已知的 CLI，且不得設定 `NODE_OPTIONS`、`PATH` 這類可在他人行程插入程式碼的環境變數。
  - `.workbench/extensions.yaml` 的 id 加上格式檢查、skill 路徑必須留在工作區內，同步不會寫到宣稱目錄之外。
- Mermaid 改用 `securityLevel: 'strict'`：`loose` 會跳過 URL 檢查，圖裡的 `javascript:` 連結會成為可點的程式執行。
- 未認證的區網請求不再能讓主行程丟出未捕捉例外（靜態路由收斂、realpath 檢查、stream 加 error handler）；同時修好 `/remote.webmanifest` 原本永遠 404 的問題。
- `<webview>` 的權限改由主行程釘死（無 preload、無 Node、contextIsolation）；`openExternal` 只放行 http(s)。

### Docs / 文件
- 新增 `docs/architecture.html`：中英雙語的架構說明（行程與傳輸、pty 扇出、完整訊息表、所有常數與上限及其成因、四條端到端流程、信任邊界、狀態位置、模組地圖、建置封裝、資安現況與已知取捨）。

---

## [0.1.23] - 2026-09-30

### Added / 新增
- **iPhone 遠端控制（區網）**（設定 → 遠端控制）：同一個 Wi-Fi 下用 iPhone 查看並操作 CLI 終端。
  - 加入主畫面的 PWA，依 Apple HIG 設計：首頁以電腦名稱為標題，「等你回覆」置頂；審批面板把 CLI 的問題、要執行的指令與每個選項原文直接做成按鈕；「閱讀」模式依手機寬度重新換行（字級跟隨 iOS 文字大小），「終端」模式原樣鏡像；特殊鍵列與輸入框、交接筆記與 Git 狀態檢視。
  - 視覺沿用桌面版的莫蘭迪兩套主題（晨霧暖調／暮靄冷調）：背景是莫蘭迪色暈，卡片、按鈕、面板都是磨砂玻璃（邊緣高光、背景模糊透色），圖示是桌面版的染色圓角方塊，按下去會回彈；App 圖示改為鼠尾草青。
  - 淺色／深色／增強對比三組配色，玻璃最透處也維持所有文字對比 ≥ 4.5:1；狀態同時用文字與圖形表示；「降低透明度」時玻璃改為實色，並尊重「減少動態效果」。
  - 可從手機開新的 Claude Code／Codex／Antigravity／Shell，桌面同步出現對應分頁並可接手。
  - 手機 App 的設定可選：外觀（跟隨系統／淺色／深色）、語言（跟隨系統／繁體中文／English），存在手機上，切換即時生效。
  - Bypass 模式可在手機上開關（與桌面「CLI 工具與 Agent → Bypass」同一個設定）：開啟前需確認、桌面會跳出通知、寫入 audit log；只影響之後啟動的 agent。
  - Web Push 通知：agent 等待審批或任務結束時推播，點通知直接開到該終端。
  - 安全：本機 CA 帶 Name Constraints（只能簽私有 IP 與 `*.local`）、一次性配對碼、裝置 token 只存雜湊、可隨時撤銷、只接受區網來源、標題列顯示連線中的手機、audit log。

### Changed / 變更
- pty 輸出改為可多方訂閱（桌面視窗與手機同時接收），並保留最後 256KB 畫面供中途接上的裝置補畫面；待審批偵測移到 main process，桌面與手機共用同一份規則（`src/shared/approvalDetect.ts`）。

---

## [0.1.22] - 2026-09-29

### Changed / 變更
- **CLI 安裝指令**（Settings → 安裝 CLI）：Windows 確認視窗改顯示官方單行指令，且實際執行的就是這一行：
  - Claude Code：`irm https://claude.ai/install.ps1 | iex`
  - Codex CLI：`irm https://chatgpt.com/codex/install.ps1 | iex`（`CODEX_NON_INTERACTIVE=1` 改由環境變數帶入）
  - Antigravity：`irm https://antigravity.google/cli/install.ps1 | iex`

---

## [0.1.21] - 2026-09-24

### Fixed / 修復
- **Dashboard Session 清單不會自動更新**：`dashboard:data` 的掃描去重在同步掃描結束後才被賦值，導致每 5 秒的輪詢永遠拿到第一次的結果，只有手動重新整理或切換工作區才會更新。已移除該去重，新的 session 約 5 秒內自動出現。
- **多開視窗時每個視窗都顯示「當前工作區」**：原本由 main 依「目前聚焦的視窗」判斷；改由各視窗依自己的工作區判斷。
- **Session 被歸到錯誤的工作區**：查不到所屬工作區時不再回填「聚焦視窗」的工作區；Antigravity 改為比對日誌內容是否包含任一已知工作區（最近開啟＋開啟中視窗）。查不到工作區且非活躍的 session 不再顯示。

### Changed / 變更
- `dashboard-cache.json` 從各工作區的 `.workbench/` 移到使用者資料夾（與 `usage-cache.json` 同處），多視窗不再互相覆寫；快取版本升為 3，升級後首次掃描會重新解析一次（較慢）。舊的 `.workbench/dashboard-cache.json` 已不再使用，可自行刪除。

---

## [0.1.20] - 2026-09-24

### Added / 新增
- **Vibe Coding 模式**（Settings → Appearance → 工作模式切換，與 Developer 模式並存）：
  - 版面：最左圖示列（Sessions／Status／Handoff／Files／Git／Settings）｜終端（對話）｜成品（App／Preview／程式碼）。
  - 圖示列游標懸停即淡入彈出、移開淡出；點擊固定，點外面／✕／Esc 關閉。
  - 終端輸出偵測到 `http://localhost:PORT` 自動在成品分頁開啟（偵測邏輯改寫自 AgentsDock，Apache-2.0）；Dev 模式則在 header 顯示可點的網址。
  - Handoff 窗格直接讀 `.project-memory/handoff.md`，整理成已完成／未完成／下一步。
  - Agent 輸出時對話欄外框呼吸光暈。
- **Token 用量區間切換**：總用量／30天／7天／今天（Dashboard 與 Vibe 終端上方狀態列共用），顯示的 Agent 與 Settings 的 CLI 啟用開關連動。

### Fixed / 修復
- **Dashboard token 數字失真**：
  - Claude：改為每次 API 呼叫依 `message.id` 去重後加總（原本輸入只取單次最大值、輸出被重複計算，實測少算約 79 倍）；拆分為 In／Cache read／Out。
  - Antigravity：移除「整個 .db 二進位當文字」的估算（同一 session 比 transcript 估算高 11 倍），統一以 transcript 估算並標示「估算」。
  - 總數改為掃全部歷史並按日累計，不再只加總「最近 N 筆 session」——修正每次打開數字都不同。
- **終端最底一行被裁掉**（視窗大小不同時才出現）：xterm FitAddon 以父層含 padding 的高度算行數，會多算一行；padding 改放在 xterm 元素上。
- **dev 版與安裝版共用 userData** 導致單例鎖衝突（`npm run dev` 直接被關）與設定互相污染；dev 版改用 `agent-workbench-dev`。
- 設定頁卡片大小統一（工作模式／語言／主題皆為單行小卡）；Dashboard 與成品工具列在窄寬度下不再外溢；終端 Launchpad 不再排出 3+1、底部停靠太矮時自動切精簡卡片；補上缺漏的 `sidebar.settings` 翻譯。

---

## [0.1.19] - 2026-09-24

### Fixed / 修復
- **可攜版更新後重開退回舊版**：`isInstalledApp()` 原本以 `isProtectedPath(exeDir)` 輔助判斷，但該函式對 exe 目錄本身恆為 true，導致可攜版被誤判為安裝版，自動更新會靜默安裝一份到 `%LOCALAPPDATA%\Programs\Agent Workbench`；使用者再從原可攜版捷徑開啟即回到舊版。現改為僅以 NSIS 解除安裝程式判定，可攜版改導向 GitHub Releases 下載頁。

---

## [0.1.18] - 2026-09-18

### Highlights & Summary / 更新亮點
Agent Workbench v0.1.18 重點重構並優化設定視窗中的「關於與更新（About & Updates）」UI 排版佈局：
1. **全面導入 Apple HIG Inset Grouped 設計體系，修復 0px 擠壓貼合問題**：
   - 解決「Check for Updates」按鈕與下方更新資訊卡片之間 0 間距、擠壓黏合的視覺問題。
   - 替換舊版自創無樣式 class，全面對齊 macOS 系統設定標準規範：外層 `.macos-section`、工具列 `.macos-section-header-bar` 與內容容器 `.macos-inset-group`。
   - 為按鈕與資訊卡片提供標準、舒適的 8px 垂直呼吸空間。
2. **About & Updates 專屬 Apple HIG 樣式建構**：
   - 建立 `.macos-about-hero` 品牌卡片，整合 App Logo、版本資訊與發行通道藥丸徽章（Installed / Dev / Portable）。
   - 建立 `.macos-update-card` 更新卡片，配置優雅的內距（`padding: 16px 18px; gap: 12px;`），使更新說明日誌滾動框與下載進度條/重啟按鈕排列清晰美觀。
   - 修復 `.macos-settings-body` 的 Flex 溢出行為（加入 `min-height: 0;`），避免彈窗內容在滾動時被底部 Footer 裁切。
3. **偏好設定與 GitHub Releases 頁面整合為標準 Inset 清單行**：
   - 將「啟動時自動檢查更新」開關與「前往 GitHub Releases 頁面」整合至同一組 Inset Group 之中，消除原本孤立懸浮在底部的外連按鈕。

---

## [0.1.17] - 2026-09-18

### Highlights & Summary / 更新亮點
Agent Workbench v0.1.17 深度優化設定視窗（Settings Modal）的擴充功能、儲存按鈕佈局與文檔工具驗證狀態記憶機制：
1. **Extensions & Skills 移除 Switch，改為點擊各 Agent 符號縮圖作為獨立開關**：
   - 取消右側單一的 Switch 按鈕，改為直接點擊各 Agent（Claude 太陽、Antigravity 行星、Codex 徽章）縮圖切換。
   - 每個 Agent 的開關完全獨立，切換 Claude 絕不影響 Antigravity 或 Codex；Claude 原生 `enabledPlugins` 與後端狀態完全隔離同步。
   - 擴展 Antigravity 技能掃描，支援外掛子目錄技能、全域原生技能與工作區專用技能獨立枚舉。
2. **開關符號縮圖固定寬度，整齊三欄垂直對齊**：
   - 嚴格鎖定每顆縮圖寬度為 `104px`、狀態徽章（`ON`/`OFF`）固定 `27px`，未安裝狀態補上等寬佔位符。
   - 徹底解決 `ON` 與 `OFF` 字寬差異及未安裝項目的寬度抖動問題，使所有項目整齊劃一垂直對齊成三直欄。
3. **Appearance 與 Extensions 底部儲存按鈕排版標準化**：
   - 修復彈窗內容主體 `height: 100%` 導致視窗內容溢出擠壓 Footer 的問題，改為 Flex 容器自適應滾動。
   - 鎖定 Footer 高度為 48px，規範 Save 與 Cancel 按鈕尺寸與行高，避免折行破壞佈局。
4. **Document Tools 測試通過狀態持久化記憶**：
   - 修復開啟視窗時因初始狀態未定義誤判為「Verification failed」的錯誤。
   - 在 `settings.json` 新增 `docToolTestResults` 記憶體，測試通過後永久記憶，下次開啟立即顯示 Verified。
5. **CLI & Agents 安裝狀態動態標籤**：
   - 當系統已自動探測到或已設定有效路徑時，按鈕自動切換為「Reinstall / 重新安裝」，消除安裝就緒時的混淆。

---

## [0.1.16] - 2026-09-18

### Highlights & Summary / 更新亮點
Agent Workbench v0.1.16 全面校準並升級各 AI Agent CLI 的官方原生 PowerShell 下載安裝指令與路徑自動探測機制：
1. **Claude Code 採用官方原生 PowerShell 一鍵安裝與自動更新規範**：
   - 全面替換原先易受全域環境與權限阻礙且已被官方 Deprecated 的 `npm` 安裝方式，對齊 Anthropic 官方推薦之原生指令：`irm https://claude.ai/install.ps1 | iex`。
   - 原生安裝檔自動下載獨立二進位檔並配置到 `%USERPROFILE%\.local\bin\claude.exe`。
   - 具備雙層安全容錯：若網路特殊阻擋，背景自動無縫 fallback 至 npm 備用安裝方案。
2. **OpenAI Codex CLI 採用非交談式官方 PowerShell 一鍵安裝**：
   - 對齊 OpenAI 官方規範：`powershell.exe -ExecutionPolicy Bypass -Command "$env:CODEX_NON_INTERACTIVE='1'; irm https://chatgpt.com/codex/install.ps1 | iex"`。
   - 注入 `$env:CODEX_NON_INTERACTIVE='1'`，徹底解決官方腳本在背景執行時因等待終端使用者互動確認（`Prompt-YesNo`）而卡死超時的問題。
   - 自動安裝至 `%LOCALAPPDATA%\Programs\OpenAI\Codex\bin\codex.exe` 或 `.codex\packages\standalone\current\bin\codex.exe`。
3. **路徑探測與環境變數未重啟即時命中**：
   - 擴充 `findCli` 搜尋候選清單：納入 `.local\bin\claude.exe`、`claude.ps1`、`OpenAI\Codex\bin\codex.exe`、`codex.ps1` 等。
   - 即使使用者電腦在安裝完成後尚未重啟終端或重新登入，Agent Workbench 依然能即刻自動偵測並帶入正確二進位路徑，即時完成測試驗證。

### Changed / 變更
- **CLI 安裝指令及確認彈窗更新 (`src/main/ipc/ext.ts`)**:
  - `getAgentInstallInfo()` 與 `runInstallAgent()` 全面切換至 Anthropic 及 OpenAI 官方原生 PowerShell 安裝命令。
- **CLI 路徑搜尋器擴充 (`src/main/ext/paths.ts`)**:
  - 納入官方原生安裝目錄，提升 Windows 與 Unix 跨平台自動偵測精準度。
- **設定視窗描述更新 (`src/renderer/src/components/SettingsModal.tsx`)**:
  - 更新 Codex CLI 說明文字為官方執行器規範。

---

## [0.1.15] - 2026-09-18

### Highlights & Summary / 更新亮點
Agent Workbench v0.1.15 強化工作區啟動機制與安裝升級體驗，實現預設不主動載入目錄（僅依使用者手動載入記錄復原），並新增更新與安裝完成後自動啟動應用程式：
1. **預設不主動載入目錄，僅復原使用者主動開啟之專案**：
   - **移除強制 cwd / Documents 兜底**：全面改寫 `determineInitialWorkspace()`，移除原先無先前記錄時盲目載入目前執行目錄或 Documents 之行為。首次啟動或無使用者主動開啟之專案時，直接進入乾淨的空白首頁。
   - **排除已自 Dashboard 移除之專案**：在啟動與還原工作區時，主動比對 `deletedWorkspaces`，確保已手動移除的專案不會在下次啟動時被強制載入。
   - **優化空工作區首頁與面板體驗**：
     - 空工作區狀態下視窗標題保持精簡 `Agent Workbench`，側邊欄預設停留在 `Dashboard` 儀表板，提供專案與會話總覽入口。
     - 檔案樹（`FileTreePanel`）在尚未開啟工作區時呈現 Apple 質感的 Empty State（資料夾圖示、說明引導與「開啟資料夾」按鈕）。
     - 底層保護：`files:list` 在目錄為空時安全回傳空陣列 `[]`，終端 PTY 在無工作區時安全回退至使用者家目錄，防止行程崩潰或越界警告。
2. **安裝與更新完成後自動啟動 Agent Workbench**：
   - **NSIS 安裝設定升級 (`electron-builder.yml`)**：加入 `runAfterFinish: true`，確保無論是手動執行安裝程式或背景自動更新升級完成，皆會自動啟動應用程式。
   - **快速安裝腳本升級 (`install.ps1`)**：在安裝完成後自動探測 `Agent Workbench.exe` 路徑並立即調用 `Start-Process` 啟動應用程式，免去手動尋找開始功能表之繁瑣操作。
   - **應用程式內部無縫升級重啟 (`updater.ts`)**：配合 NSIS 自動完成新版本重啟。

### Added / 新增功能
- **檔案樹無工作區空狀態介面 (`FileTreePanel.tsx`)**:
  - 新增無開啟資料夾時的引導畫面與「開啟資料夾」按鈕。
- **安裝完成自動執行設定 (`electron-builder.yml` & `install.ps1`)**:
  - NSIS 安裝精靈預設勾選「執行 Agent Workbench」，安裝或升級完畢後自動啟動。
  - `install.ps1` 靜默/一般安裝成功後自動啟動應用程式。

### Changed & Fixed / 變更與修復
- **工作區初始化邏輯重構 (`index.ts`)**:
  - 移除預設強制以 cwd 或 Documents 作為工作區之行為，回歸由使用者主動決定。
  - 啟動時自動過濾已被標記移除的專案。
- **空工作區保護機制 (`files.ts` & `pty.ts`)**:
  - `files:list` 支援安全空目錄回傳。
  - 終端 PTY 初始化時防禦空路徑，回退至使用者家目錄。

---

## [0.1.14] - 2026-09-18

### Highlights & Summary / 更新亮點
Agent Workbench v0.1.14 帶來 Antigravity CLI 外部會話深度追蹤、重塑 Apple HIG 確認彈窗、修復「從 Dashboard 移除」後重載復原機制，並支援 Git Clone 遠端儲存庫直接載入：
1. **外部 Antigravity CLI 會話即時活躍偵測與狀態同步**：
   - 解決外部終端或 IDE 外執行的 Antigravity CLI 運作時未顯示 Active 的問題。新增即時探測 SQLite WAL（`conversations/*.db-wal`）、背景子任務日誌（`tasks/*.log`）、即時訊息目錄與 `transcript_full.jsonl`，不再因 `transcript.jsonl` 延遲更新而落入 idle。
   - 統一度量衡：Active 判定時間窗口調整為 5 分鐘，Idle 調整為 30 分鐘。
   - 雙向去重與資料合併：以 session ID 合併 CLI 即時活躍時間與 UI 豐富 Token 統計，自動放行非工作區根目錄之外部 CLI 會話。
2. **重塑「從 Dashboard 移除」確認彈窗（Apple HIG 規範）**：
   - **徹底更換誤導性垃圾桶圖示**：改採 Apple `folder.badge.minus` 規範的 `IconFolderMinus` 專案資料夾減號圖示，並防呆設定非破壞性彈窗預設不顯垃圾桶。
   - **Apple HIG Accessory View 結構重構**：彈窗擴充為 420px，消除右側按鈕兩行折行問題；新增專案預覽卡（Target Card）與安全免責呼籲盒（Safe Callout），以綠色護盾圖示明確說明「本機硬碟檔案完全不受影響，重新開啟隨時恢復」。
3. **修復隱藏專案後重新載入無法重新出現之問題**：
   - 移除 Vite 打包後失效之動態 `require('./dashboard')`，改以解耦事件回呼。
   - 強化 `unmarkDeletedOrArchivedWorkspace`：在重新開啟、切換或克隆專案時，同步將專案路徑與旗下所有會話從 `deletedWorkspaces` 及 `deletedIds` 抹除釋放，專案與會話卡片即時重現。
4. **Git Clone 遠端儲存庫與開啟專案**：
   - 支援直接透過彈窗貼入 Git URL，克隆至本機並自動在新視窗或當前工作區載入。
5. **工作區安全與目錄過濾**：
   - 修正切換工作區時目錄探測越界拋出 `files:list Access denied` 警告。

### Added / 新增功能
- **Antigravity CLI 全面即時探測 (`dashboard.ts`)**:
  - 新增 `getAntigravitySessionMaxMtime()`，跨 `.db-wal`、`messages`、`tasks` 多點獲取即時活躍時間。
- **全新 Apple HIG 風格確認彈窗 (`AppleAlertDialog.tsx` & `appleAlertDialog.css`)**:
  - 新增 `IconFolderMinus` 與 `IconShieldCheck`。
  - 新增專案目標預覽卡（`.apple-alert-target-card`）與安全護盾呼籲盒（`.apple-alert-safe-callout`）。
  - 對話框加寬至 420px，按鈕設定 `white-space: nowrap; min-height: 36px`。
- **Git Clone IPC 處理器 (`git.ts`)**:
  - 新增 `git:clone` IPC 呼叫，支援背景克隆儲存庫。

### Fixed / 修復問題
- **移除專案後重新開啟無法重現修復 (`dashboard.ts` & `settings.ts`)**:
  - 清理 `deletedIds` 避免關聯會話持續遭到過濾。
  - 修正 Vite 打包環境動態 require 失敗問題。
- **檔案樹工作區切換路徑過濾 (`FileTreePanel.tsx`)**:
  - 過濾不屬於當前 targetRoot 之舊展開目錄路徑，消除越界日誌報錯。
- **JumpList 排除已被封存或移除之工作區 (`jumplist.ts`)**:
  - 任務列 JumpList 即時過濾已封存與已刪除的專案。

---

## [0.1.6] - 2026-09-15

### Highlights & Summary / 更新亮點
Agent Workbench v0.1.6 帶來 Dashboard 會話卡片原生拖曳排序與跨資料夾歸類（Issue #7，方案 A），以及精準的活躍會話偵測與專案路徑反解（Issue #8）：
1. **Dashboard 會話卡片原生拖曳重排與跨資料夾移動 (Issue #7)**：卡片支援在群組內依游標中線上下插入重排，亦可直接拖曳至各資料夾容器進行跨專案歸類。排序與分類變更支援 `localStorage` 本地持久化，放回原生目錄時自動清理覆寫。右側終端面版拖曳 Handoff 與點選繼續執行完整保留。
2. **活躍 Agent 會話偵測與專案目錄路徑反解 (Issue #8)**：
   - 擴充 Antigravity 系統提示緩衝區至 512KB + 尾部 64KB，比對當前工作區路徑，徹底解決路徑被巨大 prompt 裁切而歸類至 "Antigravity Workspace" 的問題。
   - 修復 Claude 專案路徑盲目將連字號替換為空格的 Bug，保留如 `IDE-remade-3` 等真實目錄結構，並結合子檔案 mtime 穿透 Windows 目錄快取限制。
   - 補齊 Claude 與 Codex 掃描器的 `< 2m` 活躍狀態計算。
   - 新增 SQLite 二進位反解，自動提取 Antigravity CLI 會話之真實專案路徑與工作區名稱。
3. **終端已開啟會話防重複啟動**：點擊已在右側終端運行的會話卡片時直接切換並聚焦既有終端 Tab，不再反覆生成重複進程。

### Added / 新增功能
- **Dashboard 原生拖曳排序與放置 (`DashboardPanel.tsx` & `dashboard.css`)**:
  - HTML5 原生拖曳資料傳輸 (`application/x-dashboard-session-id`)。
  - 卡片上下邊界發光指示條（`.drag-over-top` / `.drag-over-bottom`）。
  - 資料夾群組容器放置高亮（`.dash-folder-group.drag-over-folder`）。
  - `localStorage` 持久化儲存（`dashboard-order` 與 `dashboard-folder-overrides`）。
- **Antigravity CLI 資料庫專案反解 (`dashboard.ts`)**:
  - `extractWorkspaceFromBlob()` 精準解析 `~/.gemini/antigravity-cli/conversations/` 下 SQLite blob 之工作區目錄與路徑。

### Fixed / 修復問題
- **會話目錄歸屬與狀態識別修復 (`dashboard.ts`)**:
  - 修正 Antigravity 巨大系統提示截斷導致路徑遺失問題。
  - 修正 Claude 專案目錄連字號破壞與 Windows 目錄 mtime 滯後問題。
  - 補齊 Claude / Codex 活躍（active）狀態判斷邏輯。
  - PTY 優先度 3 同工作區匹配加入 30 分鐘時間窗口，避免好幾天前歷史會話誤標活躍。
- **終端會話聚焦重複開啟修復 (`TerminalPanel.tsx`)**:
  - 比對 `associatedSessionId`，點選既有會話卡片時直接 focus 既有終端 tab。

---

## [0.1.5] - 2026-09-14

### Highlights & Summary / 更新亮點
Agent Workbench v0.1.5 針對安裝與發布體驗進行關鍵優化：
1. **即時下載進度條與狀態反饋 (`install.ps1`)**：替換原先靜默無感的下載機制，優先採用 `curl.exe` 動態即時進度條與百分比顯示；並於靜默安裝 (`-Silent`) 期間提供動態 Spinner 與秒數計時，徹底消除安裝卡頓與凍結假象。
2. **安裝指令簡便模式**：支援 `$env:INSTALL_SILENT=1` 與 `$env:INSTALL_DOWNLOAD_ONLY=1`，提供更簡潔的單行 PowerShell 安裝方式。
3. **發布權限與安全標註 (`README.md` & `release.ps1`)**：明確標註 `npm run release` 為專案維護者專用（Maintainers only），強化開源協作之安全界線說明。

### Added / 新增功能
- **PowerShell 安裝腳本即時進度條 (`install.ps1`)**:
  - `Download-FileWithProgress` 優先調用 Windows 內建 `curl.exe -fL --progress-bar`，提供動態即時進度條與百分比。
  - 自動容錯降級為 `.NET HttpWebRequest` 串流下載與 `Write-Progress` 頂部進度條。
  - 在 `-Silent` 靜默安裝模式下加入即時動態 Spinner 與已耗時秒數顯示。
  - 支援 `$env:INSTALL_SILENT=1` 與 `$env:INSTALL_DOWNLOAD_ONLY=1` 單行環境變數快捷開關。

### Changed / 變更與調整
- **README 與 Release 腳本標註維護者權限 (`README.md` & `scripts/release.ps1`)**:
  - 中英文文檔清楚標示 `npm run release` 僅限專案維護者（需要本機 `gh` 倉庫寫入權限），並說明外部貢獻者無法擅自發布或更改專案資產。

---

## [0.1.4] - 2026-09-14

### Highlights & Summary / 更新亮點
Agent Workbench v0.1.4 帶來完整的自動化發布工作流、線上自動更新支援、真實 Codex/Antigravity 會話恢復與極窄防跑版佈局：
1. **真實 Codex & Antigravity 會話恢復 (Real Session Resume - PR #4)**：修正點擊儀表板會話卡片時未能真實帶入 session resume 參數的問題，串接 Antigravity CLI 原生會話儲存庫與 Token 啟發式估算，並為 PTY 增加 WebContents 銷毀保護避免崩潰。
2. **零停機線上自動更新與容錯 (Auto-Updater & Error Sanitization)**：支援桌面安裝版在線檢查更新與背景下載，消除 404 報錯；免安裝綠色版亦可一鍵獲取最新發布。
3. **極窄邊欄響應式防重疊佈局 (Responsive Dashboard Sidebar)**：採用 CSS Container Query，側邊欄極度縮小或拖曳時按鈕自動動態轉為圖示，消除文字折行與多層卡片擠壓。
4. **內建 Web 測試瀏覽器離線智慧引導 (Browser Dev Server Offline Guidance)**：本地 Web 開發伺服器未啟動時提供清楚引導卡片與常用 Port 快捷切換。
5. **一鍵式自動發布與安裝 (Automated Release Publisher & Quick Installer)**：引入 `npm run release` 一鍵自動編譯、壓縮免安裝綠色包並直推 GitHub Releases；提供 PowerShell 單行快速安裝腳本。

### Added / 新增功能
- **一鍵式 Release 發布腳本 (`scripts/release.ps1` & `npm run release`)**:
  - 自動讀取 `package.json` 版本號，執行 TypeScript 檢查與 electron-builder 打包。
  - 自動壓縮綠色免安裝目錄 `release/win-unpacked` 為 `Agent-Workbench-<version>-portable.zip`。
  - 自動調用 GitHub CLI (`gh`) 上傳安裝檔、免安裝包、`latest.yml` 與區塊校驗檔。
- **PowerShell 一鍵安裝腳本 (`install.ps1`)**:
  - 支援 TLS 1.2/1.3，自動抓取最新 Release 安裝檔並於使用者端一鍵下載安裝。
- **內建 Web 測試瀏覽器未啟動引導 (`TestBrowserPanel.tsx`)**:
  - 監聽 `did-fail-load`，伺服器離線時展示友善引導與常用 Port（:5173, :3000, :8080, :8000）按鈕。
- **Auto-Updater 404 報錯過濾與備援檢查 (`updater.ts`)**:
  - 消除無效的 404 錯誤堆疊傾印，在線版本即時確認無誤後提供打勾確認狀態。

### Fixed / 修復問題
- **真實 Codex & Antigravity 會話恢復 (PR #4 by Jerrywu-TT)**:
  - 補齊 `TerminalPanel.tsx` 中 Codex resume 指令參數（`args = ['resume', req.id]`）。
  - 對接 `scanAntigravityCliConversations` 至 CLI 原生資料庫目錄，避免 `--conversation` 參數被剔除。
  - 加入 `estimateTokensFromBlob` 提供 Antigravity 會話 Token 統計。
  - PTY 進程加入 `isDestroyed()` 守護，防止視窗關閉時觸發 WebContents 崩潰。
- **儀表板側邊欄縮小重疊跑版 (`DashboardPanel.tsx` & `dashboard.css`)**:
  - 資料夾標題重構為雙層結構，徹底解決切換按鈕文字折疊與標籤重疊。
  - 導入 CSS 容器查詢（`@container dash-panel`），窄版自動將按鈕收縮為圖示。

---

## [0.1.3] - 2026-09-14

### Highlights & Summary / 更新亮點
Agent Workbench v0.1.3 為需要高度自動化工作流的開發者引入 AI Agent 啟動權限略過模式（Bypass Mode）：
1. **一鍵全自動無人值守 (Bypass Permissions Mode)**：在設定面板提供全域開關，啟動 Claude Code、OpenAI Codex 或 Google Antigravity 時自動注入官方免審批與跳過權限參數。
2. **終端與卡片即時狀態可視化**：終端面板與 Launchpad 卡片即時呈現醒目的橘色 Bypass 懸浮徽章，並針對排版進行強化修復，確保無截斷、不跑版。
3. **安全預設與防重複注入**：預設維持關閉以保障安全性，且 PTY spawn / launchers 實作嚴謹的參數重複檢測，相容多種作業系統環境與啟動封裝。

### Added / 新增功能
- **CLI 啟動權限與略過模式 (CLI Launch Permissions & Bypass Mode)**:
  - 在「設定 ➔ CLI 與 AI 代理」新增專屬控制區塊，配備 Apple HIG 盾牌圖示、開關切換器與警示橫幅。
  - 支援全域設定持久化（`.workbench/settings.json`，欄位 `cliBypassPermissions`，預設為 `false` 確保安全）。
  - 啟用時自動向各 AI Agent 注入官方略過審批參數（具備防重複注入機制）：
    - **Claude Code**: `claude --permission-mode bypassPermissions`
    - **OpenAI Codex**: `codex --dangerously-bypass-approvals-and-sandbox`
    - **Google Antigravity**: `agy --dangerously-skip-permissions`
  - 終端啟動板（Launchpad cards）與「`+`」選單即時呈現橘色 `Bypass` 標籤，採用卡片右上角懸浮標籤佈局，徹底解決文字截斷跑位問題。

---

## [0.1.2] - 2026-09-14

### Highlights & Summary / 更新亮點
Agent Workbench v0.1.2 專注於極速開機體驗與精準的儀表板互動連動：
1. **42 倍極速冷啟動效能優化 (42x Fast Startup Optimization)**：實作檔案 mtime 磁碟持久化快取與單面板按需掛載（Mount-on-Demand），全盤掃描時間由 281ms 驟降至 6.6ms。
2. **Dashboard 與 Settings CLI 動態連動 (CLI Enable/Disable Linkage)**：設定中啟用或停用特定 Agent CLI（Claude, Antigravity, Codex）時，Dashboard 遙測卡片、會話清單與資料夾群組即時同步過濾；若全域停用則提供友善導引與一鍵直達設定按鈕。
3. **主行程資源保護與雙重掛載消除 (Resource Guard & Mount Cleanup)**：後端自動跳過已停用 Agent 的磁碟 I/O，消除多重請求衝突；移除開發環境雙重掛載，系統冷開機不再額外啟動不必要的 Git 背景進程與渲染負擔。

### Added / 新增功能
- **Dashboard CLI 啟用/停用動態連動 (Dashboard CLI Linkage)**:
  - 儀表板遙測卡片網格（`dash-agents-grid`）僅動態渲染目前已啟用的 Agent。
  - 頂部統計指標（總 Tokens、活躍進程、總會話數）動態計算已啟用 Agent 之數據。
  - 會話資料夾群組（`folderGroups`）與會話卡片全面過濾已停用 Agent 之歷史紀錄。
  - 新增全停用空狀態導引橫幅（`.dash-no-agents-banner`），附帶直達「設定 ➔ CLI 設定」快捷按鈕。
  - 後端 `src/main/ipc/dashboard.ts` 智能跳過已停用 Agent 之日誌掃描與 PTY 進程關聯，省下磁碟 I/O 與 CPU 負擔。
- **檔案 mtime 磁碟持久化快取 (mtime Disk Cache)**:
  - 引入 `.workbench/dashboard-cache.json` 本機快取機制，利用檔案最後修改時間比對未變更會話（單檔快取命中耗時 < 0.05ms）。
  - 引入 `activeScanPromise` 互斥鎖，避免定時輪詢與多重請求重複觸發檔案讀取。
  - 工作區切換時自動宣告記憶體快取失效並安全重整。

### Fixed & Improved / 修復與改進
- **面板按需掛載與狀態保持 (Mount-on-Demand with Keep-Alive)**:
  - 側邊欄（Files, Git）與中央區（Preview, Memory）改採 `visitedTabs` 按需掛載，冷啟動時不再生成多個 Git 子行程，亦不預載重量級 Mermaid 庫；訪問過後持續保留於 DOM，確保切換分頁狀態不丟失。
- **消除開發模式雙重掛載 (React StrictMode Cleanup)**:
  - 移除 `<React.StrictMode>`，消除開發時兩次重複觸發全盤掃描與 effect 負擔。
- **快取檔案納入版控忽略 (.gitignore)**:
  - 將 `.workbench/dashboard-cache.json` 加入 `.gitignore`，確保各機器本機暫存檔不污染版控。

---

## [0.1.1] - 2026-09-14

### Highlights & Summary / 更新亮點
Agent Workbench v0.1.1 帶來了重大功能升級與穩定性強化，主要包含：完整整合 Codex 擴充套件與真實會話 Token 遙測掃描（外部 PR #2 整合）、全新 Session 資料夾分組與工作區一鍵快速切換、全介面中英文雙語系支援（Strict English & Traditional Chinese）、會話活躍狀態與時間戳精準判定，以及終端啟動板滾動顯示修復。

### Added / 新增功能
- **Codex 擴充掃描與真實會話遙測 (Integration with PR #2)**:
  - 支援自動掃描 `~/.codex/config.toml`，提取 Codex MCP 伺服器與外掛設定。
  - 支援自動掃描 `~/.codex/skills/` 讀取自訂 Skill 定義與 metadata。
  - 遞迴解析 `~/.codex/sessions/**/rollout-*.jsonl` 與 `~/.codex/session_index.jsonl`，計算真實累計 Token 數、會話標題與工作目錄。
  - 智慧 PTY 行程匹配（三段優先級：cwd 匹配、啟動時間鄰近度判斷），精準標記 Codex 活躍終端會話。
  - 標準化 Token 三重分類法：`Context & System Prompt`、`Cached Input Context`、`Thinking & Generation`。
- **Session 資料夾分組與互動式工作區切換 (Session Folder Grouping & Workspace Switching)**:
  - Dashboard 會話列表支援以工作目錄（Folder）手風琴樣式分類展示，支援一鍵全部展開／摺疊。
  - Session 卡片新增資料夾標籤按鈕，點擊立即切換後端工作區根目錄 (`files:setWorkspaceRoot`)，自動切換至 Files 檔案側邊欄並即時重新整理檔案樹。
  - 當側邊欄處於收合狀態時，點擊切換工作區將自動展開側邊欄。
  - 檔案樹系統引進 `fs.watch` 即時監聽與視窗事件廣播，本機檔案異動零延遲同步。
- **全域中英文雙語系國際化 (Bilingual i18n Support)**:
  - 支援嚴謹的英文（Strict English）與繁體中文（Traditional Chinese）全介面切換。
  - 覆蓋全部核心面板：Dashboard、Editor、Git、Terminal、Preview、Settings、Customized 及快顯提示。
  - 設定選單支援即時切換語系並持久化儲存。

### Fixed & Improved / 修復與改進
- **遙測準確性與幽靈會話消除 (Telemetry Precision & Ghost Session Cleanup)**:
  - 修正 Active 與 Completed 狀態判定錯誤，精確綁定實體 PTY 行程存續狀態。
  - 修復 Antigravity 會話時間戳為空白導致的排序與過濾異常（改以檔案最後修改時間 `mtime` 作為可靠判定基準）。
  - 過濾無效、零 Token 與空白之幽靈會話（Ghost Sessions）。
- **終端啟動板滾動修復 (Terminal Launchpad Scroll Fix)**:
  - 修復終端開啟過多會話時，頂部選項卡被截斷且無法向上滾動的 Flexbox CSS 缺陷 (`min-height: 0` 容器捲動修正)。
- **Apple HIG 微互動 (Apple HIG Micro-interactions)**:
  - 為工作區跳轉按鈕與分類標頭導入 Apple HIG 風格的按壓縮放 (0.96 scale) 與懸浮微動畫。

---

## [0.1.0] - 2026-09-10

### Initial Release / 初始版本
- **Monaco Editor Core**: VS Code 同款 Monaco 編輯器核心，支援程式碼編輯、多分頁開啟與即時未存檔保護。
- **Multi-CLI Terminals**: 透過 `@lydell/node-pty` 與 `xterm.js` 實現的原生終端殼層，支援 Claude Code、Antigravity CLI 及各類 Shell 同時執行。
- **Git Hub & Visualizer**: 整合 Git 狀態檢視、暫存 (Stage)、提交 (Commit)、分支切換、GitGraphView 提交歷史拓撲圖與 Monaco DiffEditor 歷史比對。
- **Shared Project Memory**: 整合 `.project-memory/` 標準規範，跨 AI 代理人記憶讀取與交接機制。
- **Document & Markdown Preview**: 內建 Markdown/HTML 即時預覽，以及 Office (Word, Excel, PowerPoint) / PDF 文件檢視器。
- **Native Packaging**: 支援 Electron-Builder 跨平台打包 (Windows NSIS 安裝包)。
