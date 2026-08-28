# aitrace

AI 编程会话的可观测性驾驶舱：记录每一次文件编辑，关联 Claude Code 的会话元数据（agent、工具调用、意图），并通过 MCP 把时间线暴露给 agent——让 agent 能够检查并修正自己引入的回归。

**当前状态：Claude Code · Windows + Linux · Beta（v0.7.2，目标 1.0.0 三平台）**

```
cargo run --release
```

---

## 当前状态（Beta · 通往 1.0.0）

三平台 CI 全绿（ubuntu / macos / windows）；真机全量测试：Windows 10/11 + Linux（WSL）。以下能力已端到端验证：

- **全链路录制**：`PostToolUse` hook → daemon → 快照存储 + 编辑日志，每帧携带 `agent_label`、`operation_id`（`session:tool_use_id`）、`operation_intent`（assistant 声明的意图）、`intent`（触发编辑的用户请求）
- **MCP server**：7 个 timeline 工具，agent 可直接查询帧、diff、回归窗口
- **自纠工作流**：`/aitrace` skill，测试失败时按时间线定位回归帧并外科手术式修复
- **覆盖安装与版本验证**：daemon 停止 → 构建 → 重启的验收流程，MCP 进程重连即升级

跨平台注意事项：Windows AF_UNIX 走 `uds_windows`，Unix 走 std；macOS FSEvents 报告解析后的路径（`/private/var/...`），关联键已做 canonical 兜底。待办：SessionStart/Stop hook 脚本目前为 PowerShell（Linux 下报错但不阻塞录制）；macOS 真机验收。以下能力继承自上游（vibetracer），代码保留但**未验证**：Cursor / Codex CLI 会话导入、git-ai 导出。

### 路线：1.0.0

- [x] 三平台 CI 矩阵全绿（`workflow_dispatch` 手动触发；fork 仓库 push 触发待启用）
- [x] Linux 真机（WSL）全量测试通过
- [ ] Linux 下 Claude Code 集成接管开发
- [ ] macOS 真机验收
- [ ] hook 脚本跨平台化（去 PowerShell 依赖）
- [ ] 1.0.0：三平台 + 文档 + 冻结 MCP 工具面

---

## 快速开始（Windows）

```bash
git clone https://github.com/raystyle/aitrace
cd aitrace
cargo build --release
```

```powershell
# 启动后台录制 daemon（独立进程，关终端不影响）
.aitrace\bin\aitrace.exe daemon start
.aitrace\bin\aitrace.exe daemon status
```

在本目录启动 Claude Code：接受工作区信任对话框，然后 `/mcp` 批准 `aitrace` 服务器。此后每次 Write/Edit 都会被记录，`/aitrace` 随时可用。

---

## Claude Code 项目集成

本仓库自带项目级（非用户级）配置，只在本目录生效：

| 表面 | 文件 | 说明 |
| --- | --- | --- |
| Hook | `.claude/settings.json` | `PostToolUse`（`Write\|Edit`）执行 `<项目>/.aitrace/bin/aitrace.exe hook-send` |
| MCP | `.mcp.json`（仓库根，不在 `.claude/` 下） | 服务器名 `aitrace`，stdio JSON-RPC |
| Skill | `.claude/skills/aitrace/SKILL.md` | `/aitrace` 自纠工作流 |
| 常驻指令 | `CLAUDE.md` | Claude Code 会话级约定 |

Hook 转发的元数据：`session_id`（→ agent 身份）、`tool_use_id`（→ 操作分组）、`transcript_path`（→ 意图解析）。daemon 沿 transcript 的 `parentUuid` 父链回溯出 assistant 声明的操作意图，并从 `last-prompt` 取用户请求。

在其他项目使用时：daemon 检测到该项目有 `.claude/` 目录即自动在 `settings.local.json`（gitignore）注册本地 PostToolUse hook——除非该项目已提交的 `settings.json` 自行定义了 aitrace 处理器。会话日志（`~/.claude/projects/`）由后台线程解析。

---

## MCP 工具

```bash
aitrace mcp   # 启动 stdio JSON-RPC 服务器
```

| 工具 | 说明 |
| --- | --- |
| `list_sessions` | 列出已录制会话及元数据 |
| `get_timeline` | 编辑时间线（分页，可按文件过滤；含双 intent 字段） |
| `get_frame` | 重建任意时间点的文件精确状态 |
| `diff_frames` | 任意两帧之间的 unified diff |
| `search_edits` | 按 regex 查找触及某模式的历史帧 |
| `get_regression_window` | 圈定疑似回归的候选帧区间 |
| `subscribe_edits` | 订阅实时编辑通知 |

---

## 验收与回归流程（本项目的核心纪律）

每次回归 / 验收必须能回答"测的是哪个构建"：

1. **先停 daemon 再构建**——`aitrace daemon stop`（会顺带 reap `target\debug` 里误启动的 `--daemon-child`）。残留进程用 `aitrace daemon reap`（只杀 daemon，不杀 MCP）。cwd 不要设成 `target\debug`。
2. `target\debug\aitrace.exe daemon start` 自动把新 exe 装进 `.aitrace/bin/`（目标被锁时改名 `aitrace.exe.old` 让路）
3. **MCP server 不会自动升级**——`initialize` 回报 `serverInfo.version`（crate 版本），`/mcp` 检查；版本过期须重连 MCP（或重启 Claude Code）后再验收 MCP 侧改动
4. 报告必须写明 **git 短哈希 + 小版本号**

完整流程见 `.claude/skills/aitrace/SKILL.md` 的"受测二进制验证"一节。

---

## CLI 时间线

TUI 已于 0.7.0 移除——aitrace 是**无头**的 daemon + CLI + MCP：

