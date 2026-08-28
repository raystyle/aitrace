---
name: aitrace
description: >
  基于 aitrace 编辑历史的自纠工作流。测试失败、AI 辅助编辑后行为回归、
  或用户要求 bisect / 回放 / 检查本次会话改动了什么时使用。
allowed-tools: Read Grep Glob Bash(cargo test *) Bash(cargo build *) mcp__aitrace__list_sessions mcp__aitrace__get_timeline mcp__aitrace__get_frame mcp__aitrace__diff_frames mcp__aitrace__search_edits mcp__aitrace__get_regression_window mcp__aitrace__subscribe_edits
---

# aitrace 自纠工作流

测试失败或一系列 AI 辅助编辑后行为回归时使用本 skill。通过 MCP 拖动 aitrace 时间线，在回归源头做外科手术式修复。

项目 MCP 已配置在 `.mcp.json`（服务器名 `aitrace`，`aitrace mcp`）。**不要**添加用户级 MCP 服务器。daemon 必须在录制状态（`aitrace daemon status`）。

## 受测二进制的验证

每次验收 / 回归都必须写明测试的是哪个构建（git 短哈希或 crate 版本），并确认运行中的进程就是该构建：

1. **构建前先停 daemon**——daemon 子进程从 `target\debug\aitrace.exe` 运行，锁住链接器输出（否则 os error 5）
2. `target\debug\aitrace.exe daemon start` 把新 exe 装进 `.aitrace/bin/`（目标被锁时自动改名 `aitrace.exe.old` 让路，常驻 MCP server 不会阻塞更新）
3. **MCP server 进程不会自动升级**——它持续运行旧二进制直到重连。`initialize` 回报 `serverInfo.version`（crate 版本），用 `/mcp` 查看；版本过期则**提醒人类重连 MCP（或重启 Claude Code）**，之后再验收 MCP 侧改动
4. 报告必须写明哈希 + 版本号

## 工作流

### 阶段 1：加载上下文

1. `list_sessions` 找到活跃或最近的会话
2. `get_timeline` 拉取完整编辑历史
3. 记录编辑总数、触及的文件、编辑区间

### 阶段 2：界定范围

1. 按 `operation_id` 分组（`session:tool_use_id`，每次工具调用一组；批量并行编辑共享同一意图）
2. 按文件分组看哪个文件改动最多
3. 确定"之前"状态（第 1 帧或当前工作的起点）
4. 帧上的 `operation_intent`（assistant 声明的意图）和 `intent`（用户请求）直接说明每次编辑想干什么

### 阶段 3：运行验证

1. 在仓库根运行 `cargo test`（先停 daemon，见上文）
2. 全部通过 → 报告成功并停止
3. 有失败 → 记录具体错误与失败用例

### 阶段 4：二分定位回归

1. `get_regression_window` 配合文件过滤缩小候选帧
2. 在候选帧上二分：
   a. 取中点帧
   b. `get_frame` 查看该点文件状态
   c. `diff_frames` 对比中点与已知良好状态
   d. 判断引入回归的改动在此点之前还是之后
   e. 收缩窗口，重复
3. 锁定引入问题的帧后，`diff_frames` 该帧与前一帧，看精确改动

### 阶段 5：外科手术式修复

1. `get_frame` 回归前一帧，看本来的状态
2. 结合 `operation_intent` 理解那次编辑想做什么
3. 写**保留原意图、只纠正错误**的最小修复
4. **不要**整段回滚编辑——修具体问题

### 阶段 6：验证修复

1. 重跑 `cargo test` 确认回归已修
2. 再跑一次 `get_timeline` 确认修复被记录
3. 报告：发现了什么、哪一帧引入、修了什么

### 阶段 7：沉淀时间线记忆 + patch/diff 自省（每轮工作收尾）

1. `get_timeline` 拉当前会话，按 `operation_id` 分组操作，读取每组的 `operation_intent`（想干什么）与 `intent`（用户要求）
2. 汇总文件修改分析：编辑次数、±行数、热点文件
3. **patch/diff 自省**——用 `search_edits` / `diff_frames` 审视自己的实际改动：
   - **意图↔diff 一致性**：diff 内容是否超出意图声明的范围（夹带私货）
   - **意外文件**：patch 里有无草稿 / 临时 / 不该提交的文件（如验收残留）
   - **重做检测**：同文件被反复编辑（≥3 轮）通常意味着设计摇摆或修复不收敛，标记进待办
   - **最小化**：改动行数与问题规模是否相称，有无顺手重构
   异常项当场修正或写入待办
4. 写入 `docs/timeline/<YYYY-MM-DD>.md`（同日追加，中文），含五节：**今日轨迹**（会话→提交的叙事）、**操作意图时间线**、**文件修改分析**、**经验教训 → 代码与 SKILL 改进项**、**待办**
5. **硬性检查：待办必须非空**——五原则走练若没产生任何新待办，视为无效走练；此时把"为什么没找到改进项"本身写成待办（反思自省的覆盖盲区），并至少完成一项旧待办或明确推进
6. 教训若指向流程或工具缺陷，**同步修订本 SKILL 或 CLAUDE.md**——完成"记录 → 改进"闭环，而不是只记不用
7. 新会话开工先读最新的记忆文件，继承上下文与待办

## 提示

- `search_edits` 用 regex 找触及某函数/变量的历史帧
- 多个回归逐个修，每修一个重跑测试
- `subscribe_edits` 订阅实时编辑通知
- `get_timeline` 的 `file_filter` 收窄到单文件
