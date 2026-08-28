# Windows 下 aitrace TUI：提示符叠层、按键回显与假空白预览

## 现场结论（已落地，2026-08-28 晚）

用户目验「TUI终于正常了」。自检画面把核查项 5 从「未能核实」钉死：

- `stdin after: 0x380`（EXTENDED + VT-INPUT，ECHO/LINE 已清）
- `stdout after: 0xf`（含 VT-OUT）
- 尺寸 `210×57` 铺满
- events **只有** `Resize`，框外仍有 `pwsh ❯`

根因是 `#![windows_subsystem = "windows"]`：pwsh 把 GUI 子系统 exe 当 notepad 一样立即还提示符，TUI 与 PSReadLine 共写同一控制台。QuickEdit / VT-OUT / 强制清屏是必要前置，但消不掉这张图。

已改为 CUI；机检 `tests/subsystem.rs`（`IMAGE_SUBSYSTEM_WINDOWS_CUI = 3`）。daemon 闪框仍靠 `CREATE_NO_WINDOW`。下文第 6 节「不建议改回 console」**作废**。

## 需求

- 研究：把用户贴出的 Windows 画面拆成可观察症状；对照 aitrace v0.6.5 的 TUI 实现与已有防护；在 ratatui / crossterm / Windows Terminal / ConPTY / pwsh 生态里找同类故障；给出复现、诊断和可落地修复方向。
- 核查：
  1. 这张图是不是 README 已写的「回显串进 TUI」。
  2. `pwsh ❯ aaaaaaaaa` 是否证明 raw mode 未清掉 ECHO。
  3. 分隔线上的「继续」是不是 CJK 双宽溢出。
  4. v0.6.5 / `aaa737c` 的 QuickEdit 硬化是否已经盖住这张图。
  5. `windows_subsystem = "windows"` + `AttachConsole` 是否就是本次争用源。

意图：研究为主，夹带现场核查。锚点：aitrace **v0.6.5**，git 短哈希本轮采集时为 `aaac9fb`，Windows + pwsh（Oh My Posh / Starship 风格提示符 `via v1.97.1`），画面日期 2026-08-28。

## 结论

### 1. 画面里其实叠了三类不同的东西

贴图不是「TUI 整体画崩了」一张单因图，而是三层叠在一起：

| 你看见的 | 对应组件 | 说明 |
|---|---|---|
| 中央 ASCII logo、`waiting for edits`、左右键/Space/R/c/g/t/? 提示 | `src/tui/widgets/preview.rs` 的 `render_empty_state` | 预览区走「空状态」分支。文案在源码里是 `start coding in another pane` + `aitrace will`；贴图变成 `start coding in aitrace will`，左半截被提示符盖掉。 |
| 右侧 TOKENS / COST / VELOCITY / FILES / AGENTS / OPS / WATCHDOG | 驾驶舱 dashboard | 这侧是活的：5 个文件、claude-code 13 idle、6 条 OPS（含「继续——守恒机检刚逮住我」）。 |
| 底栏时间线、`1/16`、五条文件轨道 | timeline | 会话里**已经有编辑**（约 16 帧）。 |
| `aitrace on  main is v0.6.5 via v1.97.1` 和 `pwsh ❯ aaaaaaaaa` | **pwsh 提示符 + 回显**，不是 TUI widget | 出现在预览区中下部，和快捷键提示同一行高。TUI 独占终端时这里不该有 shell。 |
| `─…──…─继续—…───` | 主区与时间线之间的分隔带混进了汉字 | 源码里这条分隔线只画 `─`（`event_loop.rs` 的 `HorizontalSep`）。「继续」来自别处：要么是上一屏残留，要么是 OPS 中文被错误宽度截断后的鬼影。 |

驾驶舱和时间线有数据，中央却显示「waiting for edits」——空状态被用在了两个完全不同的条件上：`current_edit()` 为 `None`，**或者**当前帧快照读不出来（`preview.rs` 两个分支都调用 `render_empty_state`）。右侧已有 5 个文件 / 16 帧时，这是**假空白**，不是「还没有任何编辑」。

### 2. 主故障：屏幕上是 TUI，键盘却在 shell 里

`pwsh ❯ aaaaaaaaa` 是定性证据。raw mode 生效时，控制台不得把按键写回屏幕；alternate screen 生效时，主缓冲区上的提示符应被藏起来。两者只要有一个真正成立，就不会同时看到完整 TUI 画幅和一条还在回显的 pwsh 提示符。

