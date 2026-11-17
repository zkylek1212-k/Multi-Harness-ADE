# Cowork.md — 多 Agent 協作模式設計計畫

> **已由 Cowork 2.0 取代（2026-11-13，DEC-013）。** 本文件描述的會議／規劃／背景執行協議已移除，不再維護，只留作歷史紀錄。現行設計是畫布模式：每個 Agent 是一個互動式終端節點，各自一個 worktree，見 `src/shared/canvas.ts` 與 `core/crates/wb-core/src/canvas.rs`。

- 日期：2026-10-07（Asia/Taipei）
- 狀態：**P1 已實作**；背景執行（取代原 P2 的 PTY 派送，含同時執行）見 §12.5
- 沿革：Claude Opus 5 原稿 → Codex 覆核修訂 → Claude Opus 5.5 合併（各版差異見附錄 B）
- 原稿基準：`master` @ `b6c7841`（v0.1.38）
- 驗證範圍：讀過原始碼、CLI help 與官方文件；**未執行模型任務，未實作 Cowork**

> **閱讀前先看（2026-10-24 整理）：本文件混合了原始設計與後來的實作，以下章節已被取代或與程式碼不一致。** 以程式碼與 `docs/cowork-architecture.md` 為準。
>
> | 章節 | 現況 |
> |---|---|
> | §4.1「不讓 agent 自由討論」 | 只適用 Project 規劃。之後新增的 Discussion 模式讓參與者依序看到前面的公開發言（DEC-008）。 |
> | §2.3「發現越界時先顯示 diff」 | 2026-10-24 起實作：每輪結束後比對改動與 scope，越界的檔不 commit，任務停在 blocked，由使用者還原、接受或追問（`executor.ts`）。 |
> | §2.2「不預設共用含 native module 的 `node_modules`」 | 背景執行可選擇以 junction 共用；2026-10-24 起，只要有任務的 scope 會改依賴（package.json、lockfile），整場就不共用。 |
> | §2、§5 的 P2 設計（專用 PTY 派送、`cowork.task()` MCP 回報、輪流執行） | 已被 §12.5 的 headless 背景執行取代，沒有實作 MCP 回報。 |
> | §3.1 能力表「Claude Code 本機 PATH 上找不到、待實測」 | §12.1 已有 Claude Code 2.1.292 的實測結果。 |
> | §12.5「尚未實作 `resources` 的互斥」 | 已實作（`executor.ts` 排程時檢查）。 |
> | §12.1 各 CLI 的唯讀參數與 Antigravity 隔離方案 | 是 Claude Code 2.1.292、Codex 0.160.1、Antigravity 1.2.14 的實測結果；CLI 更新後需要重測。 |
> | 任務間交接（§2.3「只傳 summary 與 commit hash」） | 2026-10-24 起：前置任務以 git 取得的改動清單，加上 agent 回報的 `cowork-result` 區塊（介面、已驗證、未驗證、備註）完整交給下游，文字摘要只當補充。 |

| 標記 | 意思 |
|---|---|
| ✅ 原始碼／help 確認 | 確認介面或選項存在，不代表端到端實測通過 |
| 📖 文件確認 | 官方文件有此能力，本機安裝與行為仍須驗證 |
| ⚠️ 待實測 | 尚未驗證的實作前提 |
| 🔶 設計決策 | 本計畫採用的選擇與代價 |

## 設計主軸

先讀這六條，後面每一節都是它們的展開：

1. **規劃走 headless 結構化輸出，執行走互動 PTY。** 規劃需要可解析的結果，執行需要看得到、插得了手。規劃因此完全不必解析終端畫面。
2. **會議是有界協議，不是自由對話。** 主席開場、其他人獨立覆核、主席一次定案。理由見 §4.1。
3. **三種狀態各用對的工具。** manifest 記協調狀態，worktree 隔開工作樹，git 記成果。git 不拿來做協調。
4. **送出不等於收到，回報不等於完成。** 不確定就停下交給使用者，不猜、不自動重送。
5. **恢復時暫停，交給人核對。** 崩潰與中斷後一律進 `paused`，由使用者核對後再續跑。自動修復的路徑最難測、最容易藏 bug，所以不做。
6. **先循序，後平行。** 派送、回報、commit 與恢復都要先在單一寫入者下證明可靠，才開放多個寫入者。

## 0. 需求與交付界線

終端區新增 **Cowork** 分頁。使用者給一則 prompt，選定主席與參與 agent，先形成可審閱的任務板，再由 agent 執行。介面是聊天室式時間軸加上常駐任務板；git 保存成果，manifest 保存協調狀態。

- **P1：多 agent 規劃驗證版。** 產出任務板，以及可以手動派送的任務文字。還沒有自動共同執行。
- **P2：可靠的循序執行。** 多個 agent **輪流**執行，同時只有一個任務在寫入。這一階段打通派送、回報、git 與恢復。
- **P3：多 agent 平行執行。** 每個 agent 有自己的 worktree，依賴排程與整合都可靠之後，才算滿足原始需求。

> ⚠️ **對原始需求的落差**：使用者最初要的是「討論分工後同時進行」。P2 的體感是輪流，不是協作；同時進行要到 P3。這是刻意的取捨：先在單一寫入者下證明可靠。

第一版支援本機、已有 commit 基線的 git repo，每個 git common directory 同時只能有一個活動 run。非 git 專案可以用 P1 的附檔規劃，P2/P3 暫不支援。跨機共同執行、通用 mesh 與自由多輪聊天都延後。

## 1. 定位：可停用的協調模式

`implement.md` 目前寫「工作台不是協調者」，Cowork 確實擴張了這個定位。**即使做成可停用的模式，這個例外仍要寫進文件，B 案不能省掉 `implement.md` 的修訂。**

**ADR-01（本計畫採用）**

- **背景**：需要跨 CLI 分工；模型迴圈、工具與原生審批仍由各家官方 CLI 負責。
- **決策**：採 B 案。Cowork 是選用的協調模式，負責任務板、派送、狀態與成果整合。run 狀態以 manifest 為準；ShareProjectMem 只接收使用者選擇保存的交接報告。
- **代價**：工作台要多負責取消、恢復與任務身分；不把 CLI 原生迴圈搬進來。
- **替代**：A 案（全面擴張成 agent runtime）沒有現成需求；C 案（手動交棒）做不到自動共同執行。

進入 P2 前，同步修訂 `implement.md` 的定位與安全邊界。

```text
Cowork 分頁 ── IPC ── main orchestrator ── manifest（唯一 run 狀態）
                          ├─ headless runner：規劃、定案
                          ├─ 既有 PTY：執行、原生審批、人工介入
                          ├─ cowork MCP：任務接收、結果回報
                          └─ git：worktree、任務 checkpoint、整合分支
```

## 2. 隔離與 Git：scope 是分工，worktree 隔開工作樹

**ADR-02（取代原稿預設的 scoped 平行寫入）**

- **背景**：不同 scope 仍可能同時改到型別、lockfile 或建置產物；共用 index 也可能混進別人 staged 的內容。按 scope commit 再補一筆 sweep，無法保證歸屬正確。
- **決策**：P2 用一個專用的 run worktree 循序執行；P3 用一個 integration worktree，再加上每個執行 agent 各一個 worktree。`scope` 只用於規劃與越界檢查，**不作為隔離或歸屬的保證**。
- **代價**：要準備依賴、佔用磁碟、處理整合衝突。P2 只用一份環境來壓低成本。
- **替代**：不提供在共用工作樹上平行寫入。環境負擔太重時改成循序執行，不退回不可靠的平行模式。

worktree 隔開的是檔案與 index；git refs 仍然共用，全域設定、憑證、網路、硬體與作業系統也都沒有隔開。agent 不自行 checkout、commit、merge、reset，也不啟動另一套多 agent 排程。git 寫入流程由 Cowork 負責，偵測到外部變動就暫停並核對。這條約定要靠 CLI 權限與原生審批一起守住。

### 2.1 開始 run 的基線

1. 記錄 repo 實際路徑、git common directory、來源 branch 與完整 `baseCommit`，並取得這個 repo 的活動 run 所有權。
2. 檢查 tracked、staged、untracked 檔案，merge/rebase 狀態，以及編輯器內未儲存的內容。規劃與執行預設都使用選定的已提交快照。
3. 原工作樹可以是 dirty 的，但 UI 要明確顯示「以 `<baseCommit>` 規劃，未提交／未儲存的內容沒有納入」。若要納入，由使用者先自行處理再選新的基線。不自動 stash、commit、丟棄或複製任何修改。
4. 另建 `cowork/<runId>` 分支與專用 worktree，原工作樹不切換分支。改了基線就要重跑規劃，舊的核准跟著失效。
5. integration 分支與各 agent 分支使用不同名稱；名稱衝突就停止，不覆寫。worktree 放在本機 userData 的 run 目錄裡，避開專案掃描與 OneDrive 同步。

