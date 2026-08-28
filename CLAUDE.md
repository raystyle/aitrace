# aitrace

Rust TUI：AI 编程会话可观测性。把每次文件编辑记录进 `.aitrace/`，关联 Claude Code hook 元数据与 transcript 意图，经 MCP 暴露时间线，让 agent 检查并修正自己的回归。当前阶段：**Claude Code · Windows · Beta**。

本仓库是 Claude Code 的工作目录。**只用项目级配置，禁止污染全局**：hook / MCP / skill 一律部署在本仓库（`.claude/settings.json`、根目录 `.mcp.json`、`.claude/skills/`），绝不写入 `~/.claude/` 或 `~/.claude.json` 用户级；本地覆盖放 `.claude/settings.local.json`（gitignored，勿提交）。不改用户的 rustup / cargo home 和 `D:\ohmypwsh`。首次在本目录启动：接受工作区信任对话框，`/mcp` 批准 `aitrace` 服务器。

## 命令

```bash
cargo test                  # 全量测试（先停 daemon，见下）
cargo clippy --all-targets
cargo fmt --check
aitrace daemon start|status|stop
aitrace sessions
aitrace mcp
```

## 编译 / 测试 / 部署流程（必须遵守）

1. **先停 daemon 再构建**：`.aitrace\bin\aitrace.exe daemon stop` → `cargo build` / `cargo test`。daemon 子进程从 `target\debug\aitrace.exe` 运行，不停则链接报 os error 5。
2. **部署**：`target\debug\aitrace.exe daemon start` 自动把运行中的 exe 装进 `.aitrace/bin/`（目标被锁时改名 `aitrace.exe.old` 让路）。hook 与 MCP 必须调用该项目二进制，不是 PATH 上的全局 `aitrace`——PATH 安装只是为了手敲方便。
3. **MCP server 不随部署升级**：涉及 `src/mcp/` 的改动，验收前提醒人类重连 `/mcp`（serverInfo.version 应为新版本）。
4. **验收标记**：每次回归 / 验收报告必须写 git 短哈希 + crate 版本。完整流程见 `.claude/skills/aitrace/SKILL.md` 的「受测二进制的验证」。

## 五原则 · 三层环架构（MAPE-K + PDCA + Reflexion 重构）

**三层嵌套环，五原则是环上的角色而非平行清单：**

```
慢环（跨会话）取件 → 自迭代（修复+标准化）→ 规则修订
中环（每轮收尾）自省（意图↔diff）→ 记忆沉淀 → 待办守恒检查
快环（每次编辑）自举验证（build/test/lint）+ aitrace 录制
```

- **自举（约束）**：用 aitrace 开发 aitrace；`cargo test` 全绿 + MCP 验证录制链路（帧带 `agent_label`/`operation_id`/双 intent，无 `*.tmp.*` 噪音）。验证器本身用变异验证保证（故意破坏实现，测试必须变红）。
- **自愈（快环失败响应）**：回归走 `/aitrace`：`get_regression_window`/`diff_frames` 定位坏帧 → 外科手术式修复，不猜、不回滚、不重读整棵树。
- **自迭代（执行引擎）**：缺陷的完成定义（DoD）四件套缺一不可——修复提交 + 回归测试 + 部署验收（哈希/版本）+ **教训标准化**（规则/测试/文档三选一，kaizen 第六步）。
- **自省（传感器）**：每轮收尾用 `search_edits`/`diff_frames` 审视自己的 patch——意图↔diff 一致性、意外文件、重做检测、最小化。对 diff 不对意图（无责原则），发现写入记忆并条件化到下次。
- **自进化（K 更新）**：收尾形成时间线记忆 `docs/timeline/<日期>.md`；教训回流为代码改进项或规则修订；新会话开工先读最新记忆取件。

**自洽公理（规则的免疫系统）：**

1. **可证伪**：每条规则必须绑定可观察的检查；无检查的不是规则，是愿望。
2. **待办守恒**：每轮 产 ≥1 且 消 ≥1（推进或关闭）；每项待办必带 DoD——防 Goodhart（垃圾待办凑数）与积压（只产不消）。
3. **终止**：一切自指自动化必须带一次性 guard（marker / 标志位），无 guard 的自指规则不予采纳（防无限回归）。
4. **分层不阻塞**：快环失败不进中环（先修再续）；中环产出不阻塞当轮交付。
5. **单一来源**：宪法（本文件，约束）/ 程序（SKILL，操作步骤）/ 知识（docs/timeline，历史事实）三层各司其职，同一事实只写一处。

## MCP 工具

`list_sessions`、`get_timeline`、`get_frame`、`diff_frames`、`search_edits`、`get_regression_window`、`subscribe_edits`。stdio server 读 `CLAUDE_PROJECT_DIR` 下的 `.aitrace/`。先启动 daemon 保证新编辑被录制；数据已在磁盘时 MCP 仍可读历史会话。

## 布局

- `src/main.rs` — CLI（`daemon`、`mcp`、`hook-send`、TUI）
- `src/daemon/` — 后台录制器；`correlation.rs`（hook↔watcher 关联 + HOOK_GRACE 宽限）、`intent_index.rs`（transcript 意图解析）、`agent_registry.rs`
- `src/mcp/` — stdio JSON-RPC
- `src/hook/` — hook 注册 + `hook-send`
- `src/tui/` — ratatui UI
- 数据：`.aitrace/`（gitignored）— 快照、`edits.jsonl`、`daemon.sock`、`bin/aitrace.exe`

## 约束

- crate / 二进制名 `aitrace`，不得改回 vibetracer。
- `publish = false`；无 Homebrew、无 crates.io。
- Windows 10 1809+（AF_UNIX 经 `uds_windows`）；不引入命名管道、TCP、nightly std AF_UNIX。
- CLAUDE.md 保持精简，长流程放 `.claude/skills/`。
