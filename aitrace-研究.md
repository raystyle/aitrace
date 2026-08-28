# aitrace / vibetracer：AI 编程会话可观测与回放

调研日期：2026-08-28。对象是当前工作区 `D:\aitrace`（GitHub：`raystyle/aitrace`），及其上游 crate / 仓库 `vibetracer`。

## 需求

- 研究：
  1. 项目身份与定位：本仓库与上游分别是什么、要解决什么问题、官方叙事
  2. 作者、许可、版本、发布渠道，以及 `raystyle/aitrace` 与 `omeedcs/vibetracer` 的关系
  3. 功能版图：TUI cockpit、daemon、restore、分析引擎、多 agent、导出、MCP
  4. 技术架构与实现路径（记录层、UI、集成点、数据目录）
  5. 同类工具与生态位（会话回放、成本看板、git 归因、harness tracing）
  6. 社区讨论与用户反馈（X、GitHub issue/star、下载量）
  7. 安装与落地：crates.io、Homebrew、源码构建、Windows / CI 实际覆盖
- 核查：
  - `cargo install vibetracer` 是否在 crates.io 真实存在
  - 上游是否为 `omeedcs/vibetracer`
  - 本仓库是否为该上游的 fork，是否有独立演进
  - Claude Code / Cursor / Codex 支持是否有代码落地
  - MCP 是否真有 7 个工具
  - Homebrew `omeedcs/tap/vibetracer` 是否落地
  - 文档声称的平台支持 vs 实际（尤其 Windows）

意图：混合（以研究为主，核对本仓库 / README 的核心主张）。

已有锚点：`https://github.com/omeedcs/vibetracer`、`https://github.com/raystyle/aitrace`、crate `vibetracer` 0.6.2、作者 Omeed Tehrani（`omeedcs` / `@omeedtehrani`）。

## 结论

### 1. 身份与定位

本工作区 crate 名仍是 **vibetracer**（`Cargo.toml` `name = "vibetracer"`，版本 `0.6.2`）。GitHub 远端是 `raystyle/aitrace`，GitHub API 标记它为 **`omeedcs/vibetracer` 的 fork**，创建于 2026-08-28。两边最近推送时间同为 2026-03-29T05:03:20Z，本 fork 没有独立提交。

上游自我定位：**AI 编程会话的终端可观测 cockpit**。它不解析模型权重、也不做生产 APM，而是在项目目录旁记录每一次文件改动，把会话做成可 scrub 的时间轴，并允许按文件 / 按编辑外科手术式恢复。作者在 2026-03-21 的开源帖里把痛点说成：Claude 一次改 20–40 个文件后，人跟丢顺序、漏改调用点、误覆盖关键值，最后只能 `ctrl+z` 或 `git reset` 整场推倒。官方口号是 “the missing undo button”。

与「token 账单看板」「git 行级 AI 归因」「把 agent 轨迹送到 Phoenix/CloudWatch」不是同一层。vibetracer 的核心对象是 **工作树里的编辑帧**，会话日志（Claude JSONL、`.agent-trace/`）是给这些帧贴 agent / intent / token 的附属通道。

不要和同名产品混淆：电商推荐 SaaS Vibetrace、Python 函数级 `szamani20/VibeTracer`、网站扫描 vibetrace.app，都不是这个项目。

### 2. 作者、许可、版本、发布渠道、仓库关系

| 项 | 记载 |
| --- | --- |
| 作者 | Omeed Tehrani。GitHub `omeedcs`（Dallas, TX；站点 omeedtehrani.com）。X `@omeedtehrani`。crates.io owner 同为 `omeedcs`。 |
| 主业 | 个人站与 LinkedIn 记载其为 Constellation Space 联合创始人（YC W26，卫星星座操作系统），不是以 vibetracer 为公司产品。 |
| 许可 | MIT。`LICENSE` 版权行 `Copyright (c) 2026 Omeed Tehrani`。 |
| 上游仓库 | 2026-03-21 创建；126 commits；默认分支 `main`；未归档。2026-03-29 最后一次 push。2026-07-19 仍有 star 活动。无 GitHub Release（`latestRelease: null`，releases 列表长度 0）。 |
| 本仓库 | `raystyle/aitrace`，fork，0 star，0 fork。描述复制上游。 |
| 其他 fork | `Ecopavel81/vibetracer`、`vkn129/vibetracer`（后者 1 star）。均停在同一 push 时间。 |
| crate | crates.io 2026-03-25 上架 0.1.0，2026-03-29 到 0.6.2。8 个版本，全部由 `omeedcs` 发布。累计下载 **200**，近 90 天 **33**。约 1.7 万行 Rust。 |
| GitHub 热度 | 上游 29 star、3 fork、1 个 open issue、0 条 PR。作者 2026-04-10 发帖感谢 25 star。 |