### 2.2 環境與共用資源

- 安裝依賴、啟動 server 都要明確操作。P2 只有一份環境；P3 按需要安裝，不替每個 agent 預先啟動 server。
- port 可以用 Vite 自動換 port，或明確指定，「只有一個 5173」不足以否決 worktree。不預設 symlink，也不共用含 native module 的 `node_modules`。
- 任務可以宣告 `resources`，例如 `device:usb-1`、`port:5173`、`build-output:firmware`，同一個資源同時只派給一個任務。P3 在 main 用一個 Set 取得與釋放就夠了；P2 全部循序，不需要。只有確認使用者或背景行程已經停止佔用時才釋放；逾時或回報不明時不直接解鎖。
- 同 scope 或會共同修改某個介面的任務，要加上依賴或改成循序。scope 不同不代表沒有語義依賴；`detail` 必須寫清楚介面契約與驗收條件。

### 2.3 Checkpoint 與 P3 整合

- agent 回報只代表進入 `reported`。先停止對該 session 的自動派送，再核對 worktree、scope、產物、必要驗收，以及是否還有背景行程在寫入，確認後才形成 checkpoint。
- git 操作一律串行。每個工作樹在任務開始時必須乾淨，HEAD 也要符合預期。commit 保存完整 hash，訊息帶 run/task/attempt 身分。唯讀任務可以沒有 commit，不硬造空 commit。
- 發現越界時先顯示 diff，由使用者調整 scope 或接手，不自動 commit 整個目錄。歸屬不明的殘留不做無條件 sweep。
- P3 依照依賴順序，把核准的任務 commit cherry-pick 到 integration branch。遇到衝突就保留現場並標為 blocked，不自動選 ours 或 theirs。`done` 代表已整合且必要驗收已通過。
- 下游任務開始前，該 agent 的 worktree 必須乾淨、session 停在可以接新任務的邊界，再同步到包含前置成果的 integration checkpoint。**只傳 summary 與 commit hash，工作樹裡並不會有前置任務的程式碼。**
- 原始任務 commit 與 integration commit 要分開保存，因為 cherry-pick 會改變 hash。有依賴的任務不保證能單獨 revert，回滾時仍要檢查下游。

### 2.4 🔶 worktree 對使用者的可見後果

選 worktree 的代價，要明確寫進 UI 需求，不能只留在架構層：

| 後果 | 處置（P2 必做） |
|---|---|
| agent 在 `userData/.../worktrees/` 裡工作，**主面板的 Files、Git 與編輯器都看不到進行中的改動**，因為它們指向 `workspaceRoot` | Cowork 分頁要提供 run worktree 的檔案清單與 diff 檢視，每個任務可以展開看自己的 diff |
| 以 `baseCommit` 快照執行，**使用者尚未 commit 的改動不會被看到** | 開始前醒目提示，並列出被排除的 dirty 檔案（§2.1.3） |
| 每個 run 要完整安裝一次依賴；本 repo 有 `node-pty` native module 需要重編，**耗時以分鐘計** | 建立 worktree 後進入 `preparing` 階段：顯示「需要安裝依賴」並由使用者確認執行；環境沒就緒就不派送任務 |
| run 結束後 worktree 仍佔用磁碟 | 收斂時提供清理（§7 的刪除規則） |

## 3. CLI 能力與規劃權限

### 3.1 能力表

