# Claude Code 项目级 Hook / MCP / Skill 配置

研究日期：2026-08-28。只读，未改仓库配置。

## 需求

- 研究：如何给 Claude Code 配 **只对本仓库生效** 的 hook、MCP、skill（文件落点、JSON/目录格式、与用户级/全局的优先级、Windows 路径与 spawn、工作区信任与 `.mcp.json` 审批）。
- 研究：这三套官方机制如何对上本仓库现有实现：`aitrace hook-send`（`src/hook/registration.rs` / `src/hook/send.rs`）、`aitrace mcp`、`skills/aitrace-review.md`。
- 核查：（无独立核查主张）
- 意图：研究。本轮不落地 `.claude/` / `.mcp.json`。

已有锚点：官方文档 `code.claude.com/docs/en/{hooks,hooks-guide,mcp,mcp-quickstart,skills,claude-directory,settings,features-overview}`；本仓库 `D:\aitrace`。

## 结论

### 1. 三件事各自的项目级落点（不要写到 `~/.claude/`）

只在本仓库触发、且可随 git 分发，官方指定的位置是：

| 要配什么 | 项目级文件 | 作用范围 | 是否进 git |
| --- | --- | --- | --- |
| Hook | `.claude/settings.json` 的 `hooks` | 只在含该文件的项目 | 是（团队共享） |
| Hook（本机私有） | `.claude/settings.local.json` 的 `hooks` | 只在本机本项目 | 否（Claude Code 写入时会 gitignore） |
| MCP | 仓库根目录 `.mcp.json`（**不在** `.claude/` 里） | 只在当前项目 | 是 |
| MCP（本机私有、仍只本项目） | `~/.claude.json` 里该项目路径下的 `mcpServers`（`--scope local`，默认） | 只在本机本项目 | 否 |
| Skill | `.claude/skills/<name>/SKILL.md` | 只在本项目 | 是 |