八天内从 0.1.0 冲到 0.6.2，提交信息大量 `Co-Authored-By: Claude Opus 4.6`。这是一个 **8 天密集 vibe-coded 的个人工具**，之后约五个月没有新版本。

### 3. 功能版图

按 README 与源码目录，产品拆成七块：

1. **记录**：后台 daemon 监视项目目录，每个文件版本按 SHA-256 内容寻址存进 snapshot store，编辑写入 append-only JSONL。数据目录 `.vibetracer/`。
2. **TUI cockpit**（ratatui）：htop 式仪表盘（token/成本、编辑速率、文件热力图、agent 活跃、blast radius / sentinel / watchdog 摘要）；vim 四模式（Normal / Timeline / Inspect / Search）；命令面板 `:` / `Ctrl+P`；会话级 blame、annotation、bookmark、session diff。
3. **恢复**：scrub 只改视图；`R` 才写盘；有 restore log 可 `u` 撤销；checkpoint（手动 + 每 N 次编辑自动）；CLI `vibetracer restore`。
4. **分析引擎**：blast radius（声明式依赖、抓「改了 3/5 个耦合文件」）、sentinels（跨文件不变量）、watchdog（不该变的常量）、可配置 alerts（toast / flash / bell）。
5. **Claude 深集成**：探测 `.claude/` 后注册 `PostToolUse` hook；后台线程解析 `~/.claude/projects/` JSONL（prompt、工具树、token、cache hit）。
6. **多 agent 导入 / 导出**：导入 Claude JSONL 与 `.agent-trace/` JSON；导出 Agent Trace JSON 与 git notes。
7. **MCP**：`vibetracer mcp`，stdio JSON-RPC，7 个工具，配一份 `skills/vibetracer-review.md` 做回归二分工作流。

作者自己在 issue #1 里把「AI 读自己的编辑史做 self-correction」写成产品下一步；v0.4.0 已把 MCP 落地，该 issue 到调研日仍 open。

### 4. 技术架构与实现路径

实现栈：Rust edition 2024、ratatui 0.29、crossterm、notify + notify-debouncer-mini、similar（diff）、sha2、syntect、clap、serde/toml。`rust-toolchain.toml` 钉 `stable`。

运行时形状：

```
文件系统 watcher ──► snapshot store (SHA-256) + JSONL edit journal
                         │
                         ├─► TUI tailer（实时 cockpit）
                         ├─► analysis（每条 edit 评估）
                         ├─► Claude log parser（并行线程）
                         └─► MCP handlers（按帧重建 / 二分）
```

关键实现细节：

- **daemon 探活用 Unix `libc::kill(pid, 0)`**（`src/daemon/pid.rs`、`src/daemon/mod.rs` 的 `SIGTERM`）。这是 Windows 不能直接编译 / 运行 daemon 的硬依赖。
- **Cursor / Codex 探测并不分家**：`src/import/detect.rs` 只要看到项目下 `.agent-trace/` 就记名为 `"cursor"`。Codex 若也写同一目录，检测层不会标成 Codex。真正导入走 `AgentTraceImporter`，agent 名可覆盖。
- **Agent Trace 导出是自有 JSON**：`version: "0.1"`、`generator: "vibetracer"`、`contributions[]` 含 agent/model/timestamp/file/diff/reasoning/operation_id。未能从官方 Agent Trace 规范正文核对字段是否一致。
- **git-ai 导出是纯文本行**再 `git notes add -f`，不是 git-ai README 里那种带行级归因、随 rebase/squash 迁移的 authorship log。
- **CONTRIBUTING.md 过时**：仍列 `splash.rs`、`rewind/`、`pty/`、`equation/`、schema diff、refactor tracker；当前树没有这些模块。README 的架构图更接近现状。
- **Rust 版本自相矛盾**：README 写 1.70+；CONTRIBUTING 写 1.85+（edition 2024）。edition 2024 以 1.85 为准。
- **CI**：`.github/workflows/ci.yml` 只跑 `ubuntu-latest`（test + clippy + fmt）和 `macos-latest`（test）。无 Windows job。上游无 GitHub Release，因此 formula 里的 `releases/download/v…` 资产也不存在。

