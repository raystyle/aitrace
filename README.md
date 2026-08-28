# aitrace

AI 编程会话的可观测性驾驶舱：记录每一次文件编辑，关联 Claude Code 的会话元数据（agent、工具调用、意图），并通过 MCP 把时间线暴露给 agent——让 agent 能够检查并修正自己引入的回归。

**当前状态：Claude Code · Windows · Beta（v0.6.4）**

```
cargo run --release
```

---

## 当前状态（Beta）

以下能力已在本仓库环境（Windows 10/11 + Claude Code）端到端验证：

- **全链路录制**：`PostToolUse` hook → daemon → 快照存储 + 编辑日志，每帧携带 `agent_label`、`operation_id`（`session:tool_use_id`）、`operation_intent`（assistant 声明的意图）、`intent`（触发编辑的用户请求）
- **MCP server**：7 个 timeline 工具，agent 可直接查询帧、diff、回归窗口
- **自纠工作流**：`/aitrace` skill，测试失败时按时间线定位回归帧并外科手术式修复
- **覆盖安装与版本验证**：daemon 停止 → 构建 → 重启的验收流程，MCP 进程重连即升级

以下能力继承自上游（vibetracer），代码保留但**未在本阶段验证**：macOS / Linux、Cursor / Codex CLI 会话导入、git-ai 导出。

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

## Cockpit 功能

> **TUI 需要独占终端**：不要在多路复用面板（rmux/tmux 类）或嵌套 shell 里运行——面板驱动共写屏幕并拦截键盘，会出现"回显串进 TUI、按键无响应"的假故障。用独立的 Windows Terminal / cmd 窗口运行。
>
> **诊断**：同一窗口跑 `aitrace --input-test`。框必须铺满整窗，`stdout after` 必须有 `VT-OUT`，`PE subsystem` 必须是 `CUI (3)`，按键要出现在 events 里。若 `PE subsystem` 是 `GUI (2)` 或框外仍有 `pwsh ❯` 且按键无事件，那是旧的 GUI 子系统构建——父 shell 没在等 TUI。详情写在 `.aitrace/input-test.log`。
>
> **中文输入法请切英文模式**（`Shift` 或 `Win+Space`）：raw mode 下字母键会被 IME 组字窗拦截，控制台收不到——表现为"按键全部无效"。方向键/Enter 不受影响。这不是 aitrace 的 bug，是 Windows 控制台与 IME 的固有限制（Vim 等终端程序同理）。

### 驾驶舱仪表盘（`D` 切换）

htop 风格总览：

- Token 与成本追踪（从 Claude Code 日志计算燃尽率）
- 编辑速率图（最近 60 秒每分钟编辑数）
- 文件热力图（按编辑次数排序的最热文件）
- Agent 状态（10 秒活跃窗口判定 active/idle）
- 操作进度、爆炸半径摘要、哨兵失败计数、看门狗状态、缓存命中率

### Vim 模态界面

四种模式，状态栏显示当前模式：

- **Normal**（默认）——导航、拖动播放头、面板切换
- **Timeline**（`t`）——时间线缩放、平移、轨道选择
- **Inspect**（`i`）——单帧深查：diff / 文件 / 会话上下文
- **Search**（`/`）——可组合过滤语法

命令面板（`:` 或 `Ctrl+P`）模糊搜索全部动作，MRU 置顶。

### 时间线与回放

```
+-- timeline -------------------------------------------------------+
| src/auth.rs      [====|==  |=====|=  ] ------>                     |
| src/config.py    [==  |    |=    |   ] ------>                     |
|                        ^                                           |
|                     playhead                                       |
+--------------------------------------------------------------------+
```

- 每文件横向轨道，编辑单元格按 agent 着色
- 全局播放头 + 独立每文件播放头（可分离拖动）
- 轨道 Solo / Mute、缩放（`+`/`-`）平移
- 命令视图（`g`）按 AI 操作分组
- 实时跟随（Live）与手动暂停/播放（`Space`）

### 调查工具

**搜索与过滤**（AND 组合语法）：

```
file:auth agent:claude kind:modify tool:Edit after:14:30 before:15:00 lines>20 op:refactor content:token
```

| 谓词 | 说明 |
| --- | --- |
| `file:` | 文件路径子串 |
| `agent:` | agent ID 或标签 |
| `kind:` | create / modify / delete |
| `tool:` | 工具名（Edit、Write 等） |
| `after:` / `before:` | HH:MM 偏移或编辑 ID |
| `lines>` / `lines<` | 变更行数过滤 |
| `op:` | 操作意图子串 |
| `content:` | grep diff 内容 |
| 裸文本 | 全字段模糊匹配 |

**Blame 视图**（`B`）——文件预览上逐行标注 agent 与操作归属。

**内联注释**（`A`）——代码旁显示操作意图（与 blame 互斥）。自 v0.6.4 起有真实数据（transcript 意图解析）。

**会话 diff**（`:diff from to`）——时间线上两点间的逐文件变更对比。

**书签**（`M` 创建，`'` 跳转）。

### 恢复系统

拖动播放头只是预览；恢复才写盘——两者是独立动作。

- **恢复文件**（`R`）——把播放头处版本写回磁盘
- **撤销恢复**（`u`）
- **内容寻址快照库**——每个版本按 SHA-256 存储
- **检查点**（`c`）——手动全项目快照 + 每 N 次编辑自动检查点
- **CLI 恢复**——`aitrace restore <file> --edit-id <N>` 无头恢复