本仓库已经把这个症状写成已知假故障，而不是未发现的新物种。`README.md` 明确要求 TUI **独占终端**，不要在 rmux/tmux/嵌套 shell 里开——「面板驱动共写屏幕并拦截键盘，会出现『回显串进 TUI、按键无响应』」。`run_tui_guarded` 的注释写的是同一画面的另一成因：TUI panic 后留下死帧，shell 在上面继续画。

按可能性排序（高 → 低）：

1. **死帧 + shell 恢复**（TUI 已退出或从未真正切到备用屏，最后一帧留在主缓冲区，pwsh 重新画出提示符，随后敲的 `a` 被 shell 回显）。仓库里没有 `.aitrace/tui-panic.log`，所以不是这次捕获到的 panic；clean exit、备用屏序列被主机忽略、进程被宿主掐掉，都会留下同一视觉。
2. **宿主共写**（rmux / VS Code / Cursor / Claude Code 集成终端）。TUI 在画，父级或邻板仍拥有输入或继续刷 prompt。这与 README 禁令一致。
3. **raw mode / QuickEdit 在主 TUI 路径上没保住**。自检模式已经能清掉 ECHO/LINE/QUICKEDIT（见核查 4），但 Windows Terminal / ConPTY 会在运行中改写 console mode（微软终端 #19674），而且 `EnableMouseCapture` 的 ANSI 鼠标跟踪正是那个 issue 的触发组合之一。aitrace 主循环会 `EnableMouseCapture`。
4. **中文 IME**。README 写过：raw mode 下字母键被组字窗吃掉，表现为「按键全部无效」。它解释「TUI 收不到键」，**不解释**提示符里出现 `aaaaaaaaa`——那串字已经到了 pwsh。IME 最多是并列干扰，不是这张图的主因。

`aaaaaaaaa` 与当日记忆里待复验的 `ddd ttt` 怪串同类：键没进 TUI 动作表，却以字面形式出现在终端上。

### 3. 本仓库已经做了什么、什么还没闭环

启动路径（`src/tui/mod.rs`）按教科书顺序：`enable_raw_mode` → `harden_console_input`（清 QUICKEDIT/INSERT/MOUSE，保留 EXTENDED）→ `EnterAlternateScreen` → `EnableMouseCapture`。退出时反向恢复；panic 时 `run_tui_guarded` 强制 `disable_raw_mode` + `LeaveAlternateScreen`。

今天（2026-08-28）专门加了两条：

- `b21d398`：`--input-test` 把 console mode、尺寸、事件画在屏上并写入 `.aitrace/input-test.log`。
- `aaa737c`：TUI 启动清 QuickEdit。记忆文件仍把「Inline Annotations 人工验证」标成被 Windows 输入问题阻塞，**部署后待复验**。

本机残留的 `input-test.log` 是现场主机画像，不是主 TUI 那次运行：

```
stdin mode before raw: 0x1f7  ECHO, LINE, PROCESSED, MOUSE, QUICKEDIT, INSERT, EXTENDED
stdin mode after raw:  0x180  EXTENDED only
size: 120x14  →  Resize 210x28
随后 150s 只有 FocusLost/FocusGained，没有任何 Key 事件
```

三件事实：

- 硬化在**自检路径**上有效，ECHO/QUICKEDIT 能清掉。
- 用户终端会从 **120×14** 拉到 **210×28**。14 行刚够 layout 最低拆分（状态 1 + 分隔 1 + 主区 ≥3 + 分隔 1 + 时间线 5 + 分隔 1 + 键位 1）。贴图是超宽画幅，和 210 列吻合。`event_loop.rs` 对 `Event::Resize` 只 `continue`，下一帧靠 `frame.area()` 重排，没有强制 `terminal.clear()`。从 14 行拉到 28 行时，旧单元格（含提示符、CJK）很容易留在新尺寸里。
- 自检跑了两分半，**一个键都没进 crossterm**。和「键盘在 shell / 别的窗口」一致，也和反复失焦一致。

二进制是 `windows_subsystem = "windows"`（避免 daemon/hook 闪黑框），交互启动时 `attach_parent_console()`。stdout 已有句柄则直接返回。从 pwsh 前台敲 `aitrace` 时通常继承控制台、父进程等待，单凭这一条不能推出争用；从 GUI / 任务 / 已有控制台的父进程拉起时，才可能和仍在读同一 console 的进程抢输入。

