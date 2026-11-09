# 兩台電腦的工作流程

在兩台 PC 上輪流開發同一個專案時，讓兩邊（以及 Claude Code、Codex、Antigravity 三家 AI）都知道「做到哪、下一步是什麼」。

兩條同步管道：
- **程式碼**：專案 repo（git push / pull）。
- **專案記憶**（`.project-memory/`，含 handoff）：Workbench 的「記憶同步」，三選一：
  - **GitHub 私人 repo**（預設 `ai-project-memory`）：有版本紀錄，Workbench 會自動拉取、每 10 分鐘背景同步。
  - **OneDrive** 或 **Google Drive** 裡你選的一個資料夾：由雲端硬碟程式同步，Workbench 不做任何網路動作。

  所有專案共用同一個位置，每個專案一個資料夾；專案 repo 不追蹤記憶檔，記憶不會出現在公開的專案 repo。
  **沒開啟記憶同步時**，交接只存在本機，Workbench 和 AI 都不會做任何同步或連網動作。

Workbench 本身的設定、儀表板紀錄、Cowork 會議紀錄存在各台電腦本機，**不會**同步。

---

## 一、每台電腦的一次性設定

1. **安裝工具**
   ```powershell
   winget install --id Git.Git -e
   winget install --id OpenJS.NodeJS.LTS -e
   ```
2. **取得專案**
   ```bash
   git clone https://github.com/zkylek1212-k/Multi-Harness-ADE.git
   cd Multi-Harness-ADE
   npm install
   git config core.hooksPath .githooks
   ```
   `core.hooksPath` 每個 clone 都要設一次：`git pull` 之後會自動更新 Codex／Antigravity 讀的入口檔。
3. **記憶同步**（Workbench：設定 → 記憶同步）
   1. 打開「開啟記憶同步」開關（先看清楚頁面上的說明）。
   2. 選「記憶放哪裡」：GitHub（私人 repo）、OneDrive 或 Google Drive。兩台電腦要選同一個。
   3. 依選項完成設定：
      - **GitHub**：按「安裝 GitHub CLI」「登入 GitHub」（指令貼進新的終端分頁，按 Enter，需要時在瀏覽器登入），再按「建立或連結私人 repo」。第一台會建立私人 repo，其他電腦連到同一個；公開的 repo 會被拒絕。
      - **OneDrive／Google Drive**：按「選擇資料夾」，在雲端硬碟裡選（或新增）一個資料夾，例如 `OneDrive\ai-project-memory`。只接受雲端硬碟裡的子資料夾，記憶只會存在那裡。另一台電腦選同一個資料夾。
   4. 之後打開任何有 `.project-memory` 的專案，都會**自動接上**（「自動接上專案」預設開啟）：
      - 第一台：本機記憶先備份，再搬進共用位置。專案 repo 不會被修改：只寫這個 clone 的 `.git/info/exclude`，不產生提交。
      - repo 仍在追蹤記憶檔，或本機記憶和共用位置的不同（例如同一個 repo 的第二個 clone）：不自動接上，Workbench 會通知你到「這個專案」確認；追蹤中的記憶檔要按按鈕才會產生「停止追蹤」的提交（不會自動推送）。
      - 其他電腦：直接連到共用位置裡的記憶，入口檔（`AGENTS.md`、`.agents/rules/shared-memory.md`）自動補回。
      - 專案裡的 `commit-handoff.sh`、`status.sh` 會換成 Workbench 附的版本（依設定決定存本機、存資料夾或推到私人 repo）。
      也可以在設定頁「這個專案」手動操作。
4. **記憶 plugin**（Claude Code／Codex／Antigravity）：Workbench 內附 shared-project-memory，開啟設定時會詢問是否幫已啟用的 Claude Code、Codex、Antigravity 安裝（從 GitHub 裝過舊版的會一併換成內附版）。
   裝了之後，每次開新對話都會即時讀到專案記憶，並有 `/handoff`。沒裝時，Claude Code 讀 `.claude/rules/shared-memory.md`、Codex 讀 `AGENTS.md` 裡的記憶快照。
   外掛會在開新對話時執行**該專案自己的** `.project-memory/status.sh`，只在信任的專案使用。