```powershell
aitrace                      # daemon 状态 + 会话计数
aitrace replay <session-id>  # 文本时间线：帧号/时间/±行数/文件 + 双意图
```

历史帧的意图直接可见（无需界面）：`op:` 为 assistant 声明的操作意图，`ask:` 为触发它的用户请求。

---
## 配置

`aitrace init` 自动探测常量、schema、依赖。配置在 `.aitrace/config.toml`：

```toml
# .aitrace/config.toml

[theme]
preset = "tokyo-night"

[watch]
debounce_ms = 100
auto_checkpoint_every = 25
ignore = [".git", "node_modules", "target", "__pycache__", ".aitrace", "*.tmp.*"]
# ignore 支持整段路径组件精确匹配与 glob 模式（*.tmp.* 过滤编辑器原子写临时文件）

# 看门狗：注册的常量被改即告警
[[watchdog.constants]]
pattern = "MAX_RETRIES"
file = "src/config.rs"
severity = "critical"

[[watchdog.constants]]
file = "**/*.py"
pattern = 'EARTH_RADIUS_KM\s*=\s*([\d.]+)'
expected = "6371.0"
severity = "critical"

# 哨兵：跨文件不变量
[sentinels.feature_count]
watch = "src/model.rs"
assert_eq = "src/features.rs"
description = "feature count must match model input"

[sentinels.tensor_dims]
description = "tensor input dimensions must match feature count"
watch = "**/*.py"
rule = "grep_match"
pattern_a = { file = "config.py", regex = 'N_FEATURES\s*=\s*(\d+)' }
pattern_b = { file = "model.py", regex = 'input_size\s*=\s*(\d+)' }
assert = "a == b"

# 爆炸半径：声明文件依赖
[[blast_radius.manual]]
source = "src/auth.rs"
dependents = ["src/session.rs", "src/api/login.rs"]

# 可配置告警
[[alerts]]
name = "cost-warning"
when = "session_cost > 1.00"
action = "toast"
message = "Session cost exceeded $1.00"

[[alerts]]
name = "sentinel-break"
when = "sentinel_failures > 0"
action = "flash"
message = "Invariant broken"

[[alerts]]
name = "runaway-edits"
when = "edit_velocity > 15"
action = "bell"
message = "Unusually high edit rate"
```

---

## CLI 参考

```
aitrace [path]                          状态摘要（daemon 状态 + 会话计数）
aitrace replay <session>                文本时间线回放（帧号/±行/文件 + 双意图）
aitrace sessions                        列出历史会话
aitrace import [session]                导入 Claude Code 会话（上游能力）
aitrace restore <file> --edit-id <N>    恢复文件到指定编辑
aitrace export --format agent-trace <session>   导出 Agent Trace JSON（上游能力）
aitrace export --format git-notes <session>     写 git-ai git notes（上游能力）
aitrace mcp                             启动 MCP 服务器（stdio JSON-RPC）
aitrace daemon start|stop|status|reap   管理后台 daemon
aitrace init                            生成配置（自动探测）
```

---

## 架构

```
aitrace
  daemon/           后台录制器（文件监视 + 快照库 + 编辑日志）
    correlation.rs  hook↔watcher 事件关联（路径归一化 + FIFO 队列 + HOOK_GRACE 宽限窗口）
    intent_index.rs Claude Code transcript 增量索引（parentUuid 父链回溯解析意图）
    agent_registry.rs agent 注册与标签分配
  recorder/         共享录制逻辑（daemon 与 --no-daemon 模式共用）
  snapshot/         内容寻址存储（SHA-256）+ 追加式 JSONL 编辑日志
  checkpoint/       全项目状态检查点（手动 + 自动）
  restore/          文件恢复引擎 + 冲突检查
  analysis/         爆炸半径、哨兵、看门狗（每次编辑时评估）
  claude_log/       Claude Code 会话日志解析（后台线程）
  hook/             PostToolUse hook 注册 + hook-send（元数据转发）
  import/           多智能体会话导入（上游能力）
  export/           会话导出（上游能力）
  mcp/              MCP 服务器（stdio JSON-RPC，7 个工具）
```

daemon 把变更写入 `.aitrace/` 下的追加式 JSONL 编辑日志；每个文件版本按 SHA-256 存入内容寻址快照库。CLI（`sessions`/`replay`）与 MCP 工具直接读该数据。分析引擎在每条编辑上评估。

数据存在项目目录的 `.aitrace/`（已 gitignore）。

---

## 设计文档

调研与设计笔记在 [`docs/`](docs/)：

| 文档 | 内容 |
| --- | --- |
| [`aitrace-研究.md`](docs/aitrace-研究.md) | 原始调研：会话可观测与回放——目标与上游（vibetracer）分析 |
| [`aitrace-windows-支持.md`](docs/aitrace-windows-支持.md) | Windows 支持设计：uds_windows AF_UNIX、隐藏 daemon |
| [`claude-code-项目级配置.md`](docs/claude-code-项目级配置.md) | Claude Code 项目级集成：hook / MCP / skill |
| [`aitrace-review-skill.md`](docs/aitrace-review-skill.md) | 自纠 skill 的初版归档（生效版为 `.claude/skills/aitrace/SKILL.md`，命令 `/aitrace`） |

---

## 安装要求

从源码构建。无 crates.io 包，无 Homebrew tap（`publish = false`）。

- **Rust 1.85+**（edition 2024）
- **Windows 10 1809+**（AF_UNIX，经 `uds_windows`；不引入命名管道 / TCP / nightly）
- **Linux / macOS**：std AF_UNIX；构建与全量测试由 CI 三平台矩阵验证

---

## 贡献

见 [CONTRIBUTING.md](CONTRIBUTING.md)。

## 许可

MIT