依赖：`ratatui = "0.29"` + `crossterm = "0.28"`。上游最新是 ratatui **0.30.2**（2026-06-19），默认 crossterm 0.29。0.29 之后合入的多宽字符绘制修复（#1764、#2517 等）不在当前锁里。

### 4. 生态里对得上的同类故障

**备用屏与 raw mode（设计层）**  
Ratatui 文档：fullscreen TUI 必须进 alternate screen，把画幅和 shell 的主缓冲区隔开；raw mode 关掉回显和行缓冲。不用备用屏时，TUI 直接画在提示符所在的那块屏上——与贴图同构。并非所有模拟器都同样实现备用屏。

**Windows 控制台模式（机制层）**  
微软文档：默认 cooked = LINE + ECHO + PROCESSED。ECHO 就是「键入即写回活动屏」。QuickEdit 必须 `ENABLE_EXTENDED_FLAGS` 配着开关；只清 QUICKEDIT 位而丢掉 EXTENDED，位不生效。aitrace 的 `harden_console_input` 按这个契约写。微软终端 **#18406**（仍 open）：Windows Terminal 上 `SetConsoleMode` 去 QuickEdit **不一定挡住鼠标选区**——宿主会忽略应用的选择模式。

**ConPTY 在运行中改写 mode（回归层）**  
微软终端 **#19674**（2025-12，后标 closed，但评论给出 1.6s / 100% 复现）：ANSI 鼠标跟踪 + `SetConsoleMode` + raw stdin + 大量 VT 输出时，ConPTY 会清掉 `ENABLE_MOUSE_INPUT` / `ENABLE_EXTENDED_FLAGS`。业界 workaround 是定时再 `SetConsoleMode`。Node **#61161** 从另一侧说明同一问题：`setRawMode(true)` 整寄存器覆盖而不是读-改-写。aitrace 的硬化是一次性的，主循环不再刷 mode。

**Windows 上「新内容盖旧内容」**  
Claude Code **#19637**（Windows，重叠/乱码，cmd 重、pwsh 轻）和 **#35803**（pwsh + Windows Terminal，compact 后就地覆盖、残渣透出）证明：即使用户没用 aitrace，同一宿主上「两层文本叠在一起」也是高频画面。不能把 aitrace 的叠层全部推给 Claude Code，但说明 **Windows Terminal + pwsh 这条路径本身就会叠字**。

**宽字符**  
Ratatui 0.29 之后仍在修 CJK/emoji 续格、光标和溢出。aitrace 驾驶舱 `truncate(name, 18)` 按 **Unicode 标量个数** 截，再 `{:<18}` 按字符补齐。18 个汉字 = 36 列。OPS 里就有「继续——守恒机检刚逮住我：本轮 消2」。Line 渲染会被 widget `Rect` 裁剪，**不能单凭这一条断言分隔线上的「继续」一定是 dashboard 溢出**；它仍是真实的宽度债，和「用单宽 `─` 覆盖双宽汉字留下鬼影」兼容。

**尺寸/坐标**  
crossterm **#1095**（2026-07 仍 open）：Windows 上窗口在大 screen buffer 里滚过之后，`cursor::position()` 返回缓冲区绝对行而不是窗口相对行。在 conhost 上会让 inline 重绘滚错；Windows Terminal 上不一定走同一条分支，但是「TUI 以为的几何 ≠ 宿主几何」这一类。结合本机 120×14 → 210×28，几何漂移是贴图里残留提示符的合理辅助因。

### 5. 怎样确认你撞上的是哪一条

在**同一窗口、同一宿主**跑，不要另开一个「干净」的 WT 再宣称复现失败：

1. `aitrace --input-test`  
   看：框是否铺满整窗；`after raw` 是否仍含 ECHO/LINE/QUICKEDIT；按 `a` 是否出现在事件列表。若事件列表空白、宿主提示符仍在回显——输入根本没进进程。
2. 对比宿主  
   独立 Windows Terminal 标签（不要分屏） / `conhost`（系统设置里默认终端改 Console Host） / VS Code·Claude Code 集成终端 / rmux 窗格。只有独立 WT 干净、集成终端叠层 → 宿主共写或备用屏未实现。
3. 中文输入法切英文后再试字母键（排除 IME）。
4. 看 `.aitrace/tui-panic.log` 是否在叠层出现的同一分钟有记录。
5. 假空白：时间线非空时预览仍是 logo → 查该 playhead 的 `after_hash` 能否在 `sessions/<id>/snapshots/` 取到。空状态文案不应再说「waiting for edits」。

### 6. 修复方向（按杠杆，不是一张 PR 清单）