### 5. 同类工具与生态位

AI coding observability 在 2026 已经裂成几条产品线。vibetracer 占的是其中最窄、也最「像视频编辑器」的一条：**工作树编辑帧 + 外科恢复 + 把帧喂回 agent**。

| 切面 | 代表 | 与 vibetracer |
| --- | --- | --- |
| 编辑时间轴 / 按文件回滚 | **vibetracer**（29★，本项目） | 基准。本地 snapshot + TUI scrub + restore。 |
| 事后读 JSONL 做会话审计 TUI | [luoyuctl/agenttrace](https://github.com/luoyuctl/agenttrace)（123★，2026-08 仍在更新） | 覆盖更多 harness（Claude/Codex/Gemini/Cline/Cursor export 等），偏成本、失败、健康度、HTML 报告、CI gate。不做内容寻址恢复。 |
| 会话时间轴可视化 | [tg1482/vizier](https://github.com/tg1482/vizier) | 读 Claude / OpenCode 会话文件，Ink TUI 看对话流，不写盘回滚。 |
| Token / 成本看板 | vibe-coding-tracker、VibeTime、AI Observer、vibetracking.dev | 扫 `~/.claude` / `~/.codex` 算账单。不管「哪一次 Edit 改坏了哪一行」。 |
| Git 行级 AI 归因 | [git-ai-project/git-ai](https://github.com/git-ai-project/git-ai)（2512★） | 提交时把 agent/model/session 写入 git notes，跨 rebase 保持。vibetracer 只声称兼容导出，实现是简化文本。 |
| Harness 轨迹 → 评测平台 | [Arize-ai/coding-harness-tracing](https://github.com/Arize-ai/coding-harness-tracing)（36★，2026-05 发布） | prompt / tool / retry / latency 进 Phoenix 或 AX，做实验和 dashboard。 |
| 研究用会话合并回放 | RECAP（VS Code Copilot + shadow git，arxiv 2026-05） | 学术平台，不是 CLI 产品。 |
| 云指标 | AWS Coding Agents Observability（CloudWatch OTLP） | 团队配额与成本，不是单机时间轴。 |

生态位一句话：如果你要 **「现在把 `auth.rs` 恢复到 14:32 那一帧，并让 Claude 自己二分是哪次 Edit 引入回归」**，这是 vibetracer 的设计中心；如果你要账单、组织级 %AI、或跨 harness 评测，应看 git-ai / Arize / agenttrace。它在同类 TUI 里 star 低于后发的 `luoyuctl/agenttrace`，且上游已停更约五个月。

### 6. 社区讨论与用户反馈

公开讨论几乎全是作者自播，没有形成第三方评测或使用报告。

- **X `@omeedtehrani`**：2026-03-21 发布（906 浏览、3 like）；03-26 宣布 crates.io v2 并 @garrytan / YC（807 浏览）；同日宣布 v0.4.0 MCP 七工具；04-10 感谢 25 star（132 浏览）。没有可见的技术回复线程。
- **LinkedIn**：同日转载开源帖，叙事与 X 一致。
- **GitHub**：唯一 issue #1 是作者自己的 MCP + skill 设计稿（2026-03-26），无评论。无 PR。无 Discussions 材料进入本轮。
- **中文检索**：「vibetracer AI 编程 可观测」只命中上游 README，没有中文评测或移植帖。中文社区在谈的是 VibeTime（时长）、agenttrace（会话审计）等邻近产品。
- **X 领域讨论**（不当成 vibetracer 的用户证言）：编码 agent 的 replay 难题（轨迹 + 环境 + 干预）、Codex Record & Replay、gitlogue 这类「提交回放 TUI」有热度，但没有把 vibetracer 点名为解决方案。

结论：产品叙事清楚，采用面很小（200 次 cargo 下载、29 star），停更后没有社区接力。

### 7. 安装与落地

文档给出三条路径，实际状态不同。

| 路径 | 文档 | 核实结果 |
| --- | --- | --- |
| `cargo install vibetracer` | README / crates.io | **成立**。crate 0.6.2 存在，安装的是同名二进制。 |
| `git clone … && cargo install --path .` | README | 源码树完整，有测试。需要 edition 2024（≥1.85），不是 README 写的 1.70。 |
| `brew install omeedcs/tap/vibetracer` | README | **未能核实为可用**。`omeedcs/homebrew-tap` 仓库不存在；GitHub 上 0 个 Release；`homebrew/vibetracer.rb` 仍写 `version "0.1.0"` 且 sha256 是占位注释。 |
| Windows | 未写支持；CONTRIBUTING 写 macOS or Linux | **当前不能按文档当一等公民**。daemon 用 `libc::kill`；CI 无 Windows；本工作区却在 Windows 上。fork 若要在本机可用，需要先做进程探活 / 停止的 Windows 实现。 |

落地集成点（有代码）：Claude `PostToolUse` hook 自动注册；MCP stdio；`.vibetracer/config.toml`；skill 文件 `skills/vibetracer-review.md`。Cursor/Codex 依赖项目内 `.agent-trace/`，不是 Cursor 扩展商店式集成。

---

### 核查项

| 主张 | 结论 |
| --- | --- |
| `cargo install vibetracer` 可从 crates.io 安装 | **成立**（0.6.2，200 次累计下载，2026-03-29） |
| 上游是 `omeedcs/vibetracer` | **成立** |
| `raystyle/aitrace` 是该上游的 fork，且尚无独立演进 | **成立**（GitHub `fork: true`，parent `omeedcs/vibetracer`，pushedAt 相同） |
| 支持 Claude Code、Cursor、Codex CLI | **部分成立**：Claude 有 hook + JSONL 解析；Cursor/Codex 走 `.agent-trace/` 同一导入器，检测层把该目录一律标成 `cursor`。不是三个对等的一等集成。 |
| MCP 7 工具 | **成立**。`src/mcp/tools.rs` 的 `all_tool_definitions()` 列出 `list_sessions`、`get_timeline`、`get_frame`、`diff_frames`、`search_edits`、`get_regression_window`、`subscribe_edits`，与 README / v0.4.0 发帖一致。 |
| Homebrew tap 可用 | **未能核实**；现有证据指向未落地。 |
| 跨平台（含 Windows） | **不成立 / 文档过时**。CONTRIBUTING 与 CI、`libc::kill` 均指向 Unix。 |
| 导出兼容官方 Agent Trace / git-ai | **未能核实**。本地是自有 JSON 与简化 git notes 文本；未拉到 Cognition Agent Trace 规范正文做字段对照。 |

## 事实源

1. **github** · `omeedcs/vibetracer` 仓库元数据（创建 2026-03-21，29★，3 fork，MIT，最后 push 2026-03-29，无 latestRelease）· 需求 2、7 · 上游身份与热度。
2. **github** · `raystyle/aitrace` 仓库元数据 + `fork/parent` 字段（创建 2026-08-28，parent `omeedcs/vibetracer`，pushedAt 与上游相同）· 需求 1–2、核查 fork · 本仓库是未演进的 fork。
3. **github** · `omeedcs/vibetracer` README（抓取正文，与本地 README 同构）· 需求 1、3、4、7 · 官方功能与安装叙事。
4. **github** · `omeedcs/vibetracer` issue #1（2026-03-26，作者自开，仍 open）· 需求 3、6 · MCP self-correction 设计稿，提出 7 工具与 `/vibetracer-review` skill。
5. **github** · `src/mcp/tools.rs` blob `846de291…` / commit `72a81b0` · 需求 3、核查 MCP · 7 个工具定义在代码里。
6. **github** · 最近 commits（作者 Omeed Tehrani，2026-03-29 一批 cockpit / demo / 0.6.2，Co-Authored-By Claude Opus 4.6）· 需求 2、4 · 8 天内版本冲刺、AI 合著。
7. **github** · forks 列表：`raystyle/aitrace`、`Ecopavel81/vibetracer`、`vkn129/vibetracer` · 需求 2。
8. **github** · `users/omeedcs`（姓名、blog、twitter、location Dallas）· 需求 2。
9. **github** · 上游无 releases（API length 0）；`omeedcs/homebrew-tap` 不存在 · 需求 7、核查 Homebrew。
10. **github** · `luoyuctl/agenttrace` 123★（2026-08-26 更新）、`git-ai-project/git-ai` 2512★、`Arize-ai/coding-harness-tracing` 36★ · 需求 5。
11. **web** · https://crates.io/api/v1/crates/vibetracer （2026-03-25 创建，0.6.2 于 2026-03-29，downloads 200 / recent 33，owner Omeed Tehrani）· 需求 2、7、核查 cargo install。
12. **web** · https://crates.io/crates/vibetracer 页面元数据（与 API 一致）· 同上。
13. **web** · https://www.omeedtehrani.com/ （YC W26 / Constellation Space 联合创始人，链到 github.com/omeedcs 与 x.com/omeedtehrani）· 需求 2。
14. **web** · LinkedIn 帖 https://www.linkedin.com/posts/omeedtehrani_new-open-source-project-httpslnkdin-activity-7440957405515100160-PVuG （2026-03-21）· 需求 1、6 · 与 X 开源叙事同文。
15. **web** · https://github.com/git-ai-project/git-ai/blob/main/README.md · 需求 5、核查导出兼容 · git-ai 用 git notes 做行级归因，能力远宽于 vibetracer 的文本 notes 导出。
16. **web** · https://arize.com/blog/open-source-coding-agent-tracing/ （2026-05）· 需求 5 · harness tracing 进 Phoenix/AX，覆盖 Claude/Cursor/Codex/Copilot/Gemini。
17. **web** · https://aws-observability.github.io/observability-best-practices/ai/coding-agents-observability/ · 需求 5 · 云侧 OTLP 指标，不是本项目。
18. **x** · post `2035189787749920846` @omeedtehrani 2026-03-21（906 views）· 需求 1、6 · 开源动机与产品定义全文。
19. **x** · post `2036958723470746018` 2026-03-26 · 需求 2、7 · crates.io v2、`cargo install vibetracer`。
20. **x** · post `2037066023871062228` 2026-03-26 · 需求 3、核查 MCP · v0.4.0 七工具名单。
21. **x** · post `2042670532429254942` 2026-04-10 · 需求 6 · 感谢 25 stars。
22. **本地树（与上游同 sha 的 fork 工作区）** · `Cargo.toml` 0.6.2；`LICENSE`；`src/import/detect.rs`；`src/export/agent_trace.rs`；`src/export/git_notes.rs`；`src/daemon/pid.rs`；`src/mcp/tools.rs`；`skills/vibetracer-review.md`；`homebrew/vibetracer.rb` 0.1.0 占位；`.github/workflows/ci.yml` ubuntu+macos；`CONTRIBUTING.md` macOS/Linux、1.85+、过时模块图 · 需求 2–4、7 及全部核查项。

## 缺口

- **Agent Trace 官方规范正文未拉到**：`cognition.com/blog/agent-trace` DNS 失败；不能断言导出 JSON 与 Cursor / Cognition / git-ai 规范逐字段兼容。对应核查项「导出兼容」保持未能核实。
- **未实际执行 `cargo install` / `brew install` / Windows 构建**：结论来自 registry API、仓库存在性、源码与 CI，不是本机安装实验。技能要求不跑命中里的代码。
- **X 第三方用户证言缺席**：除作者账号外，关键字 `vibetracer` 的 Latest 结果被无关同名账号 `@VibeTracer`（动物视频）污染；语义检索未找到点名本项目的从业者讨论。对应需求 6 的「领域人群讨论」仅有作者自播。
- **中文材料无独立源**：中文检索没有评测、教程或 issue。需求 7 的中文落地经验为空。
- **Homebrew tap 源码未找到**：不能排除私有 tap 或未推送的 gist；公开 GitHub 上不存在 `omeedcs/homebrew-tap`。
- **Cursor / Codex 真实日志格式未对照官方文档**：只核了本仓库 importer 结构。`.agent-trace/` 是否仍是 Cursor/Codex 现行输出目录，本轮未抓官方文档确认。
- **上游停更原因未记载**：无 issue、无 changelog 说明 3 月底后停止发布。作者主业转向 YC 公司是时间线旁证，不是仓库内陈述。
- **GitHub code_search 对 `windows` 在上游 Rust 代码中返回空**：与本地 `libc::kill` 一致，但不能排除将来分支；本轮只看了 `main` @ `72a81b0`。