### 分析引擎

- **爆炸半径**（`b`）——文件被改后哪些依赖可能需要同步；抓"改了 3/5 个耦合文件就收工"的半截重构
- **哨兵**——跨文件不变量规则（如"配置的特征数必须等于模型输入维度"）
- **看门狗**（`w`）——注册不许变化的常量（物理值、端点、阈值），被改即告警，分 critical / warning / info
- **可配置告警**——按会话状态触发 toast / flash / bell，条件解除自动重新武装

### 多智能体与导出（上游能力，未在本阶段验证）

- Claude Code / Cursor / Codex CLI 会话导入
- Agent 着色、Solo 过滤（`1`-`9`）、冲突指示（5 秒内同文件双 agent）
- Agent Trace JSON / git-ai git notes 导出

---

## 快捷键

### Normal 模式

| 键 | 动作 |
| --- | --- |
| `q` | 退出 TUI（daemon 继续运行） |
| `Q` | 退出 TUI 并停止 daemon |
| `?` | 帮助浮层 |
| `Space` | 播放 / 暂停 |
| `←` / `→` | 全局播放头逐帧拖动 |
| `Shift+←` / `Shift+→` | 单文件拖动（脱离全局） |
| `a` | 重新吸附全局播放头 |
| `t` / `i` / `/` | 进入 Timeline / Inspect / Search 模式 |
| `:` 或 `Ctrl+P` | 命令面板 |
| `g` | 编辑视图 / 命令视图切换 |
| `d` | 预览模式（文件 / diff） |
| `D` / `C` / `B` / `A` | 仪表盘 / 会话面板 / blame / 内联注释 |
| `M` / `'` | 创建书签 / 跳书签 |
| `R` / `u` / `c` | 恢复 / 撤销恢复 / 检查点 |
| `x` | 显示/隐藏恢复产生的编辑 |
| `s` / `m` | Solo / Mute 当前轨道 |
| `b` / `w` | 爆炸半径 / 看门狗面板 |
| `z` | 最大化聚焦面板 |
| `j` / `k` | 预览滚动 |
| `+` / `-` / `0` | 时间线缩放 / 复位 |
| `Tab` | 循环切换面板焦点 |
| `1`-`9` | Solo agent N（命令视图） |

### Timeline 模式（`t` 进入）

| 键 | 动作 |
| --- | --- |
| `Esc` | 返回 Normal |
| `←` / `→` | 平移时间线 |
| `↑` / `↓` | 选择轨道 |
| `+` / `-` / `=` | 缩放 / 复位 |
| `s` / `m` | Solo / Mute 所选轨道 |
| `Enter` | 播放头跳到所选轨道 |

### Inspect 模式（`i` 进入）

| 键 | 动作 |
| --- | --- |
| `Esc` | 返回 Normal |
| `n` / `p` | 下一帧 / 上一帧 |
| `d` | diff 视图 |
| `f` | 完整文件 |
| `c` | 会话上下文 |
| `Enter` | 展开详情 |

### Search 模式（`/` 进入）

| 键 | 动作 |
| --- | --- |
| `Esc` | 取消退出 |
| `Enter` | 锁定过滤并返回 Normal |
| `↑` / `↓` | 滚动结果 |
| 任意字符 | 追加查询 |

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
aitrace [path]                          监视目录（默认当前目录）
aitrace demo                            交互式功能演示
aitrace replay <session>                回放历史会话
aitrace sessions                        列出历史会话
aitrace import [session]                导入 Claude Code 会话（上游能力）
aitrace restore <file> --edit-id <N>    恢复文件到指定编辑
aitrace export --format agent-trace <session>   导出 Agent Trace JSON（上游能力）
aitrace export --format git-notes <session>     写 git-ai git notes（上游能力）
aitrace mcp                             启动 MCP 服务器（stdio JSON-RPC）
aitrace daemon start|stop|status        管理后台 daemon
aitrace init                            生成配置（自动探测）
aitrace --no-daemon [path]              单进程模式
aitrace --debug [path]                  写调试日志
```

---

## 主题

19 个内置主题，运行时命令面板切换，无需重启。

**深色**：dark（默认）、catppuccin-mocha、catppuccin-macchiato、gruvbox-dark、tokyo-night、tokyo-night-storm、dracula、nord、kanagawa、rose-pine、one-dark、solarized-dark、everforest-dark

**浅色**：light、catppuccin-latte、gruvbox-light、solarized-light、rose-pine-dawn、everforest-light

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
  tui/              终端 UI：模态系统、仪表盘、时间线、预览、面板
  claude_log/       Claude Code 会话日志解析（后台线程）
  hook/             PostToolUse hook 注册 + hook-send（元数据转发）
  import/           多智能体会话导入（上游能力）
  export/           会话导出（上游能力）
  mcp/              MCP 服务器（stdio JSON-RPC，7 个工具）
  theme/            19 套主题，运行时切换
```

daemon 把变更写入 `.aitrace/` 下的追加式 JSONL 编辑日志；每个文件版本按 SHA-256 存入内容寻址快照库。TUI 实时尾随编辑日志渲染驾驶舱。分析引擎在每条编辑上评估并触发告警。

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
- macOS / Linux 为上游继承能力，本阶段未验证

---

## 贡献

见 [CONTRIBUTING.md](CONTRIBUTING.md)。

## 许可

MIT