已有 Harden / panic restore / `--input-test` 不必推倒。缺口在「启动时有效 ≠ 运行中一直有效」，以及空状态撒谎。

- **运行中守住 console mode**：周期性（或每次 `EnableMouseCapture` / 失焦恢复后）按 `harden_console_input` 再写一遍；自检屏上同时打出当前 mode，避免只在启动打一次。
- **Resize 后全清**：`Event::Resize` 不要只 `continue`；`terminal.clear()` 再画，消灭 14→28 行的旧提示符和 CJK 续格。
- **空状态拆开**：无编辑 / 有编辑但快照缺失 / playhead 越界，三种文案。有轨道还显示 waiting，会让人以为录制坏了。
- **截断按显示宽度**：dashboard / conversation / 文件名一律 `unicode-width`（或 grapheme 宽度），不要 `chars().count()` + `{:<18}`。
- **升级 ratatui 0.30 + crossterm 0.29**：吃到宽字符 diff/光标修复；注意官方警告——两套 crossterm major 会各养一条事件队列和一份 raw-mode 账本。
- **产品层**：启动时若检测到 stdin mode 仍带 ECHO，或 `GetConsoleMode` 失败，直接拒绝进入 cockpit 并打印「请用独立 Windows Terminal」。比叠层之后再猜便宜。
- **文档**：`--input-test` 只存在于 clap，README 的 TUI 注意里应写上这条诊断命令。
- **PE 必须是 CUI**：已落地。不要为了「daemon 不闪框」把 `windows_subsystem` 加回去。

不建议：为这张图重写布局引擎、换 TCP/named pipe。

### 核查项

1. **「回显串进 TUI」** — **成立**。提示符与 `aaaaaaaaa` 叠在空状态快捷键行上，和 README / `run_tui_guarded` 描述的画面一致。
2. **「ECHO 没关掉」** — **说法不一**。自检路径启动后 mode 为 `0x180`（无 ECHO）；主 TUI 那一帧没有同期 mode 转储。`aaaaaaaaa` 也可以是 TUI 已退出后 pwsh 的正常回显。两种都能画出这张图。
3. **「继续」= CJK 溢出** — **未能核实为唯一因**。dashboard 按字符数截 18 是真实缺陷，OPS 文本确实以「继续」开头；分隔线组件本身只画 `─`。更干净的解释是：备用屏/清屏失败后的双宽残格，或上一屏中文被单宽线盖掉一半。
4. **QuickEdit 硬化已盖住这张图** — **不成立（尚未闭环）**。`aaa737c` 已合入，自检证明能清 QUICKEDIT；记忆文件仍写「部署后待复验」。贴图本身不能证明主 TUI 当时跑的是硬化后的二进制，也不能证明 ConPTY 没有在运行中把 EXTENDED 清掉。
5. **`windows_subsystem = "windows"` 是争用源** — **成立**（晚间自检 + 改 CUI 后用户目验）。不是 AttachConsole 失败，是父 shell 根本没在等。

## 事实源