| CLI | 確認來源 | 非互動／輸出 | Schema | 權限候選與限制 |
|---|---|---|---|---|
| Codex | ✅ 本機 `codex exec --help` | `codex exec`；`--json` 輸出事件 JSONL；`-o` 保存最後一則訊息 | `--output-schema <FILE>` | `--sandbox read-only`；外部 MCP/hooks 仍須另外排除，不能只靠這個 flag |
| Antigravity | ✅ 本機 `agy --help`；✅ 唯讀方案實測（§12.1） | `--input-format stream-json --output-format stream-json -p=`，prompt 走 stdin | `--json-schema`，可給字串或檔案；結果在 `structured_output` | 隔離家目錄 + 權限規則 + 即時工具白名單，見 §12.1 |
| Claude Code | 📖 [CLI reference](https://code.claude.com/docs/en/cli-reference)、[programmatic usage](https://code.claude.com/docs/en/headless)；本機 PATH 上找不到 | `claude -p --output-format json`，附 metadata | `--json-schema`，結果在 `structured_output` | 可設定工具與 MCP 限制；`plan` 模式不是 OS sandbox，⚠️ 本機待實測 |

**原稿的錯誤已修正**：原稿說只有 agy 有 schema，Codex 用 `--output-format`，兩者都錯。三家的外層格式（envelope）與最終結果要分開處理。help 或文件裡有，不等於登入、schema、唯讀與 Windows 啟動都已驗收。

Settings 裡 enabled 只代表使用者的選擇，不代表 CLI 已安裝、已登入或符合 Cowork 需求。開始前要檢查可信路徑、版本與所需能力；不合格就顯示原因，不悄悄換成別家。第一版可以先支援驗證通過的兩家，不必等三家都完成。

能力分成「可規劃」與「可自動執行」兩級。MCP 或續派能力還沒通過，不影響 P1；但該 CLI 不能進入 P2/P3 的自動執行。

### 3.2 規劃權限

**ADR-03（本計畫採用）**

- **背景**：headless 模式照樣能改檔、執行命令與呼叫 MCP；執行前的 approval 擋不住規劃期間產生的副作用。`cwd` 也限制不了對其他目錄的存取。
- **決策**：規劃不繼承執行用的 bypass、Connections 工具憑證，也不載入有副作用的 MCP/hooks。優先使用已驗證的唯讀工具或 sandbox 設定；無法驗證時，改由工作台附上有界的程式碼快照，並禁用 agent 工具與自訂執行入口。兩者都無法約束的 CLI，不參與自動規劃。
- **代價**：附檔模式的探索能力較弱，要額外補 context。CLI 自己的登入與 cache 寫入不算專案編輯，但要在能力探測中說明。
- **替代**：只在 prompt 裡寫「不要改檔」，不足以保證唯讀。不直接拿預設的 headless 設定去跑真實 workspace。

規劃以基線快照進行，結束後比對 HEAD、index 與 tracked/untracked 變更。只要有副作用就判定這次規劃失效，並保留 diff。**事後偵測只是補強，前提仍是執行前的權限約束。** repo 裡的指令檔與其他 agent 的回答都是待審資料，不能要求 runner 放寬權限。

### 3.3 Runner 與輸出驗證

- 沿用 `findAgentCli`、全域可信 CLI 路徑與 Windows 啟動處理。`pty.ts` 已經有 `.exe/.cmd/.bat/.ps1` 的分支，但 `resolveCommand` 是私有函式；只抽出必要的啟動解析，不連同 PTY 參數或 bypass 一起搬。
- 優先直接 `spawn` 執行檔，prompt 從 stdin 傳入，stdout/stderr 以串流讀取並設上限，避開 `execFile` 預設的 buffer 限制。CLI 不吃 stdin 時，才改用已驗證過的原生 prompt 參數或檔案入口。
- `.cmd/.bat` 需要透過直譯器啟動，不能把 prompt 拼進 `/c` 的字串裡。沒有安全的傳入方式就拒絕自動啟動；長 prompt、換行、引號與含空白的路徑都要納入驗收。
- 依各家格式明確解析外層 envelope 或事件串流，取出最終結果，再做 schema 與語義驗證。**不抓第一個合法的 JSON 區塊**，那可能是工具參數或中途輸出。
- 拒絕以下內容：重複 ID、未知的 assignee 或依賴、循環依賴、不合法的 scope、超過上限的任務數、未解決的 blocker。路徑正規化後，拒絕絕對路徑、`..`、git 內部路徑，以及 symlink/junction 跨出目標工作樹的情況。
- 輸出無效時，最多做一次有界的修正呼叫，計入預算；仍然失敗就顯示原因並提供人工處理入口。先用小型的型別與欄位檢查，schema 複雜到難以維護時再考慮引入驗證套件。

## 4. 會議：主席開場、獨立覆核、一次定案

### 4.1 🔶 為什麼不讓 agent 自由討論

使用者最初的構想是「主席開場後，大家互相討論」。主席開場採用了；自由討論不採用，原因有三：

1. **成本會平方成長。** 每一輪，每個 agent 都要讀完其他人說過的全部內容。三個 agent 討論三輪約要十次呼叫，context 還一路累積。
2. **LLM 在對話中傾向附和**，尤其會附和主席的框架與最後一位發言者。結果很可能是付了三輪的費用，拿回主席原本的計畫加上一些修飾：看起來有審議，實際上沒有。這種失敗成本最高，也最難察覺。
3. **沒有終止條件。** 由主席宣布結束，討論就變成橡皮圖章；要求達成共識，又可能一直繞圈。

所以不是取消討論，而是**讓每位覆核者獨立表態，再把所有異議集中交給主席裁決**。人類的群體決策也有類似經驗：先各自判斷再攤開分歧，常比自由開會好，因為自由開會容易錨定在第一位或最大聲的發言者上。

### 4.2 協議

**ADR-04**：主席先提出框架，其他人在彼此看不到回覆的情況下覆核，最後由主席一次處理全部異議並定案。獨立覆核仍可能被 R1 的框架錨定，因此品質與成本的改善是**待驗證的假設，不是已證明的事實**。每位覆核者都拿到原始 prompt、相同的基線，以及來源與介面說明，不只讀主席的摘要。

UI 保留 R1/R2/R3/R4 四個標籤，但**R3 仲裁與 R4 定案合併成一次呼叫**。不再讓 LLM 判斷「兩項反對是否算不同意見」；任何 objection、missing 或未回答的問題都必須處置。

> **修正原稿漏洞**：原稿規定「兩人反對意見不同才進入仲裁」。預設只有兩位參與者時，R2 只有一位覆核者，這個條件永遠無法成立，**單一反對會被直接跳過**。

**R1 主席開場**：框架、草案任務、介面與資源依賴、scope、驗收條件、資料來源，以及主席自己不確定的問題（留給覆核者挑戰）。

**R2 獨立覆核**：N−1 次平行呼叫，覆核者彼此看不到；可以連框架本身一起反對。

```jsonc
{
  "agree": ["t1"],
  "objections": [{ "id": "o1", "target": "t2", "reason": "...", "alternative": "..." }],
  "missing": [{ "id": "m1", "title": "...", "why": "..." }],
  "claims": ["t1"],
  "answers": { "q1": "..." }
}
```

**R3/R4 仲裁定案**：主席收到全部有界的 R2 結果，對每一項 issue 寫明採用或拒絕及理由，無法決定的列為 blocker。claims 只是偏好，仍須符合能力、資源與依賴。輸出 `tasks`、`decisions`、`unresolved` 三部分。App 驗證每項 issue 都有處置、DAG 合法、沒有 blocker 之後，才進入 `awaiting-approval`。這項完整性檢查只能證明每項都處理了，不能證明裁決正確。

**散會條件由程式判定**：每項 issue 都有處置、DAG 合法、沒有 blocker，就散會。不靠任何人宣布。

| 做法（3 個 agent） | 呼叫次數 | context 成長 |
|---|---|---|
| 平行盲提案（原稿初版） | 4 | 持平，但要讀三遍 codebase |
| **本協議** | **N+1 = 4**（兩個 agent 時為 3） | 持平 |
| 自由對話三輪 | 約 10 | 每輪累積 |

- R2 失敗不能當成同意。保留已成功的結果，提供一次重試，或明確選擇「減少參與者後重新定案」，並更新 revision。
- 使用者改板、改 prompt，或補充會影響分工的資訊，都會讓 `planRevision` 加一；核准綁定在特定 revision 上。比較早發出的呼叫晚完成時，不能覆蓋新版本。
- 使用者改板同樣要通過 scope、assignee、DAG 與 blocker 驗證；approval 不能取代程式檢查。

## 5. 派送、回報、排程與恢復

### 5.1 Session 與可靠派送

P2 同時只有一個寫入任務，每個 agent 可以保留一個 **Cowork 專用 session**。P3 每個 agent 有自己的 worktree 與 session，同一個 agent 內循序，不同 agent 之間平行。headless 的主席與執行 session 分開；後續定案從 manifest 與 diff 重建 context，不假設能沿用同一段對話。

✅ 現況：`TerminalPanel.tsx` 的 `sendToSession` 會等 `ptyId` 出現再以 bracketed paste 寫入；`handleDispatchToAgent` 會找第一個同家且未退出的 session。兩者都**沒有接收確認**。`writePty` 成功只代表寫進了行程；4 秒沒有輸出所判定的 `busy=false`，也不能當成完成訊號。

- P1 重用任務文字與 UI，但由使用者明確選擇目標分頁、貼上並自己送出，不宣稱自動執行成功。
- P2 只新建或接回綁定這個 run 的 session，並透過驗證過的原生 initial prompt，或在確認可接受輸入的任務邊界派送。**先保存派送意圖（intent）再送出，而且只送一次**；收到 `cowork.task()` 確認當前 attempt 後，才轉為 `running`。
- 後續任務也要先確認 session 能接收新 prompt。沒有可靠續派能力的 CLI，可以在任務邊界重啟 session；安全優先於重用 context。
- 沒有收到 ACK 就標為 blocked，原因記為 `delivery-unknown`，並保留這次 attempt。**不自動重送、不另開 agent，也不退回 pending。** 使用者核對終端後可以接回；確定舊行程已停止、殘留也處理完，才建立新的 attempt。
- session 停在待審批畫面時，不注入 prompt，也不自動按 Enter。`@agent` 的補充訊息先排隊，由使用者在終端確認後才送入，避免被誤當成批准指令。

### 5.2 MCP 回報與身分

第一版只需要三個動作，`ask` 延後：

```text
cowork.task()                               接收並確認當前任務與 attempt
cowork.done(taskId, attemptId, summary)     回報成果（進入 reported，還不是 done）
cowork.blocked(taskId, attemptId, reason, kind)
```

- 🔶 **attempt 建立時就綁定當下的 `planRevision`，回報只需要帶 `attemptId`。** main 由 attempt 查回 revision。改板若影響到正在執行的任務，先讓它停在安全邊界並結束該 attempt，舊 attempt 的回報就自然失效；沒受影響的 attempt 照常有效。這與在回報中另帶 revision 提供同樣的保證，但介面更小，模型也少一個可能填錯的欄位。
- 每個連線都綁定 repo、run、agent 與 session，憑證由 main 在啟動時注入，模型不能自稱是某個任務的 assignee。payload 還要再核對 task、attempt 與目前狀態。
- 同一個 attempt 重複回報 `done`，回傳既有結果；內容不同、attempt 過期，或回報別人的任務，一律拒絕，不能讓它結束後來的任務。每個 attempt 的結果都留在 manifest 的歷史事件中，重試不覆寫舊成果。
- stdio adapter 透過只限本機、帶 session 憑證的通道回報給 main，不開放公開的 broker 或通用 mesh。憑證不寫入 manifest、prompt 或版控設定。
- ✅ `ext/adapters.ts` 目前只為 Claude（專案 `.mcp.json`）與 Antigravity（全域設定路徑）輸出 MCP 設定，**沒有 Codex 的輸出**。`AGENT_PATHS` 裡有 Codex，不代表註冊已經完成。
- 各家都要驗證有沒有 session 專屬的設定或啟動入口；不為了每次 run 去改全域 MCP 設定。只有全域設定、無法依 run 區隔的 CLI，先保留人工派送。

### 5.3 完成、失敗與復會

```text
pending → dispatching → running → reported → done
                  ↘ blocked ←──────┘
活動狀態 → failed／cancelled（保留成果，但不等於可以自動重派）
```

- 依賴全部 `done`、資源可以取得、agent 閒置，任務才算 runnable。P2 也要檢查 DAG 與 scope，基本安全檢查不延到 P3。
- `done` 必須滿足預定的驗收條件；要求測試、build 或 review 的任務，要保存實際結果。agent 的自述、沒有輸出、git 有修改、exit 0，任何一項單獨都不能證明完成。要求寫檔卻沒有 diff 的，要說明原因。
- 失敗任務的下游一律 blocked，互不相干的任務可以繼續。行程退出、逾時或斷線時，不把任務重設為 pending；保留修改與錯誤，先核對再接回或重派。
- 局部卡住（缺資訊、需要決策）由使用者回覆。計畫失效（發現缺任務、依賴假設不成立、需要重切範圍）時，由主席產出 board patch，經驗證、revision 加一並重新核准。受影響的 running 任務先停在安全邊界，不在執行中途改它的 scope 或 assignee。
- 已完成的歷史不改寫，修正一律以新增任務或依賴的方式進行。未來的 `ask` 先做成行內問題加有界回覆，不往忙碌的 PTY 注入別人的指令。

### 5.4 暫停、取消、崩潰恢復

- **暫停**：停止新的派送，執行中的任務可以走到邊界；UI 要明示它仍在執行。需要立即停止就用中止，但不承諾瞬間沒有任何副作用。
- **中止／取消**：先保存意圖、撤銷回報憑證，再終止受管理的行程與其子行程；worktree、commit 與未提交的 diff 都保留。確認停止之後才能重派。可參考現有 Windows 的 `hardKill`；headless 與其他平台另外實測。
- App 關閉、擁有者視窗關閉或 workspace 切換，都走生命週期處理。非同步操作固定使用 run root，不退回到目前聚焦視窗的 workspace。
- 🔶 **重啟後，所有非終態的 run 一律進入 `paused`，等使用者核對後才續跑。** 不假設舊的 ptyId/PID 仍然有效，**也絕不只憑 PID 去終止行程**（PID 可能已被系統重用給其他程式）。
- 派送或建立 checkpoint 前，先把意圖寫入磁碟。若 commit 已完成但 manifest 尚未更新，以 run/task/attempt 標記搜尋候選 commit，**只列出供使用者確認，就算只有一個候選也不自動採用**，也不重新 commit。
- 不承諾所有副作用都 exactly-once。結果不確定就停止自動推進，不建立完整的交易引擎。

## 6. 收斂與介面

### 6.1 收斂

1. 確認沒有任何活動中的寫入者、所有任務都已進入終態，列出未提交或越界的成果，歸屬不明的殘留交給使用者處理，不自動 sweep。
2. 在 integration branch 上，對照原始 prompt 與驗收條件做最後檢查。額外的 agent review 是選配，但指定的 build、測試與整體驗收一定要完成。
3. 產出 report，內容包括：基線、revision、各項異議的裁決、task/attempt、原始與整合後的 commit、驗收結果、未解問題，以及實際或估算的用量。
4. `done` 不代表已經 merge；failed 或 cancelled 的部分成果仍可審閱。merge 或 squash 之前，要重新檢查目標 branch 與 dirty 狀態；有變動就重新核對，不自動 merge 或 reset。

report 預設寫在本機 run 目錄的 `report.md`。只有使用者選擇匯出或寫入既有的 `handoff.md` 時才修改 repo；沒有 `.project-memory/` 就只提供匯出，不自動 scaffold。review 發現問題時，可以新增明確的修正任務，或只保留在 report 中，不無限延長 run。

### 6.2 分頁、時間軸與輸入

主欄是時間軸，旁邊是常駐、可收合的任務板；Cowork 分頁與執行用的 session 在終端區是兄弟分頁。可以重用 `AgentMark`、attach、diff 與 modal 元件，但 **Cowork 分頁的型別、狀態與 worktree diff 檢視（§2.4）都要新做，不能說成零新版面**。

| 階段 | 控制 |
|---|---|
| meeting | 使用者的補充會進到下一個 revision；已送出的舊呼叫收不到 |
| awaiting-approval | 編輯任務板或給回饋；通過驗證才能核准當前 revision |
| preparing | 顯示依賴安裝的需求與進度；確認執行，或取消 |
| executing | `@agent` 只能路由到本 run 的 session，對方忙碌或待審批時先排隊；純文字視為註記 |
| paused／blocked | 顯示原因、終端、殘留 diff，並提供接回、中止、修改任務板的入口 |
| converging／done | report 與 diff、匯出、merge/squash、清理 |

時間軸顯示 R2 卡片、裁決、任務狀態，以及不明或逾時事件。終端內容不鏡像到時間軸；卡住的任務可以展開最後 10 行，並標示為診斷用快照。

**R2 一定要渲染成結構化卡片**（同意、反對及替代方案、補充、認領），不要把 JSON 或散文塞進對話泡泡。強制結構化輸出的價值就在這裡：一眼就能看出「codex 反對 t2：理由…（替代方案…）」。卡片要**並排**呈現，理由見 §6.3。

**讓協議本身看得見。** 聊天室外觀容易讓人以為 agent 會一直聊下去，所以時間軸要明確標出協議進度：

```
—— 第二輪 · 獨立覆核（1/1 已回）——
—— 定案 · 3 項異議已處置，0 項待定 ——
—— 已核准 revision 2 · 準備環境 ——
—— t2 已回報 · 等待驗收 ——
```

這樣使用者看到的是「協議走到哪一步」，而不是「怎麼突然沒人說話」。

任務板上顯示暫停／停止、revision、核准狀態與剩餘預算。錯誤與待審批狀態都要有文字標籤、鍵盤操作與焦點回復，不能只靠顏色區分。

### 6.3 🔶 會議室呈現層

在 §6.2 的骨架上加一層薄薄的呈現，讓使用起來像一場會議。全部是前端工作，屬於 P1 範圍。

```
┌ Cowork · r7f3a ──────────────────────────────────┬ 任務板 ───────────┐
│ 與會者                                            │ rev 2 · 待核准     │
│ ★ Claude  主席   ✓ 已發言                         │ t1 終端分頁  codex │
│ ● Codex   覆核   ⋯ 思考中 0:42 / 3:00  [取消]     │ t2 MCP 介面 claude │
│ 👤 你                                              │ t3 測試      codex │
├────────────────────────────────────────────────────┤                   │
│ —— 第一輪 · 主席開場 ——                            │                   │
│ ★ Claude  框架…  草案任務 3 項  待答問題 2 個 ▸     │                   │
│                                                    │                   │
│ —— 第二輪 · 獨立覆核（彼此看不到）——                │                   │
│ ┌ Codex ────────────┐ ┌ Antigravity ─────┐         │                   │
│ │ 同意 t1 t3         │ │ 同意 t1           │         │                   │
│ │ 反對 t2 → 替代…    │ │ 補充：缺測試任務  │         │                   │
│ │ 認領 t1            │ │ 認領 t3           │         │                   │
│ └────────────────────┘ └───────────────────┘         │                   │
│ —— 定案 · 2 項異議已處置 ——                        │                   │
│ ┏ 輪到你 ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┓           │                   │
│ ┃ 核准 revision 2？ [核准] [改板] [回饋…] ┃           │                   │
│ ┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┛           │                   │
└────────────────────────────────────────────────────┴───────────────────┘
```

> ⚠️ **R2 不能畫成一則接一則的對話泡泡。** 覆核者彼此看不到對方的回覆；如果上下排列，使用者會以為 Codex 是在回應 Antigravity。聊天室外觀在這裡會誤導使用者，所以 R2 一律並排，並在分隔線標明「彼此看不到」。

**要做的：**

1. **與會者列**：顯示每位與會者的角色（主席／覆核／執行）、目前狀態（思考中／已發言／執行中／等你），以及已用時間與預算。使用者本人也列在其中。
2. **進行中指示**：一次 headless 呼叫最長可能跑 180 秒，過程完全沒有輸出。沒有「Codex 思考中 0:42 / 3:00 [取消]」，使用者會以為程式當掉了。這是最必要的一項。後端只需要 runner 送出開始、經過時間、結束三種事件；預算記帳（§8）本來就需要這些資料。
3. **「輪到你」卡片**：需要使用者核准或回答卡住的問題時，以醒目的卡片標出，並把焦點移過去，不要淹沒在時間軸裡。
4. **R2 並排呈現**：理由如上。參與者超過兩位時，改成可橫向捲動或分欄，不退回上下排列。
5. **訊息與任務板互相連動**：點擊反對卡片，任務板上對應的任務會亮起；反過來點擊任務，時間軸會跳到相關的發言與裁決。

**不做的：**

| 項目 | 理由 |
|---|---|
| 擬真會議室（圓桌、座位、頭像環繞） | 沒有增加任何資訊，反而加深「它們會一直聊下去」的誤解（§11） |
| 打字動畫或假串流 | 最終輸出是 JSON，逐字吐出來沒有意義；進度由第 2 項表達 |
| 自由聊天輸入框 | 協議裡沒有自由對話（§4.1），輸入一律依階段變形（§6.2 表格） |
| agent 之間互相 @ 的對話泡泡 | agent 之間並沒有直接對話，畫出來就是在演戲 |

## 7. 本機資料與持久化

**ADR-05（改變儲存位置，仍用 JSON）**

- **背景**：一次 run 只有幾十個任務，JSON 就夠用。但 repo 可能放在 OneDrive 裡，同步 runtime 資料或 worktree 會造成跨機衝突與路徑失效。
- **決策**：存放在 `userData/cowork/<repoId>/<runId>/`，`repoId` 由本機正規化後的 git common directory 算出。只有 main 會寫入，並用一個 per-repo 的活動 run Map 管控多個視窗。dev 版與打包版的 userData 不同，single-instance lock 無法保證兩者互斥；P2 另外在兩者共用的本機 appData 目錄，以原子 `mkdir` 取得 repo lock，取得失敗就不啟動第二個執行中的 run。
- **代價**：狀態不會自動跨機同步，report 可以匯出。外部 CLI 與其他機器不受這個 lock 約束，操作前後仍要核對邊界。
- **替代**：不引入資料庫，也不做多機租約。原稿的 `.workbench/cowork/` 改為選用的匯出位置；匯出 runtime 資料時要排除在版控之外。

> 🔶 跨 dev／打包版的 repo lock 保留在 P2：本專案的使用者就是 app 開發者，dev 版與打包版會同時開啟，這是實際會發生的情境。實作成本只有一次原子 `mkdir`。

```jsonc
{
  "schemaVersion": 1, "id": "r7f3a", "revision": 12,
  "planRevision": 2, "approvedPlanRevision": 2,
  "prompt": "使用者原始需求", "createdAt": 1791400000000,
  "repo": { "root": "...", "commonDir": "...", "sourceBranch": "master", "baseCommit": "<full hash>" },
  "chair": "codex", "participants": ["codex", "antigravity"],
  "executionMode": "sequential",  // P3 可為 parallel；兩者都用專用 worktree
  "branch": "cowork/r7f3a",
  "phase": "executing", // meeting|awaiting-approval|preparing|executing|paused|converging|done|failed|cancelled
  "decisions": [], "unresolved": [],
  "budget": { "planningCallsUsed": 3, "planningMsUsed": 42000, "executionMsUsed": 0 },
  "tasks": [{
    "id": "t1", "title": "...", "detail": "...",
    "scope": ["src/main/example.ts"], "resources": [],
    "dependsOn": [], "assignee": "codex", "acceptance": ["..."],
    "status": "running", // pending|dispatching|running|reported|blocked|done|failed|cancelled
    "attemptId": "a1", "planRevision": 2,   // attempt 建立時綁定的 revision
    "worktree": "...", "expectedHead": "<full hash>",
    "sessionId": null, "ptyId": null,       // CLI session 與 runtime PTY 分開記錄
    "dispatch": { "id": "d1", "state": "acknowledged" },
    "commit": null, "integrationCommit": null,
    "summary": null, "checks": [], "outOfScopeFiles": [], "error": null
  }],
  "log": [] // 有序事件與意圖記錄；不從 log 另外推導一套 run 狀態
}
```

```text
userData/cowork/<repoId>/<runId>/
  run.json          run.prev.json
  meeting/          report.md
  worktrees/integration/    # P2 也用它當執行工作樹
  worktrees/<agent>/        # P3 才需要
```

- 寫入一律串行：先寫同目錄的暫存檔，flush/fsync 後保留上一份有效的 manifest，再 rename 取代。讀取主檔與備份時都驗證 schema 與版本；Windows 遇到檔案被佔用時做有界重試，仍失敗就停止新的派送。原子替換可以防止寫出半截檔案，但不保證斷電時絕不遺失最後一次更新。
- repo lock 記錄擁有者的隨機 ID、PID 與啟動時間，正常結束時只釋放自己的 lock。崩潰留下的 lock 由使用者核對行程與 run 之後再接管，不單憑 PID 或檔案時間自動刪除。這是本機鎖，不是跨機租約。
- 恢復時仍要核對 git 與行程狀態；從備份還原時可能少了最後幾筆事件，不能據此重做副作用。遇到未知的 `schemaVersion` 只允許讀取或匯出，不默默轉換後續跑。
- `meeting/` 依 revision 與 attempt 保存有界的模型輸出與診斷資訊，不覆寫舊輪次。不保存完整 env、Connections 憑證或 session token；CLI 輸出也可能含敏感內容，匯出前要讓使用者預覽。
- MCP 不直接寫入 manifest，一律經由 main 驗證。清理前先檢查行程、dirty 的 worktree 與尚未保存的成果；刪除的路徑在解析後必須位於該 run 目錄內，不能遞迴刪到 repo 或外部連結指向的位置。

## 8. Settings 與成本界線

主席不以沒有定義的「最強」來排序挑選：

```ts
type CoworkSettings = {
  chair: AgentId | null,            // 第一次明確選擇，之後記住
  participants: AgentId[],          // 預設 2 位合格且 enabled 的 agent，包含主席
  chairExecutes: boolean,           // 預設 true；規劃與執行的 context 分開
  limits: {
    maxPlanningCalls: number,       // 預設 6，規劃、修正、改板、復會合計
    maxPlanningMinutes: number,     // 預設 20（agy 一次覆核實測 7 分多鐘）
    maxExecutionMinutes: number     // 預設 60
  }
}
```

- 主席必須是參與者之一；只允許三家 agent，不含 shell。預設是主席加一位覆核者；合格的 agent 不足兩位時，顯示缺少什麼。
- 移除 `auto` 與 `all-enabled`，開始前可以修改名單。主席不下場執行時，至少要有另一位執行者，不能產生沒有人能執行的任務。
- 🔶 `chairExecutes` 預設為 `true`。主席同時執行任務有兩個風險：它有動機把有趣的任務分給自己，而執行也會污染它的 context，降低後續仲裁的品質。但參與者只有兩位時，主席不下場就完全沒有平行度。參與者達三位以上時，建議關閉。
- 第一次執行，以及會影響 scope、assignee 或依賴的改板，都必須核准；第一版不提供關閉 approval 的選項。Cowork 不會自動回覆 CLI 的原生審批。
- 權限、可信路徑、略過審批與提高預算，只接受本機全域偏好或使用者的明確操作；repo 設定與模型都不能放寬這些限制，沿用 `settings.ts` 既有的信任分級。
- 首版預設值：每次呼叫 180 秒、stdout/stderr 各 1 MiB、任務上限 20 個、輸出修正一次、復會兩次，實測後再調整。**預算在呼叫啟動前就先記帳，失敗的呼叫也算；總呼叫上限優先於重試額度。**
- 預算涵蓋整個 run，revision、復會與重啟都不會歸零；達到上限就停止新的呼叫。時間用完時，中止受管理的行程並保留結果，追加額度後才能續跑，已花掉的成本無法收回。
- 時間按各階段實際有行程在執行的時鐘時間累計，平行呼叫不重複計算同一段時間；使用者改板或完全暫停時不計時。CLI 停在原生審批時仍算執行時間，UI 要明示剩餘額度。記帳結果與限制的快照存在 manifest；續跑時不會因為套用新預設值而悄悄放寬限制。
- ✅ Dashboard 上部分 token 數字是估算值，Antigravity CLI 也可能沒有 token 資料。優先採用能對應到特定呼叫或 session 的原生用量，其餘標示為估算或未知；不把未知當成 0，也不拿 Dashboard 的總數冒充這次 run 的費用。
- 呼叫次數與時間是 app 能夠約束的界線，**不是精確的金額或 token 上限**。CLI 有原生的回合或預算限制時，經探測確認後才採用；PTY 沒有可驗證的上限時要明示，不承諾固定花費。

## 9. 已收斂的決策與待驗證前提

| 項目 | 處置／尚待確認 |
|---|---|
| A/B/C 定位 | 採 B 案；進入 P2 前修訂 `implement.md`，寫明協調例外 |
| 自由多輪討論 | 不採用；改為獨立覆核加一次定案（§4.1） |
| 結構化輸出 | 三家都有 help 或文件依據；外層格式、唯讀與 Windows 啟動留給 P0 探測 |
| R2 彼此不可見 | 保留；仍要防主席錨定，所以覆核者可以推翻框架、核對來源 |
| scoped 平行寫入 | 不採用；P2 用單一 run worktree 循序，P3 每個 agent 各一個 worktree |
| worktree 的可見性 | Cowork 分頁自帶 worktree diff 檢視，並有 `preparing` 階段（§2.4） |
| 恢復策略 | 重啟一律 `paused`，由使用者核對；候選 commit 只列出，不自動採用 |
| report 存放位置 | 本機 run 目錄的 `report.md`，可匯出或寫入既有的 `handoff.md` |
| agent 不回報 done | 不推估機率；MCP 前提實測通過前，沒有回報就保持不明或 blocked，不猜測已完成 |
| P1 成功標準 | 兩位合格 agent 對同一基線完成開場、覆核與定案；無副作用、任務板合法、每項異議都有處置，可以改板與手動派送 |
| 自動執行前提 | session MCP、接收確認、邊界續派或重啟、中止與結果驗證全部通過；任一項未通過就只支援手動 |

## 10. 落地順序與驗收

| 階段 | 最小實作 | 完成條件 |
|---|---|---|
| **P0 能力探測** | 先挑兩家，探測路徑與版本、JSON/schema、唯讀、登入錯誤、session MCP，以及安全的 prompt 傳入方式 | 在一次性的測試 repo 中證明規劃沒有副作用、輸出可以解析；高風險前提沒過，就不進入自動執行 |
| **P1 規劃驗證版** | runner（含開始／經過時間／結束事件）、原子化 manifest、revision、R1/R2 加上合併的 R3/R4、時間軸與任務板、會議室呈現層（§6.3）、limits、改板與手動貼上 | 達成 §9 的 P1 標準；解析失敗、單家失敗、預算用完或取消時，都不能假裝完成。P1 仍不是完整的 Cowork |
| **P2 循序閉環** | 單一 worktree 加上 `preparing`、worktree diff 檢視、專用 PTY、三個 MCP 動作、ACK、基本 DAG、驗收與 checkpoint、暫停／中止／恢復、report、跨版本 repo lock | 狀態不明時不重送；舊回報不能結束新任務；dirty 的原工作樹內容不會混入成果；崩潰後可以核對再續跑。**P2 是輪流執行，不是協作** |
| **P3 平行共同執行** | 每個 agent 各一個 worktree、ready queue 與 resources、integration cherry-pick、依賴同步、越界與衝突處理、有界復會 | 兩位 agent 實際平行寫入時不互相覆蓋，下游能讀到整合後的成果，衝突與硬體資源競爭可以停止並恢復 |
| **P4 按需求追加** | `ask`、完整的重指派 UI、run 匯出與重播 | 有需求才加；重播先檢視事件，不重新執行副作用 |

實作時使用隔離的 repo 與假 CLI，留下最小可執行的檢查；只用少量真 CLI 確認權限與 MCP，不大量消耗模型額度。優先涵蓋：

- 假的 JSON 或事件串流、無效的 DAG、越界路徑，以及單一覆核者的 objection，都不能繞過定案與核准。
- UTF-8 長 prompt、引號、換行、特殊字元與含空白的路徑，都不能被當成 shell 指令執行。
- 沒有 ACK、重複回報 done、過期的 attempt，或來自其他 run 的回報，都不能推進到錯誤的任務。
- 在保存、送出 prompt 或 commit 的前後中止或崩潰，恢復後不盲目重送、不遺失 diff、不重複 commit。
- 基線是 dirty、HEAD 被外部改動、整合衝突，或前置任務失敗時，下游都不會錯誤地開始。

## 11. 主要風險與處置

| 風險 | 處置 |
|---|---|
| 規劃階段改檔或呼叫硬體 | 驗證唯讀；排除有副作用的 MCP/hooks 與工具憑證；不合格就改用附檔模式或不參與 |
| JSON 合法但計畫錯誤 | 語義驗證、每項異議都要處置、人工核准、驗收；schema 不保證品質 |
| 檔案、介面或資源衝突 | scope 預警、DAG 與介面契約、P2 循序、P3 worktree 加 resources |
| 重複送出 prompt，或誤送到審批畫面 | intent、attempt、ACK、專用 session；狀態不明時不重送，待審批時不注入 |
| 誤判完成、沒有回報、行程退出 | `reported` 與 `done` 分開，驗收並整合之後才算完成；終端輸出與 git 狀態只供診斷 |
| App 或行程崩潰 | 一律 `paused` 交給人核對、保留成果、原子化 manifest 加備份、commit 帶身分標記 |
| **使用者看不到進行中的改動** | Cowork 分頁提供 worktree diff 檢視；開始前列出被排除的 dirty 檔案 |
| 外部變動或全域設定衝突 | per-repo 單一 run、固定 run root、git 邊界檢查、session 專屬設定；不支援跨機 |
| 成本失控或會議沒完沒了 | 整個 run 的呼叫次數與時間上限、有界的修正與復會、未知用量明示 |
| **聊天室外觀讓人期待自由對話** | 時間軸上的協議標記（§6.2）；R2 並排並標明「彼此看不到」，不做擬真會議室（§6.3） |
| 長時間呼叫沒有輸出，看起來像當掉 | 與會者列顯示「思考中 經過時間／上限」並可取消（§6.3） |
| 清理時刪掉成果 | 先停止行程，檢查 dirty、未保存的成果與實際路徑，交由使用者決定 |

## 12. P1 實作紀錄（2026-10-07）

### 12.1 P0 探測結果（本機實測）

| CLI | 規劃可用 | 實際採用的唯讀設定 | 實測發現 |
|---|---|---|---|
| Claude Code 2.1.292 | ✅ | `-p --output-format json --json-schema <inline> --tools Read,Grep,Glob --allowedTools Read,Grep,Glob --restricted --strict-mcp-config --safe-mode --permission-prompts none --no-session-persistence --disable-slash-commands` | 只給 `--tools` 不夠：搭配 `--permission-prompts none` 時連 `Glob` 都會被拒，必須再用 `--allowedTools` 預先放行。`--restricted` 會擋下工作目錄外的讀取。沒有寫入工具，寫檔要求無效 |
| Codex 0.160.1 | ✅ | `exec --sandbox read-only --ignore-user-config --ignore-rules --ephemeral --json --output-schema <file> -o <file> -C <snapshot> -c windows.sandbox="<使用者設定值>" -` | `--ignore-user-config` 用來排除使用者的 MCP 與外掛（本機有 `agent-hub`），但 Windows sandbox 設定也一起丟了，結果**連讀檔都被擋**，所以要從 `~/.codex/config.toml` 讀回 `[windows] sandbox` 再用 `-c` 補上。**elevated sandbox 讀不到使用者目錄（AppData、Temp）**，D 槽上的 repo 可以 |
| Antigravity 1.2.14 | ✅（2026-10-08 補上） | `--input-format stream-json --output-format stream-json --json-schema <file> --effort medium --sandbox -p=`，stdin 送 `{"event":"user","message":{"role":"user","content":…}}`；環境變數 `USERPROFILE`／`HOME` 指向隔離家目錄 | 見下方「Antigravity 的唯讀方案」 |

**Antigravity 的唯讀方案**（agy 沒有任何 flag 能在單次執行時停用全域 MCP、外掛與 hooks，所以分四層）：

1. **隔離家目錄**：Go 在 Windows 以 `USERPROFILE` 找家目錄。指到 `userData/cowork/agy-home` 後，`agy mcp list` 回報「No MCP servers configured」，log 也顯示 `mcp_servers`、`command_assessor` hook 皆為空。**登入不受影響**：憑證在 Windows 認證管理員（`gemini:antigravity`），不在家目錄。
2. **權限規則**（隔離家目錄的 `.gemini/antigravity-cli/settings.json`）：deny `command(*)`、`write_file(*)`、`mcp(*)`、`read_url(*)`、`execute_url(*)`。實測被 deny 規則拒絕時，模型收到「Matches user-configured deny rule」的錯誤會改用別的工具，**不會中止**；沒有規則、需要確認的動作則在 headless 模式被直接拒絕並**中止整次執行**。工作目錄（快照）內的 `view_file` 預設允許；快照外的讀取（實測它曾試圖讀真正 repo 的 `.git/config`）會被擋。另外放行 agy 自己內建 skill 檔的讀取，否則它查自己的說明文件時會中止。
3. **即時工具白名單**：stream 的 `step_update` 在工具開始時就帶 `tool_name`。agy 還有 `schedule`、`send_message`、`browser_*`、`invoke_subagent`、`search_web` 等不一定受權限規則管的工具，所以只允許 `view_file`、`list_dir`、`grep_search`、`find_by_name`、`finish`（加上已實測會被規則溫和拒絕的 `run_command`），其他一出現就立刻終止行程，那次呼叫算失敗。
4. **快照核對**：與另外兩家相同。

實測：讓 agy 覆核真實會議裡 Claude 的計畫，它只用了 `view_file`（40 次），輸出通過 schema 與 `checkR2` 驗證，快照乾淨；它獨立找到了 Codex 當時找到的同一個漏洞，還多抓到一個正規式邊界問題。**代價**：那次 441 秒、約 24.6 萬 token（另有 194 萬快取讀取），所以 agy 的單次逾時下限是 600 秒、加上 `--effort medium`，預設規劃時間也從 10 分鐘調成 20 分鐘。不加 `--mode plan`：它會讓模型想把計畫寫成檔案。

### 12.2 與設計文件的偏離

1. **規劃快照放在 `<git common dir>/cowork/<runId>/snapshot`，不放 userData**（§2.1.5、§7 寫的是 userData）。原因是 12.1 的 codex sandbox 讀不到使用者目錄。快照用 `git clone --shared` 建立（物件走 alternates，不實際複製），不會在使用者 repo 的 `.git/worktrees` 留紀錄。`.git` 已在檔案監看的忽略清單裡，不會觸發檔案樹重整。代價：開會期間 OneDrive 會同步這份約 2 MB 的快照；會議核准、取消或刪除後就移除。**P2 的執行 worktree 會遇到同一個限制，要沿用這個位置，或另找 codex sandbox 讀得到的地方。** manifest 仍放 userData。
2. **新增 `approved` 階段**：P1 的終點。核准後在任務板上手動派送；修改計畫會產生新版本並回到 `awaiting-approval`。P2 之後由 `preparing`／`executing` 接手。
3. **新開終端的派送要等 CLI 啟動完成**：實測直接貼進剛啟動的 Codex，它還沒開啟 bracketed paste，開頭幾行的換行被當成一般按鍵吃掉（這次沒有被當成 Enter 送出，但不能賭）。改成等終端輸出靜止 1.5 秒（上限 20 秒）才貼；改完之後 Codex 正確辨識為一次貼上。已開著的終端照常直接貼。這是 §5.1「接收問題」的 P1 版緩解，P2 仍需要真正的 ACK。
4. **回到 Cowork 時預設打開的會議**：有進行中的就打開它；沒有的話，最近一場若是 `approved` 也打開（核准後正是要派送的時候）。
5. **R2 的 objection／missing／問題 id 一律由 App 重新編號**（`codex.o1`、`claude.q1`…），不信任模型自己取的 id，避免重複或撞名讓「每項都要處置」的檢查失準。

### 12.3 驗證

- `node --experimental-strip-types scripts/check-cowork.mts`：純邏輯、runner、原子寫入、git 快照，加上 12 組 orchestrator 端對端情境（假 CLI，輸出格式照實測）。涵蓋單一覆核者的反對不能被繞過、修正呼叫、覆核失敗、副作用作廢與快照重建、預算、取消、改板與版本綁定、待定項目、重啟後暫停。
- **真實會議一場**（主席 Claude、覆核 Codex，需求：讓 dev 網址偵測支援 https）：3 次呼叫、沒有修正呼叫、127 秒，Claude $0.39、Codex 約 3 萬非快取 token。Codex 在覆核時找到真的漏洞（`TerminalPanel.tsx` 的前置條件只檢查 `http://`），主席據此新增 t3；7 項 issue 全部有處置；真正的 repo 沒有被改動。
- 實機 UI：亮色、暗色、暗色莫蘭迪截圖檢查，核准與派送到新 Codex 終端實際操作過。
- 既有檢查：`check-portDetect`、`check-usage`、`check-bgTasks`、`check-hosts`、`check-remote` 通過。`check-remote-files` 失敗是本機 AdGuard 改寫了 CSP 標頭；`check-terminal-ui`（要用 electron 跑）失敗在手機遙控頁分頁標籤的語言斷言。兩者都與 Cowork 無關，改動也沒碰到那些檔案。

### 12.4 模型與推理強度（2026-10-08）

「設定 › Cowork」與開會表單都可以替每一家選模型與推理強度，留空就用預設；開會時的選擇會固定在這場 run 上，之後改設定不影響進行中的會議。會議畫面的與會者列顯示每一家這場用的模型。

| CLI | 怎麼指定 | 可選清單從哪來 | 「預設」代表什麼 | 實測 |
|---|---|---|---|---|
| Claude | `--model`、`--effort`（low～max） | 別名 fable／opus／sonnet／haiku，可自訂 | 你 `~/.claude/settings.json` 的模型（`--safe-mode` 不影響模型選擇） | `--model sonnet --effort low` → 回報實際模型 `claude-sonnet-5-5` |
| Codex | `-m`、`-c model_reasoning_effort="…"` | `codex debug models`：模型目錄，含每個模型支援的強度（例如 gpt-6.1-sol 到 ultra） | **你 `~/.codex/config.toml` 的 `model` 與 `model_reasoning_effort`**。規劃時用 `--ignore-user-config` 排除 MCP 與外掛，會把這兩項一起丟掉，所以 Cowork 讀回來再傳；修正前 Codex 會悄悄改用它內建的預設模型 | `-m gpt-6.1-sol -c model_reasoning_effort="low"` 在 `--ignore-user-config` 下有效 |
| Antigravity | 見下 | `agy models`（以隔離家目錄執行），把 `-high`／`-medium`／`-low` 變體合併成一個模型 | agy 預設模型 + `--effort medium` | 見下 |

Antigravity 的模型 ID 本身就帶強度：`--model gemini-3.8-flash-medium` 加 `--effort low` 會衝突；不支援強度的模型（`claude-sonnet-4-6`）加 `--effort` 會被拒；基本名稱加強度（`--model gemini-3.8-flash --effort low`）可以。所以選了有變體的模型時一定要選強度（自動選 medium，沒有就選第一個），送出 `--model <基本名稱>-<強度>`，這個 ID 一定在清單裡；沒有變體的模型或自訂的完整 ID，強度選單顯示「由模型決定」。

模型名稱與強度會接進命令列（codex 的強度還會被當成 TOML 值），只接受英數字與 `. _ - : / @`，不合法的自訂名稱會在輸入框標紅，不會被默默換成預設。只有 Claude 會回報實際用的模型；Codex 與 Antigravity 顯示的是指定的值。

### 12.5 規劃參考資料與背景執行（2026-10-08）

使用者的要求：規劃（開會）只碰 skill 與專案指示；核准後在 Cowork 裡選擇開始執行，任務在背景交給對應的 agent，執行時功能全開；最終回覆與後續互動都留在 Cowork，不切到終端。

**規劃參考資料。** 「設定 › Cowork › 規劃參考資料」可以勾選 skill（沿用擴充功能頁的盤點，只列有 SKILL.md 的），並決定是否附上專案指示。開會時把基線 commit 的 `CLAUDE.md`／`AGENTS.md`／`GEMINI.md`（`git show <base>:<file>`，不讀工作區裡未 commit 的版本）與勾選的 SKILL.md 內容固定在 `run.context`，當成「資料」附在每一輪規劃 prompt 的開頭，並註明不能改變唯讀規則。skill 的路徑由 main 從盤點結果解析，不接受 renderer 傳來的路徑。單檔 24 KB、總計 96 KB 上限。MCP、外掛與 hook 在規劃時維持關閉（§12.1 的唯讀方案不變）。

**背景執行。** 核准後的「輪到你」卡片改成執行選項：

| 選項 | worktree 與分支 | 排程 | 收尾 |
|---|---|---|---|
| 依序執行 | 一個：`.cowork/<runId>/main`，分支 `cowork/<runId>/main` | 依相依順序一次一項，每項疊在前一項上 | 結果就是 main 分支 |
| 同時執行 | 每家 agent 一個：`.cowork/<runId>/<agent>`，分支 `cowork/<runId>/<agent>` | 不同 worktree 的任務可同時跑；同一個 worktree 一次一項；前置任務在別的 worktree 完成時，先 cherry-pick 它的 commit 再開始 | 從基線開 `cowork/<runId>/integration`，依相依順序 cherry-pick 所有任務的 commit；衝突就還原並回報是哪一項 |

- **位置**：worktree 開在使用者的 repo 裡（`<repo>/.cowork/<runId>/`），`git worktree list` 與分支清單看得到平行展開的狀態。`.cowork/` 寫進本機的 `.git/info/exclude`，主工作區的 `git status` 保持乾淨，也不動 repo 追蹤的 `.gitignore`。這也避開了 Codex 的 Windows sandbox 讀不到使用者目錄的問題（§12.1）。
- **依賴**：可選擇把主工作區的 `node_modules` 用 junction 連進 worktree（只在 worktree 會忽略 `node_modules` 時才連，免得被 commit）；清理時先拆 junction 再移除 worktree。
- **完整設定**：執行用使用者平常的 CLI 與設定，沒有規劃時的隔離（agy 不用隔離家目錄、codex 不加 `--ignore-user-config`），所以使用者的 skill、MCP、外掛與 hook 都會載入。模型與強度沿用這場會議的選擇。
- **權限**：照開始執行當下「設定」裡的 Bypass（整場沿用，記在 `execution.bypass`）。開啟時分別帶 `--permission-mode bypassPermissions`、`--dangerously-bypass-approvals-and-sandbox`、`--dangerously-skip-permissions`。關閉時用不需要人點的受限模式：Claude `--permission-mode acceptEdits`（只自動接受改檔）、Codex `-c sandbox_mode="workspace-write"`；agy 沒有這種模式，遇到需要核准的工具會中止，畫面會提示。
- **headless 與接續**（2026-10-08 實測，三家都能在同一個 session 第二輪改檔）：

| CLI | 第一輪 | 接續 | session id 來源 |
|---|---|---|---|
| Claude | `-p --output-format stream-json --verbose` | 加 `--resume <id>` | `system/init` 與 `result` 的 `session_id` |
| Codex | `exec --json -o <file> -C <worktree> -` | `exec resume --json -o <file> <id> -` | `thread.started` 的 `thread_id` |
| Antigravity | `--input-format stream-json --output-format stream-json -p=` | 加 `--conversation <id>` | `init` 的 `conversation_id` |

- **進度與回覆**：逐行解析 stream，把工具動作縮成一行（路徑轉成相對 worktree；Codex 在 Windows 包的 `powershell.exe -Command` 外殼會去掉），最近 40 筆存在這一輪；回覆取最終結果（Codex 用 `-o` 的最後一則訊息補）。時間軸在「開始執行」分隔線下為每個任務放一張卡：Cowork 的交辦（折疊）、進度、回覆、使用者的追問。與會者列改顯示每家正在做哪一項。
- **追問**：輸入框改成「選任務 → 送出」，接續該任務的 session；改動另成一個 commit，整合結果重算。也可以對失敗、卡住或被停止的任務「重新執行」（新的 session，worktree 裡已有的修改保留）。
- **checkpoint**：agent 被要求不要自己 commit、切分支或動 worktree；每一輪成功結束後由 Cowork `git add -A` 並 commit（訊息 `cowork(<runId>): <taskId> <title>`）。repo 沒設 `user.name`／`user.email` 時用 `Agent Workbench Cowork` 的身分，有設就用使用者的。
- **停止、預算與重啟**：「停止」終止還在跑的 agent 並暫停排程；「繼續」時被停掉的任務有 session 就接續，沒有就重排。執行時間只算有 agent 在跑的區間，平行只算一次，用完就自動停止（`maxExecutionMinutes`）。app 重啟時還在跑的任務標成被中斷，整個執行先暫停等使用者繼續。
- **收尾**：任務都完成後進入 `review`，顯示結果分支與 `git diff --stat`。「合併到 <來源分支>」只在主工作區乾淨、而且還在原本的來源分支時執行 `merge --no-ff`，衝突就 `merge --abort` 還原，交給使用者自己合併。「移除 worktree」只動 `.cowork/` 底下，分支保留。還有 worktree 的 run 不能刪除紀錄；已開始執行的 run 不能用「取消會議」。

新的階段：`executing`、`review`、`completed`（終態）。`review` 仍佔用「每個 repo 一個活動 run」的名額，合併或清理後才能開下一場。

與 §5 原設計的差異：P2 原本規劃用專用 PTY 互動派送，靠 MCP 回報（`cowork.task()` ACK）判斷完成，而且只能輪流執行。改成 headless 之後，完成與否看 CLI 的最終結果與結束碼，不需要 MCP 回報與畫面判斷；也因此可以同時執行（原 P3）。尚未實作：跨 dev／打包版的 repo lock（§7）、`resources` 的互斥（§2.3）、整合衝突的協助解決。

### 12.6 已知限制

- 終端停靠在底部（預設 300px 高）時，會議畫面很擠。停靠右側，或搭配 PR #35（預設收起中央欄）就有足夠空間。
- 遇到未知的 `schemaVersion` 時目前直接略過，不是 §7 說的「只能讀取或匯出」。
- 規劃預算只能約束呼叫次數與時間；Claude 有實際金額，Codex 只有 token，兩者都不是精確的花費上限（§8）。
- 背景執行時 agent 的完整對話只存在各家 CLI 自己的 session 裡；Cowork 只保留每一輪的最終回覆與最近 40 個工具動作。

## 附錄 A：外部參考

原稿參考過 [herdr](https://github.com/herdrdev/herdr)（Rust 寫成的 coding agent 終端多工器）的終端狀態判斷與派送設計。授權為 **Apache-2.0**（✅ 2026-10-07 讀過 repo 根目錄的 `LICENSE` 確認）；本專案為 MIT。

- 不採用通用 mesh 與完整的終端規則引擎；明確的 run/task 回報介面已經足夠。herdr 需要那套複雜度，是因為它刻意不碰 agent 的設定，只能從畫面外面猜狀態；本工作台能產生 agent 的原生設定，headless 路徑也能完全繞開畫面解析。
- **逾時不代表沒有送到，不可以盲目重送**：這條教訓同樣適用於 Cowork 的 PTY。headless 規劃繞開了畫面解析，但沒有解決互動執行時「對方到底收到沒有」的問題。
- 未來若要採用外部程式碼，先查實際的 LICENSE/NOTICE 與適用檔案，保留要求的標頭、出處與 notice。Apache-2.0 可以併入 MIT 專案，但必須保留 notice，不能直接拿走。

## 附錄 B：修訂沿革

| 版本 | commit | 主要變更 |
|---|---|---|
| 原稿（Claude Opus 5） | `98d4e23` | 提出 headless 規劃加互動執行、主席會議協議、三層狀態分離 |
| 覆核修訂（Codex） | `c169608` | 強制 worktree、R3/R4 合併並修正兩人時的仲裁漏洞、規劃唯讀、userData 儲存、attempt/ACK/恢復機制；修正三項事實錯誤（codex 的 schema flag、adapters.ts 缺少 Codex MCP 輸出） |
| 合併定稿（Claude Opus 5.5） | `171b056` | 新增「設計主軸」；補回自由討論的反對理由（§4.1）與協議標記（§6.2）；把 worktree 的使用者可見後果列為 P2 必做（§2.4，含 `preparing` 階段與 diff 檢視）；回報介面改為只帶 attemptId（由 attempt 綁定 revision）；崩潰恢復的候選 commit 改為一律交人確認；恢復 herdr 授權的已驗證事實；明示 P2 是輪流執行 |
| 會議室呈現層（Claude Opus 5.5） | `bf14bb8` | 新增 §6.3：與會者列、進行中指示、「輪到你」卡片、R2 並排、訊息與任務板連動；列出不做的擬真元素與理由 |
| P1 實作紀錄（Claude Opus 5.5） | `a77eef1` | 新增 §12：P0 探測結果、與設計的偏離（快照位置、`approved` 階段、新終端派送要等就緒）、驗證與已知限制 |
| Antigravity 參與規劃（Claude Opus 5.5） | `3ea87de` | §12.1 補上 Antigravity 的四層唯讀方案與實測；§3.1 能力表更新；預設規劃時間改為 20 分鐘 |
| 模型與推理強度（Claude Opus 5.5） | `223d949` | 新增 §12.4：三家可選模型與強度的來源與規則，Codex 預設改為沿用使用者 config.toml，Antigravity 的 ID 與強度對應 |
| 規劃參考資料與背景執行（Claude Opus 5.5） | 本版 | 新增 §12.5：規劃附上專案指示與勾選的 skill；核准後在 Cowork 裡背景執行（repo 內 worktree、依序或同時、照 Bypass 設定、追問接續 session、每輪 commit、合併與清理）；已知限制改為 §12.6 |
