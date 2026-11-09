# ShareProjectMem（繁體中文）

跨工具共享專案記憶。讓 **Claude Code / Codex / Antigravity** 在換電腦、換 agent 時讀同一份
記憶，不必重新解釋專案或重掃程式碼。

記憶**絕不提交進專案 repo**：每個 clone 各自在本機，跨電腦共用交給
[Agent Workbench 記憶同步](#存放位置agent-workbench-記憶同步)（私人 GitHub repo，或你指定的 OneDrive／Google Drive 資料夾）。

*[English version →](README.md)*

📐 [架構總覽（互動式流程圖）](docs/architecture.html) ·
📄 [完整設計、協定與測試紀錄](docs/implement.md)

> **狀態**：機制層已在 Windows／Git Bash 與 Linux／Docker 兩個環境完整實測通過。
> 尚未驗證的一項：Codex 與 Antigravity 是否確實**照著**它們載入的指示做
> （無 headless CLI 可自動化，需 1 分鐘手測 —— 見 [`docs/implement.md`](docs/implement.md) §12）。

---

## 為什麼需要它

你在多台電腦、多個 agent 之間切換。每次切換，agent 都：

- 不知道上次做到哪、下一步是什麼
- 重新掃描整個 repo 來「理解專案」
- 重做已經完成的工作，或推翻已經做過的決定

這套系統把「下一個 agent 做正確下一步所需的最少可靠資訊」放在專案旁邊，讓每個 agent 開局自動拿到。

**成本**：約 1,000 tokens／session。**省下**：每次 10,000–50,000 tokens 的重複探索。

---

## 設計核心：單一事實來源，兩條自動化路徑

`.project-memory/handoff.md` 是**唯一**被寫入的狀態。三個 agent 開局都自動拿到它，但機制不同 ——
因為只有 Claude Code 能「自動執行指令並注入輸出」：

```
              .project-memory/handoff.md   ← 唯一事實來源
                          │
        ┌─────────────────┴──────────────────┐
        │                                     │
   status.sh                          sync-entry-files.sh
   (執行時印出 tier-1)                 (生成進靜態 entry 檔)
        │                                     │
   Claude Code                    Codex          Antigravity
   SessionStart hook          AGENTS.md      .agents/rules/
   自動執行 → 注入              自動載入        自動載入
                                    ▲                ▲
                                    └── 自動再生 ───────────┘
                              commit-handoff.sh（每次交接）與 Agent Workbench（開啟專案時）
```

entry 檔裡的 `AUTO-MEMORY` 區塊是**機器衍生**的，標有 do-not-edit，不會 drift。

---

## 各工具支援程度

| 功能 | Claude Code | Codex | Antigravity | 機制 |
|---|:--:|:--:|:--:|---|
| **開局載入記憶** | ✅ 全自動 | ✅ 全自動 | ✅ 全自動 | plugin 的 SessionStart hook（即時）／生成區塊（快照） |
| 安裝 | `/memory-init` | `install.sh` | `install.sh` | 兩條路都通 |
| 收工寫交接棒 | `/handoff` | `$handoff` | `/handoff` | plugin 的 skill；沒裝 plugin 時照 entry 檔指示 |
| **擋記憶進專案 repo** | ✅ | ✅ | ✅ | `.git/info/exclude` + git pre-commit（工具無關） |
| **擋 secret 進記憶 repo** | ✅ | ✅ | ✅ | `commit-handoff.sh` secret 掃描 |
| 跨機同步 | ✅ | ✅ | ✅ | Agent Workbench 記憶同步 + `commit-handoff.sh` |

> **誠實的地板**：hook 擋得了「沒更新記憶」，擋不了「更新得敷衍」——
> 內容品質仍取決於 agent，這是 LLM 的本質限制，沒有工具能繞過。

---

## 安裝

### 搭配 Agent Workbench（建議）

Agent Workbench 內附這個 plugin（原始碼在 Workbench repo 的 `plugins/shared-project-memory`）。
開啟設定時，Workbench 會詢問是否幫每個已啟用的 Claude Code／Codex（從本機 marketplace
`agent-workbench`）與 Antigravity（`agy plugin install`）安裝，並取代從 GitHub 裝的版本。之後 plugin
跟著 Workbench 一起更新。

沒裝 plugin 時，Claude Code 仍會拿到記憶快照：`sync-entry-files.sh` 會寫入
`.claude/rules/shared-memory.md`（只在這個 clone），裝上 plugin 後就移除，handoff 不會被載入兩次。

### Claude Code（不透過 Agent Workbench）

```bash
git clone https://github.com/zkylek1212-k/Multi-Harness-ADE.git
claude plugin marketplace add ./Multi-Harness-ADE/plugins/shared-project-memory
claude plugin install shared-project-memory@shared-project-memory
```

然後在任一專案裡：

```
/memory-init
```

### Codex / Antigravity / 純手動（不需要 Claude Code）

```bash
git clone https://github.com/zkylek1212-k/Multi-Harness-ADE.git
bash Multi-Harness-ADE/plugins/shared-project-memory/install.sh            # 裝進目前 repo
bash Multi-Harness-ADE/plugins/shared-project-memory/install.sh /my/repo   # 裝進指定 repo
```

Windows PowerShell：

```powershell
.\install.ps1
```

兩條路徑跑的是同一支 `scripts/init-memory.sh`，結果完全相同，且**冪等 —— 不覆蓋任何既有檔案**。

**前置條件**：git、bash（Windows 用 Git Bash）、目標資料夾已 `git init`。

### Codex plugin

把 clone 裡的 plugin 資料夾加成本機 marketplace 再安裝：

```bash
codex plugin marketplace add ./Multi-Harness-ADE/plugins/shared-project-memory
codex plugin add shared-project-memory@shared-project-memory
```

接著在要初始化的專案中明確呼叫 `$shared-project-memory`。plugin 不會自動初始化專案，也不提供 MCP server；skill 會先檢查 Git repo 和既有記憶檔，再呼叫現有的專案初始化器。

### Antigravity plugin

```bash
agy plugin install ./Multi-Harness-ADE/plugins/shared-project-memory
```

Antigravity 會執行根目錄的 `hooks.json`（SessionStart → `node scripts/session-start.js --antigravity`，
把 `status.sh` 的輸出注入 session），並提供 `handoff` 與 `shared-project-memory` 兩個 skill。

---

## 安裝後產生什麼

```
.project-memory/
├── INDEX.md              # 純索引，無可變狀態（tier 1）
├── handoff.md            # 交接棒，唯一的狀態寫入處（tier 1）
├── STATE.md              # 宏觀里程碑（tier 2，按需）
├── DECISIONS.md          # 追加式決策 + 頂部索引表（tier 2，按需）
├── PROTOCOL.md           # 完整協定（首次/按需）
├── status.sh             # 開局腳本（任何 agent 或人都能跑）
├── sync-entry-files.sh   # 把 handoff 生成進 Codex/Antigravity 的 entry 檔
├── commit-handoff.sh     # 依記憶同步設定存放 handoff（絕不進專案 repo）
└── archive/
AGENTS.md                          # Codex entry（含 AUTO-MEMORY 區塊）           — 僅本機
.agents/rules/shared-memory.md     # Antigravity entry（同上）                    — 僅本機
CLAUDE.md                          # 附加區塊（只有指示、沒有記憶內容；不覆蓋既有內容）
.githooks/pre-commit               # 有人強制 add 記憶時擋下
.gitattributes                     # 強制 hook 為 LF
```

`.project-memory/`、`AGENTS.md`、`.agents/rules/shared-memory.md` 會寫進 `.git/info/exclude`
（只影響這個 clone，不產生 commit）。可以進專案 repo 的只有 `CLAUDE.md`、`.githooks/`、`.gitattributes`，要不要提交由你決定。

---

## 每天怎麼用

**開工** — 三個 agent 都直接下任務即可，記憶已在 context 裡。
想要含未提交狀態的最即時版本，任何 agent 都可跑 `bash .project-memory/status.sh`。

**收工** — Claude Code 用 `/handoff`；其他 agent 依 `AGENTS.md` 的完成程序。
Agent 會改寫 handoff（`State`、`Verified`、`Not verified`、`Next`、`Risks / do not` 五段，最多
8000 bytes），再用 `commit-handoff.sh` 存放。不符更新門檻（唯讀提問、瑣碎解釋、無變更 review）時
**完全不寫檔** —— 省下無謂的 churn。

**換電腦** — 在已設定記憶同步的 Agent Workbench 開啟專案：它會把 `.project-memory/` 連到共用的那份並重產 entry 檔。
若專案有提交 `.githooks/`，每個 clone 再跑一次 `git config core.hooksPath .githooks`。

---

## 記憶分層（省 context 但不失真）

| 層 | 檔案 | 何時載入 |
|---|---|---|
| Tier 1 | `INDEX.md`、`handoff.md` | **每次**，自動 |
| Tier 2 | `STATE.md`、`DECISIONS.md`、`PROTOCOL.md` | **按需** |
| 封存 | `archive/`、`SESSION_LOG.md` | 永不自動載入 |

完整保真度**永遠留在磁碟** —— 省 token 靠「不 eager load」，**不是砍內容**。
`INDEX.md` 不放任何可變狀態（commit／日期／進度），避免與 `handoff.md` 雙寫不一致。

| 情境 | tokens／session |
|---|--:|
| 唯讀提問 | ~740 |
| 一般開發 | ~1,100 |
| 架構工作 | ~1,620 |
| **加權平均** | **~1,000** |

---

## 存放位置：Agent Workbench 記憶同步

記憶放哪裡，在每台電腦的 **Agent Workbench：設定 → 記憶同步** 選一次。plugin 的
`commit-handoff.sh` 照這個設定存放，絕不提交進專案 repo。

| 設定 | `.project-memory/` 是 | `commit-handoff.sh` |
|---|---|---|
| 關閉（預設） | clone 裡的一般資料夾 | 只存本機，不連網路 |
| 你指定的 OneDrive／Google Drive 資料夾 | 指向該資料夾的連結 | 存好即可，由雲端硬碟程式上傳 |
| 私人 GitHub repo | 指向私人記憶 repo clone 的連結 | 在那裡 commit 並 push（`MEM_AUTOPUSH=0` 不 push） |

一個記憶 repo／資料夾放所有專案，每個專案一個子資料夾。Workbench 開啟專案時自動連結；另一台電腦則連到已經存在的那份。

**沒有 Agent Workbench**：記憶只留在寫入它的那台電腦。**沒有這個 plugin**：Workbench 照樣存放與同步記憶，
但 Claude Code 開局不會載入、也沒有 `/handoff`；Workbench 的記憶同步設定頁會提示安裝 plugin。

`MEM_AUTOSYNC=1` 讓 `status.sh` 開局時 fast-forward 私人記憶 repo（僅限工作樹乾淨且無分岔；其他情況只回報，永不 merge 或 reset）。

---

## 已驗證行為

以拋棄式 repo 實測，Windows／Git Bash 與 Linux／Docker（Alpine + busybox）皆通過：

- scaffold 冪等（第二次執行全部 skip，不覆蓋）
- 安裝後 `git status` 不會列出 `.project-memory/`、`AGENTS.md`、`.agents/`（`.git/info/exclude`）
- `git add -f .project-memory/handoff.md` → **被 `pre-commit` 擋下**；團隊自己的 `AGENTS.md`（沒有 AUTO-MEMORY 區塊）→ 通過
- `commit-handoff.sh`：記憶同步關閉 → 只存本機、不 commit；雲端資料夾 → 存好；私人 repo → 在那裡 commit 並 push，專案 repo 不動
- 記憶新增 `api_key: <長值>` → **`commit-handoff.sh` 拒絕提交**到記憶 repo
- 記憶中出現 `Never store passwords, API keys` 這類**純文字** → 不誤判
- Claude hook 路徑與直接執行 `status.sh` → **輸出完全一致**
- 在沒有 `.project-memory/` 的 repo 執行 hook → **靜默 no-op，exit 0**
- 安裝後 entry 檔**立即**含 AUTO-MEMORY 區塊
- 存放 handoff → 兩個 entry 檔**自動再生**
- **從 clone 出來的 repo** 安裝並跑完整套測試 → 全數通過，換行為 LF

---

## 已知限制

- **hook 擋不了「寫得爛」** —— 只能擋「沒寫」。內容品質仍靠 agent。
- **Codex／Antigravity 的服從性未經真機驗證** —— 已確認它們**讀取**哪些檔案
  （由 Antigravity 安裝檔實證：`.agents/rules/*.md`、`AGENTS.md`、`GEMINI.md`），
  但「是否照做」需手測。
- **`handoff.md` 衝突**：兩台同時交接仍可能撞（私人 repo：push 失敗會 rebase 重試，真衝突則回報；雲端資料夾：雲端程式保留兩份）。
- **Codex／Antigravity 拿到的是上次交接或開啟專案時的快照**，進行中的狀態要跑 `status.sh` 才看得到。
- **Antigravity `.agents/rules/` 路徑可能隨版本變** —— 升級後用 grep 重驗（30 秒，方法見文件）。

---

## 文件

| 文件 | 內容 |
|---|---|
| [`docs/architecture.html`](docs/architecture.html) | 互動式架構總覽：資料流、處理管線、分層元件圖、部署 |
| [`docs/implement.md`](docs/implement.md) | 完整設計理由、協定全文、開發歷程、17 項已解決問題、11 項待解決 |

---

## License

[MIT](LICENSE)