5. **Workbench 本機設定**（每台各自做）：設定 → CLI（安裝或指定各家 CLI）、設定 → Cowork、Customized 面板的連線憑證。

## 二、每次開工

1. 專案程式碼：`git pull`
2. 打開 Workbench 和專案。GitHub 模式會在開啟專案時自動拉取（「自動同步」開著時）；OneDrive／Google Drive 模式只要雲端硬碟同步完成就是最新的。
3. 三家 AI 啟動時都會讀到最新的 handoff。想手動看：`bash .project-memory/status.sh`（會顯示記憶 repo 是否落後）。

## 三、每次收工（換電腦前一定要做）

1. 請 AI **更新 handoff**（`.project-memory/PROTOCOL.md` 的固定欄位：State / Verified / Not verified / Next / Risks / Details）。
2. **提交記憶**：
   ```bash
   bash .project-memory/commit-handoff.sh "docs(memory): <一句話摘要>"
   ```
   依記憶同步設定處理：未開啟＝只存在這台電腦（不連網路，AI 不會再重試）；雲端資料夾＝存好即可，由雲端硬碟上傳；GitHub＝提交並推送到私人 repo（新增內容若含密碼、API key、私鑰會被擋下）。任何情況都不會寫進專案 repo。
3. **提交並推送程式碼**：`git push`
4. 另一台開工時從第二節開始。

> 若有安裝使用量保護 hook（`~/.claude/hooks/usage-guard.cjs`），Claude 的用量剩不到 3% 時會自動停下來做第 1、2 步。

## 四、工具（MCP、skill、外掛）怎麼兩台一致

- 在 Workbench 的 **Customized** 面板宣告要用的 MCP／skill／外掛，會寫進 `.workbench/extensions.yaml`，跟著專案 repo 同步。
- 另一台拉下來後，在同一個專案的 Customized 面板套用。憑證不寫在這個檔案裡（用 `${conn:名稱}`），每台在 Connections 各自輸入。

## 五、不會同步的東西

| 項目 | 存放位置 | 說明 |
|---|---|---|
| CLI 路徑、Cowork 預設、最近開過的專案、記憶同步設定 | `%APPDATA%\agent-workbench\` | 每台各自設定 |
| 連線憑證 | 本機，作業系統加密 | 刻意不同步 |
| 儀表板的會話紀錄與 token 用量 | 各台的 `~/.claude`、`~/.codex`、`~/.gemini` | 只看得到本機的紀錄 |
| Cowork 會議紀錄、背景執行的 worktree | `%APPDATA%\agent-workbench\cowork` | 在哪台開的會就在哪台 |
| 記憶搬移前的本機備份 | 記憶存放位置裡的 `.local-backups/` | GitHub 模式不會推送；資料夾模式會隨雲端硬碟同步 |
| 版面配置 | Workbench 的本機儲存 | 每台各自調整 |

## 六、注意事項

- **不要兩台同時改同一份記憶**。兩台都改了同一行時，「立即同步」會回報衝突，你的改動保留在本機的提交裡；到記憶 repo 副本資料夾（設定頁的「開啟資料夾」）解決後再同步。
- 程式碼也一樣：先推、另一台再拉。
- 私人記憶 repo **不要改成公開**。Workbench 每次建立或連結時都會檢查。
- OneDrive／Google Drive 模式沒有衝突處理：兩台同時改同一個檔案時，雲端硬碟會另存一份「衝突複本」，需要手動合併。
- 要換存放方式（例如 GitHub 改 OneDrive），先關閉記憶同步再重新設定；已接上的專案要重新接一次。
- 改到 `package.json` 或 lockfile 後，另一台拉下來記得再跑一次 `npm install`。