| 类型 | 定位 | 日期 | 对应需求 | 提供了什么 |
|---|---|---|---|---|
| github（本地树） | `README.md` TUI 注意段 | 当前 main | 研究 2、核查 1 | 独占终端；回显串进 TUI；IME 吞字母 |
| github（本地树） | `src/tui/mod.rs` 153–162、230–240 行 | 当前 | 研究 2、6 | raw + harden + 备用屏 + 鼠标捕获的启动/恢复顺序 |
| github（本地树） | `src/tui/widgets/preview.rs` `render_empty_state`、`current_edit` 空 / 无快照 两路 | 当前 | 研究 1 | 假空白的代码路径；快捷键原文 |
| github（本地树） | `src/tui/widgets/dashboard.rs` `truncate` + OPS 渲染 | 当前 | 核查 3 | 按字符数截 18，中文 OPS 会超宽 |
| github（本地树） | `src/tui/event_loop.rs` `HorizontalSep`、`Event::Resize => continue` | 当前 | 研究 1、6 | 分隔线只有 `─`；resize 不清屏 |
| github（本地树） | `src/main.rs` `windows_subsystem`、`attach_parent_console`、`run_tui_guarded` | 当前 | 核查 5 | GUI 子系统 + 按需附加；panic 防死帧 |
| github（本地树） | `src/tui/input_test.rs` | `b21d398` / `aaa737c` 2026-08-28 | 研究 2、5 | 诊断模式与 QuickEdit 硬化实现 |
| github（本地树） | `docs/timeline/2026-08-28.md` 开放待办 | 2026-08-28 | 核查 4 | 自检 0x1f7→0x1f0、QUICKEDIT 残留、`ddd ttt`、待复验 |
| github（本地树） | `.aitrace/input-test.log` | 本机，与贴图同日 | 研究 1、5、核查 2/4 | 硬化成功；120×14→210×28；150s 无 Key |
| github（本地树） | `Cargo.toml` `version = "0.6.5"`, ratatui 0.29, crossterm 0.28 | 当前 | 研究 3 | 与上游 0.30.2 / crossterm 0.29 的版本差 |
| web | https://ratatui.rs/concepts/backends/alternate-screen/ | 文档页（采集日 2026-08-28） | 研究 4 | 不用备用屏就会画在 shell 缓冲上 |
| web | https://ratatui.rs/concepts/backends/ | 同上；写明 0.30.2 默认 crossterm 0.29 | 研究 3、6 | 双 crossterm major 会分裂事件队列和 raw-mode 状态 |
| web | https://learn.microsoft.com/en-us/windows/console/high-level-console-modes | 页面 `updated_at` 2025-08-05 | 研究 4、核查 2 | ECHO/LINE/QUICKEDIT/EXTENDED 契约 |
| web | https://support.microsoft.com/en-us/windows/apps/command-prompt-and-windows-powershell | 采集日 2026-08-28 | 研究 4 | Win11 默认改 Windows Terminal，图形+文本混合应用会不兼容 |
| github | https://github.com/microsoft/terminal/issues/19674 | 2025-12-23 | 研究 4、6 | ConPTY 运行中丢掉 MOUSE/EXTENDED；需周期性 SetConsoleMode |
| github | https://github.com/microsoft/terminal/issues/18406 | 仍 open，更新 2025-01-29 | 研究 4 | WT 上关 QuickEdit 不一定生效 |
| github | https://github.com/crossterm-rs/crossterm/issues/1095 | 2026-07-31 open | 研究 4 | Windows 光标行号在滚动后变成 buffer-absolute |
| github | https://github.com/anthropics/claude-code/issues/19637 | 2026-01-21 open | 研究 4 | Windows cmd/pwsh TUI 重叠乱码 |
| github | https://github.com/anthropics/claude-code/issues/35803 | 2026-03-18，标 duplicate | 研究 4 | WT+pwsh 就地覆盖、残渣透出 |
| github | https://github.com/ratatui/ratatui/pull/1764 | 2025-04-05 | 研究 3、6 | 0.29 之后的多宽字符 `set_stringn` 修复 |
| github | https://github.com/ratatui/ratatui/pull/2517 | 2026-04-29 open | 研究 3 | 后端 draw 循环对宽字符光标跟踪仍在修 |
| github | https://github.com/nodejs/node/issues/61161 | 2025-12-23 | 研究 4 | Windows 上 raw mode 覆盖全部 console 位 |
| github | `gh api repos/ratatui/ratatui/releases/latest` → `ratatui-v0.30.2` | 2026-06-19 | 研究 3 | 当前锁的 0.29 落后一个大版本 |
| x | https://x.com/gclcyhn/status/2093120878951247952 | 2026-08-27 | 研究 4 | 领域讨论：fullscreen/备用屏会牺牲选区和滚动；只作背景，不是 aitrace 证言 |

## 缺口

- **X**：没有检索到 ratatui/crossterm 维护者（joshka 等）就「Windows 提示符叠进 TUI」的一手帖。`x_user_search` 未命中 joshka 的账号。关键字命中的是 ratatui 应用安利，与本需求不对齐，未当证据。
- **GitHub `raystyle/aitrace`**：issues 已关闭，无法在上游 issue 里对这张图做交叉引用。
- **主 TUI 运行时的 console mode**：只有 `--input-test` 日志，没有 cockpit 进程内的 `GetConsoleMode` 采样，所以核查 2 不能从「ECHO 仍开」单点钉死。
- **贴图宿主未标明**：独立 Windows Terminal、VS Code、Claude Code、rmux 四种结论分叉，本轮不能从像素里读出来。
- **微软终端 #19674** 的 GitHub 搜索状态为 closed，但评论里的 1.6s 复现是否随某次 WT 发布修好，本轮没有逐条核对后续 commit，不当成「已修复」。
- 未在用户那台机器上当场重跑 `--input-test` 并对着敲 `a`（只读了已有日志）。日志里没有 Key 行，无法区分「没敲」和「敲了但事件没到」。