官方文件地图（[claude-directory](https://code.claude.com/docs/en/claude-directory)）把 MCP 标成 **project only**、根目录 `.mcp.json`；hook / skill 标成 **project and global**，靠目录区分。

不要写到这些地方，否则会漏到别的项目，或根本不被读：

- Hook：`~/.claude/settings.json` → 你机器上所有项目。
- MCP：`claude mcp add --scope user` / `~/.claude.json` 顶层 `mcpServers` → 所有项目。
- MCP：`~/.claude/.mcp.json`、`~/.claude/mcp.json`、`%APPDATA%\Claude\mcp.json` → **Claude Code 不读**（[mcp-quickstart 排错](https://code.claude.com/docs/en/mcp-quickstart)）。
- Skill：`~/.claude/skills/<name>/SKILL.md` → 所有项目。

Windows 上 `~/.claude` = `%USERPROFILE%\.claude`，通常 `C:\Users\<you>\.claude`。若设了 `CLAUDE_CONFIG_DIR`，用户级文件改到那个目录。

### 2. Hook：项目级格式、事件键、Windows exec form

官方 schema 是 **按事件名做对象键**，不是顶层数组：

```json
{
  "hooks": {
    "PostToolUse": [
      {
        "matcher": "Write|Edit",
        "hooks": [
          {
            "type": "command",
            "command": "${CLAUDE_PROJECT_DIR}/target/debug/aitrace.exe",
            "args": ["hook-send", "--project", "${CLAUDE_PROJECT_DIR}"]
          }
        ]
      }
    ]
  }
}
```

要点：

- `hooks` 的键是事件名（`PostToolUse`、`PreToolUse`、`SessionStart`…），不是 matcher。
- `matcher` 过滤的是 **工具名**（`Write|Edit`、`Bash`、`mcp__aitrace__.*`），空 / `"*"` 表示该事件每次都跑。
- 有 `args` = **exec form**：直接 spawn 可执行文件，不走 shell。路径含空格、反斜杠时应用 exec form，不必再给路径加引号。
- Windows 上 exec form 的 `command` 必须是真 `.exe`。`npx`/`eslint` 在 `node_modules/.bin` 里的 `.cmd`/`.bat` **不能** exec；要跑它们就用 shell form，或 `command: "node"` + 脚本路径。`aitrace.exe` 满足 exec form。
- 路径占位符：`${CLAUDE_PROJECT_DIR}` 会替换进 `command` 和每个 `args` 元素，并作为环境变量传给子进程。PowerShell **shell form** 写 `${CLAUDE_PROJECT_DIR}` 或 `$env:CLAUDE_PROJECT_DIR`，不要写裸 `$CLAUDE_PROJECT_DIR`（PowerShell 当成未定义变量，解析成 `$null`）。
- 多层来源的 hook **合并执行**，同名 handler 在多个 settings 文件里只跑一次；用户级 hook 不会被项目级覆盖掉。`/hooks` 只读浏览。

对本仓库：项目级、可提交的位置是 `.claude/settings.json`。本机调试、不想提交绝对路径时用 `.claude/settings.local.json`。

### 3. MCP：项目级是根目录 `.mcp.json`，要审批

命令：

```powershell
claude mcp add --scope project -- aitrace mcp
```

或手写仓库根 `.mcp.json`：

```json
{
  "mcpServers": {
    "aitrace": {
      "command": "aitrace",
      "args": ["mcp"]
    }
  }
}
```

stdio 也可显式 `"type": "stdio"`。有 `url` 却没有 `type` 会被当成 stdio 然后跳过。

范围对照：

| `--scope` | 文件 | 谁能用 | 是否只本项目 |
| --- | --- | --- | --- |
| `local`（默认） | `~/.claude.json` → `projects["<绝对路径>"]` | 仅你 | 是 |
| `project` | `<repo>/.mcp.json` | 克隆仓库的人（需各自审批） | 是 |
| `user` | `~/.claude.json` 顶层 `mcpServers` | 仅你，所有项目 | **否** |

同名覆盖：**local > project > user**（整条定义替换，字段不合并）。再下面才是 plugin / claude.ai connector。

审批与信任：

- 交互会话第一次见到 `.mcp.json` 里的 server 会提示批准；`⏸ Pending approval` 时需跑 `claude` 点同意。
- v2.1.196 起：提交进仓库的 `enableAllProjectMcpServers` / `enabledMcpjsonServers` 在 **未信任工作区** 被忽略，克隆仓库不能自我批准。
- `claude -p` / Agent SDK / cloud 会话 **不弹审批框**，会直接加载项目 MCP；要挡就写 `disabledMcpjsonServers` 或 `--setting-sources` 去掉 project。
- 改 `.mcp.json` 后要重启会话。误拒绝可 `claude mcp reset-project-choices`。

`CLAUDE_PROJECT_DIR` 会注入 **MCP 子进程环境**，不在 Claude Code 自己的环境里。若要在 `.mcp.json` 的 `command`/`args` 里用 `${CLAUDE_PROJECT_DIR}`，官方要求带默认值：`${CLAUDE_PROJECT_DIR:-.}`。plugin 配置不需要这个默认。

Windows：`command: "aitrace"` 要求 `aitrace.exe` 在 PATH。debug 构建的绝对路径（如 `D:\aitrace\target\debug\aitrace.exe`）只适合 `settings.local.json` / MCP local scope，不要提交。

### 4. Skill：必须是 `.claude/skills/<目录>/SKILL.md`

项目 skill 目录名就是斜杠命令：`.claude/skills/aitrace-review/SKILL.md` → `/aitrace-review`。frontmatter 的 `name` 在个人/项目 skill 里 **只做展示名**，命令仍取目录名。

加载：

- 从启动目录一路向上到仓库根的 `.claude/skills/`。
- 子目录里的 `.claude/skills/` 在 Claude 读写该子树时才出现；撞名时嵌套 skill 变成 `apps/web:deploy`。
- 会话已启动后编辑现有 `SKILL.md` 会热更新；**新建** 顶层 skills 目录需要重启才能 watch。
- 项目 skill 要先接受 **workspace trust dialog**。

同名覆盖（skill）：**enterprise > personal > project**。因此 `~/.claude/skills/aitrace-review/` 会盖掉仓库里的同名项目 skill。不要把本仓库 skill 拷到用户目录。plugin skill 带命名空间（`/plugin:name`），不冲突。

和 CLAUDE.md 分工：CLAUDE.md 每会话都加载（建议 <200 行）；skill 只加载 description，正文按需。Hook 是确定性脚本；skill 是给模型看的流程。官方组合模式：**MCP 给工具，skill 教怎么用**。

### 5. 优先级总表（三种机制不一样）

来自 [features-overview](https://code.claude.com/docs/en/features-overview) 与各专页：

| 机制 | 同名时 | 含义 |
| --- | --- | --- |
| Settings 标量（model 等） | 高覆盖低 | managed > CLI `--settings` > `.claude/settings.local.json` > `.claude/settings.json` > `~/.claude/settings.json` |
| Settings 列表（permissions.allow） | 合并 | 各层相加 |
| **Hooks** | **合并，都跑** | 用户级 hook 在本项目也会跑 |
| **MCP** | **覆盖** | local > project > user |
| **Skills** | **覆盖** | enterprise > personal > **project**（个人盖项目） |
| CLAUDE.md | 叠加 | 冲突时更具体的通常优先 |

「只对本目录触发」= 文件放项目位置，**并且** 不要在用户级放同名 MCP/skill；hook 即使只写项目文件，用户级已有的 hook 仍会合并进来。

### 6. 信任门：提交进仓库的配置不是立刻生效

- 项目 `.claude/settings.json` 的多数 `permissions.allow`、`env`、marketplace 等要等信任工作区。
- 项目 `.claude/skills/` 同样要信任。
- 项目 `.mcp.json` 还要 **每个 server 单独批准**。
- 未跟踪的 `.claude/settings.local.json` 的 allow 规则 **不等待信任**（文件视为你自己的）；一旦被 git 跟踪，同样走信任门。
- Windows 特例：`settings.local.json` 不一定提到 git 根，可能跟启动目录的 `.claude/settings.json` 放在一起（[settings](https://code.claude.com/docs/en/settings)：「on Windows」与所有权检查）。

云端 / Cowork 会话 **不读** 本机 `~/.claude/settings.json`、`settings.local.json`、`~/.claude/skills/`。要给云会话用，必须提交 `.claude/settings.json`、`.claude/skills/`、`.mcp.json`。

### 7. 对上 aitrace 现状

官方要求 vs 仓库现状：

| 项 | 官方项目级 | 本仓库现在 | 差距 |
| --- | --- | --- | --- |
| Hook 文件 | `.claude/settings.json`（共享）或 `settings.local.json`（本机） | `register_hook` 写 `.claude/settings.local.json` | 本机私有、符合「不漏到别的项目」；**不会**随 git 给别人。生产路径（daemon/init）**没有调用** `register_hook`，只有测试在调。 |
| Hook schema | `{ "hooks": { "PostToolUse": [ { "matcher": "Write\|Edit", "hooks": [...] } ] } }` | `{ "hooks": [ { "matcher": "PostToolUse", "hooks": [...] } ] }` | **键结构不对**：顶层应是对象不是数组；`matcher` 被写成了事件名。按现文档，这种 JSON 不会作为 PostToolUse 组注册。 |
| Hook 命令 | exec form + 真 `.exe` | `command` = `current_exe()`，`args` = `hook-send --project <绝对路径>` | exec form 方向正确，适合 Windows。绝对路径不宜提交；项目共享应改 `${CLAUDE_PROJECT_DIR}`。 |
| Hook 发送 | stdin JSON | `src/hook/send.rs` 读 stdin，转成 `{type:hook,...}` 写 `.aitrace/daemon.sock` | 与官方 stdin 模型一致；daemon 没起来时返回 Ok，不挡会话。 |
| MCP | 根目录 `.mcp.json` | 无此文件。README / skill 只给了通用 `mcpServers` 块 | 未项目级落地。`command: "aitrace"` 依赖 PATH。 |
| Skill | `.claude/skills/aitrace-review/SKILL.md` | `skills/aitrace-review.md`（仓库根单文件） | **Claude Code 不会当项目 skill 发现**。要挪到 `.claude/skills/aitrace-review/SKILL.md`。 |
| README 声明 | — | 「检测到 `.claude/` 时自动注册 PostToolUse」 | 与代码不符：注册函数存在，但未接到 daemon/TUI/init。 |

若下一步做「本仓库 beta、只对本目录」：

1. **Hook（共享）**：`.claude/settings.json`，`PostToolUse` 事件键，matcher `Write|Edit`（或再加 `Bash|PowerShell` 若要覆盖 shell 改文件），exec `aitrace`/`aitrace.exe` + `args: ["hook-send","--project","${CLAUDE_PROJECT_DIR}"]`。
2. **Hook（本机、绝对路径）**：继续用 `settings.local.json`，但先把 schema 改成官方对象键；不要写 `~/.claude/settings.json`。
3. **MCP**：仓库根 `.mcp.json`；Windows 保证 `aitrace.exe` 在 PATH，或 local scope 写绝对路径。首次 `claude` 批准。
4. **Skill**：把 `skills/aitrace-review.md` 放到 `.claude/skills/aitrace-review/SKILL.md`；MCP 块改指向项目 `.mcp.json`，不要教用户写 user scope。
5. **CLAUDE.md**（可选，非本需求三件套）：短规则「回归时用 `/aitrace-review` 和 aitrace MCP」，不要把整份流程塞进每会话上下文。

社区帖里常见错误（与官方不符，配置时避开）：把 MCP 写成 `.claude/mcp.json` / `mcp.json`；把 skill 写成 `.claude/commands/` 单文件却期望目录级 supporting files；以为用户级 skill 不会盖项目 skill。

## 事实源

1. **web** — https://code.claude.com/docs/en/claude-directory — 2026-08-27 — 需求：落点 — 表格：hook → `.claude/settings.json`；skill → `.claude/skills/<name>/SKILL.md`；MCP → 根 `.mcp.json`（project only）；`settings.local.json` gitignored。
2. **web** — https://code.claude.com/docs/en/hooks — 2026-08-27 — 需求：hook 格式 / Windows — 位置表；exec vs shell；Windows `command` 必须是 `.exe`；PowerShell 占位符；matcher 对工具名；hook 合并。
3. **web** — https://code.claude.com/docs/en/hooks-guide — 2026-08-28 — 需求：hook 项目文件 — 项目示例写在 `.claude/settings.json`；`/hooks` 只读；`$CLAUDE_PROJECT_DIR` / exec form。
4. **web** — https://code.claude.com/docs/en/mcp — 2026-08-28 — 需求：MCP 范围 — local/project/user 表；`.mcp.json`；覆盖 local>project>user；`CLAUDE_PROJECT_DIR` 在子进程；`${VAR:-default}`；审批与信任。
5. **web** — https://code.claude.com/docs/en/mcp-quickstart — 2026-08-27 — 需求：MCP 落盘 / Windows — Windows `~/.claude.json` = `%USERPROFILE%\.claude.json`；不读 `~/.claude/.mcp.json` 等；手写 `.mcp.json` 示例。
6. **web** — https://code.claude.com/docs/en/skills — 2026-08-28 — 需求：skill 位置与覆盖 — 项目 `.claude/skills/`；enterprise>personal>project；目录名即命令；trust；热更新。
7. **web** — https://code.claude.com/docs/en/features-overview — 2026-08-21 — 需求：分层 — hooks 合并；MCP 覆盖；skills 覆盖；Skill+MCP 组合。
8. **web** — https://code.claude.com/docs/en/settings — 2026-08-28 — 需求：settings 范围 / 信任 / Windows — 优先级栈；Windows `~/.claude`；`settings.local.json` 在 Windows 的放置例外；列表合并。
9. **web** — https://code.claude.com/docs/en/settings-reference — 2026-08-28 — 需求：hooks 键类型 — `hooks` 是按事件名索引的对象。
10. **x** — 2093103781294780856 (@Arahata0907, 2026-08-27) — 需求：MCP 审批 — CCA-F 题：使用 `.mcp.json` 项目 server 前要批准。与官方一致。
11. **x** — 2035706568142893229 (@akshay_pachaar, 2026-03-22) — 需求：两套 `.claude/` — 仓库内一份、家目录一份。细节以官方为准。
12. **x** — 2037792844916965714 (@AiwithDharmik, 2026-03-28) — 需求：社区结构图 — 把 MCP 画成项目文件；其中 `mcp.json` 路径与官方 `.mcp.json` 不符，当反例。
13. **github** — Automattic/wp-calypso `.mcp.json` (blob `0492cc3e…`) — 需求：落地形状 — 根目录 `mcpServers` + stdio `command`/`args`。
14. **github** — getsentry/sentry-mcp `.mcp.json` (blob `00fb0c54…`) — 需求：落地形状 — 根目录 HTTP `type`+`url`。
15. **github** — wasabeef/claude-code-cookbook `settings.json` (blob `d4a413b7…`) — 需求：hook 形状 — `hooks.PostToolUse` + matcher `Edit|Write|MultiEdit`。
16. **github** — AxiumFoundry/rails8-template `templates/.claude/settings.json.tt` (blob `4948bf8b…`) — 需求：项目路径 — `$CLAUDE_PROJECT_DIR/.claude/hooks/...`。
17. **github（本仓库）** — `src/hook/registration.rs`、`src/hook/send.rs`、`skills/aitrace-review.md`、`README.md` MCP 段 — 需求：对上 aitrace — 见结论第 7 节。

## 缺口

- 未在本机跑 `claude` 验证当前安装版本是否 ≥ 文档所引 v2.1.196（MCP 信任）/ v2.1.198（PowerShell 占位符改写）。文档以 2026-08 官网为准。
- 未实测 Claude Code 对 aitrace **错误 schema**（`hooks` 为数组、`matcher: "PostToolUse"`）是静默忽略还是 Settings Warning。结论按官方 schema 判定为「不会按 PostToolUse 组注册」。
- 未测 Windows 上 `command: "aitrace"`（无 `.exe` 后缀、靠 PATH）与 `command: "D:\\...\\aitrace.exe"` 的 spawn 差异；官方只保证 exec form 需要真可执行文件。
- X 无 Anthropic 官方账号对「项目级三件套」的原发帖；讨论多为结构图/测验，细节以文档为准。
- 未展开 plugin 打包（把 hook+MCP+skill 打成一个 plugin）。需求是项目目录触发，plugin 不是必经。
