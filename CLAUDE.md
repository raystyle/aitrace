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

## 自举纪律（自迭代进化）

- 本项目**必须通过自身的 aitrace 功能回归与验收测试**：改动后跑 `cargo test`，并用 MCP 工具验证录制链路——新编辑帧应携带 `agent_label` / `operation_id` / 双 intent 字段，且无 `*.tmp.*` 噪音帧。
- 测试失败或编辑序列出问题时，**调用 `/aitrace`，不要靠猜**：用 MCP 工具（`get_regression_window`、`diff_frames` 等）定位坏帧，而不是重读整棵树。
- 发现自身缺陷 → 修复 → 补回归测试 → 重新部署验收，如此循环（吃自己的狗粮，测试随能力进化）。

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
