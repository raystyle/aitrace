# aitrace 的 Windows 支持：最稳库、最少改动

调研日期：2026-08-28。对象是本仓库 `aitrace` 如何在 Windows 上编过、跑过 daemon / hook / MCP subscribe，约束是**稳定库 + 最少代码**。

## 需求

- 研究：
  1. 当前源码里哪些 Unix API 挡住 Windows（范围只到能编过、daemon 能启停、hook 能通知、MCP 能连 socket）
  2. IPC（`UnixStream` / `UnixListener` / `daemon.sock`）用哪套库最稳、改动最小
  3. 进程探活 / 停 daemon（`libc::kill`）用哪套库最稳、改动最小
  4. Claude Code `PostToolUse` hook 在 Windows 上怎么注册（现实现是 `nc -U`）
  5. 落到本仓库的最少改动方案：依赖、`cfg` 切分、文件级工作量、不要做什么
- 核查：（无用户给出的原子主张）

意图：研究。锚点：本仓库 `src/daemon/`、`src/mcp/streaming.rs`、`src/hook/registration.rs`；Windows 10+ `AF_UNIX`；crates `uds_windows`、`interprocess`、`windows-sys`。

## 结论

### 1. 挡住 Windows 的只有三处 IPC / 进程 API

TUI（ratatui / crossterm）、文件系统监视（notify）、snapshot 都是跨平台的。本机 Windows `cargo test` 的 7 个编译错误全部来自下面三类，没有别的。

| 能力 | 现在 | 文件 |
| --- | --- | --- |
| daemon 监听 / 客户端连接 | `std::os::unix::net::{UnixListener, UnixStream}` | `src/daemon/hook_listener.rs`、`src/daemon/mod.rs`、`src/mcp/streaming.rs` |
| PID 探活 | `libc::kill(pid, 0)` | `src/daemon/pid.rs` |
| 停 daemon | `libc::kill(pid, SIGTERM)` | `src/daemon/mod.rs` |
| Claude hook 把 JSON 打进 socket | `echo … \| nc -U <path>` | `src/hook/registration.rs` |

协议本身只是一行 JSON 写进流，不传 fd、不用 datagram。这决定了下面选库可以走「API 尽量长得像 UnixStream」而不是重写协议。

`CONTRIBUTING.md` 写 macOS or Linux；CI 只有 `ubuntu-latest` 和 `macos-latest`。

### 2. IPC：最少代码选 `uds_windows`，不要等 std、不要先上 `interprocess`

**Windows 内核已经有 `AF_UNIX` SOCK_STREAM**（Win10 17063 / 1809 起，驱动 `afunix.sys`）。官方说明：路径走 UTF-8 文件系统、`bind` 会留下 NTFS reparse 的 socket 文件、重 bind 前必须 `DeleteFile`；**不支持 SOCK_DGRAM、SCM_RIGHTS、socketpair**。本仓库只用 SOCK_STREAM 字节流，对得上。

**Rust std 还不能当稳定方案。** `std::os::windows::net::UnixStream` 在 1.97 文档里是 nightly feature `windows_unix_domain_sockets`（issue #150487 / 历史 #56533）。本仓库 `rust-toolchain.toml` 钉 stable，不能开 nightly。libs 从 2018 拖到 2026 仍未稳定。

**最少改动的稳定库：`uds_windows` 1.2.1**（`haraldh/rust_uds_windows`）。

- 从 Azure `mio-uds-windows` 分出来，API 刻意对齐 `std::os::unix::net`
- crates.io 累计约 4100 万次下载；1.2.1 于 2026-03-14 发布，要求 Rust 1.85（与本仓库 edition 2024 一致）
- smol 官方 example 用它在 Windows 上模拟 Unix socket
- 改动量大约是一层类型别名：

```rust
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};
#[cfg(windows)]
use uds_windows::{UnixListener, UnixStream};
```

`Cargo.toml` 用 target 依赖，Unix 不加这个 crate：

```toml
[target.'cfg(windows)'.dependencies]
uds_windows = "1.2"
```

**不要用 `interprocess` 做第一刀。** 它 2.4.3（2026-08-01）、Windows+Linux+macOS 都有 CI，593★，跨平台 `local_socket` 在 Windows 上走 **named pipe** 而不是 `AF_UNIX`。协议 ident、bind/connect API 和现有 `UnixStream` 不同，daemon / hook_listener / MCP 都要换调用，代码量明显大于类型别名。作者明确：要等 std 的 Windows `AF_UNIX` 才考虑在 Windows 上改用 Unix socket。适合以后若 `uds_windows` 在路径长度 / reparse 上踩坑再换，不适合「最少代码」。

**不要用 TCP `127.0.0.1`。** 零新依赖，但要改寻址（端口文件 vs `daemon.sock`）、防火墙上可能弹窗、和现路径语义不一致，省不了多少代码。

Windows `AF_UNIX` 落地时记住两件微软文档里的事：

1. 重 bind 前删掉旧 socket 文件（`DeleteFile` / `std::fs::remove_file`）。现有 `cleanup_stale` 已经在删 `sock_path`，方向对。
2. `std::fs::metadata` / `exists` 对 Windows 上的 Unix socket reparse 会报「系统无法访问该文件」（rust-lang/rust#109106）。探活不要用 `path.exists()` 当「socket 是否有效」，以 connect 成败为准。

### 3. 进程探活 / 停 daemon：`windows-sys` 三十行，不要 `libc::kill`、不要 `sysinfo`

`libc` 维护者明确拒绝在 Windows 上提供 POSIX `kill`（rust-lang/libc#3764，2024-08）：libc 不是 POSIX 兼容层。本仓库现依赖的 `libc::kill` **在 Windows 目标上就不存在**，这就是编译错误来源，不是漏写 `cfg`。

Windows 对应关系（微软文档 + rust-lang/rustwide 实现）：

| Unix | Windows |
| --- | --- |
| `kill(pid, 0)` 探活 | `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` + `GetExitCodeProcess`；退出码 `STILL_ACTIVE`（259）表示还在跑 |
| `kill(pid, SIGTERM)` | `OpenProcess(PROCESS_TERMINATE)` + `TerminateProcess`（没有 Unix 那种可捕获的 TERM，是硬杀） |

**库选 `windows-sys`（Microsoft `windows-rs`，约 1.27 万★，crates.io 默认 0.61.2）。** rustwide 自己的 Windows `kill_process` 就是这套。不要 `winapi`（旧）、不要 `sysinfo`（为两个函数拖整棵进程枚举）。

`Cargo.toml`：

```toml
[target.'cfg(unix)'.dependencies]
libc = "0.2"

[target.'cfg(windows)'.dependencies]
uds_windows = "1.2"
windows-sys = { version = "0.61", features = ["Win32_Foundation", "Win32_System_Threading"] }
```

现有无条件 `libc = "0.2"` 改成 unix-only。`pid.rs` 用 `#[cfg(unix)]` / `#[cfg(windows)]` 两个 `is_process_alive`，对外签名保持 `pid: i32` 或顺手改 `u32`（Windows PID 是 `u32`；现 PID 文件已是十进制整数，兼容）。

硬杀语义和 SIGTERM 不同：daemon 现在先尝试 socket 优雅停，5 秒后再 `kill`。Windows 上保留「先连 socket 发停、超时再 TerminateProcess」即可，不必模拟信号。

### 4. Hook：不要移植 `nc -U`，改成 exec 形式调 `aitrace` 自己

`nc -U` 是 Unix netcat 的 Unix-socket 模式，Windows 没有对等命令。Claude 官方 hooks 文档（2026-08）写明：

- Windows 上 command hook 默认 Git Bash；没装 Git Bash 才落到 PowerShell
- 可设 `"shell": "powershell"`
- **有 `args` 就是 exec 形式**：`command` 是可执行文件，`args` 逐个传入，**不经过 shell**。官方推荐路径占位符用这种形式，避免空格和引号

最少、也最稳的做法：加一个很小的隐藏子命令（或现有内部通道），例如 `aitrace hook-send`，从 stdin / 环境读 JSON，连 `daemon.sock` 写一行。注册：

```json
{
  "type": "command",
  "command": "C:\\abs\\path\\aitrace.exe",
  "args": ["hook-send"],
  "description": "aitrace edit tracking"
}
```

Unix 也可以改成同一条，从而删掉 `echo | nc -U` 和 `$$` / `$TOOL_NAME` 的 shell 拼装。这是**减少**平台分支，不是增加。

不要用 PowerShell `NamedPipeClientStream` 或手工 `AF_UNIX` Socket 一行脚本：长、难测、和 daemon 的 `uds_windows` 路径还要对齐。官方文档也警告 Windows 上 hook 走 bash 时 stdin 曾出现过不是 pipe 的 bug（anthropics/claude-code#36156）；exec 调自己的 exe 不经过 bash。

### 5. 最少改动落地（建议按这个顺序，不扩范围）

只动编译失败的模块 + hook 注册 + CI。不改 TUI、不改分析引擎、不加 async runtime。

1. **Cargo.toml**：`libc` 改为 unix target；windows 加 `uds_windows`、`windows-sys`（上述 features）。
2. **一层 IPC 别名**（新建 `src/ipc.rs` 约 10 行，或在 `hook_listener.rs` 顶上 cfg use）。`hook_listener.rs` / `daemon/mod.rs` / `mcp/streaming.rs` 把 `std::os::unix::net::…` 换成这个别名。
3. **`pid.rs`**：windows 分支 `OpenProcess` + `GetExitCodeProcess` + `CloseHandle`；unix 保持 `libc::kill(pid, 0)`。
4. **`daemon/mod.rs` 停进程**：unix 仍 SIGTERM；windows `TerminateProcess`。优雅停的 socket 路径共用。
5. **hook**：Windows（建议连 Unix 一起）注册 exec `aitrace hook-send`；实现就是现有 `UnixStream::connect` + 写 JSON。
6. **CI**：`.github/workflows/ci.yml` 加 `windows-latest` 的 `cargo test`（与 macos 一样即可）。
7. **文档**：`CONTRIBUTING.md` 的「macOS or Linux」改成含 Windows；README 写清 Win10 1809+（`AF_UNIX` 下限）。

大约新增：一个 10 行模块、一个 ~40 行 windows pid 分支、一个 ~30 行 hook-send、几处 import。**不要**同时引入 named pipe、TCP、tokio、`interprocess`。

刻意不做：

- nightly `windows_unix_domain_sockets`
- 为 Windows 单独写 named pipe 协议
- 把 `libc` 当跨平台进程库
- 用 `sysinfo` / `taskkill` 停 daemon

## 事实源

1. **github / 本地** · `src/daemon/hook_listener.rs`、`src/daemon/mod.rs`、`src/daemon/pid.rs`、`src/mcp/streaming.rs`、`src/hook/registration.rs` · 需求 1 · Unix socket、`libc::kill`、`nc -U` 是仅有的 Windows 编译阻断。
2. **web** · https://devblogs.microsoft.com/commandline/af_unix-comes-to-windows/ （Sunil Muthuswamy / WSL，Win10 17063）· 需求 2 · Windows 原生 `AF_UNIX` SOCK_STREAM；无 datagram / SCM_RIGHTS；重 bind 前删 socket 文件。
3. **web** · https://doc.rust-lang.org/std/os/windows/net/struct.UnixStream.html （std 1.97.1，feature `windows_unix_domain_sockets` #150487）· 需求 2 · std Windows UnixStream 仍是 nightly，Win10 17063+。
4. **github** · rust-lang/rust#147335 · 需求 2 · 把 Windows `UnixStream` 送进 std，仍 gated unstable。
5. **web** · https://crates.io/api/v1/crates/uds_windows · 需求 2 · 1.2.1（2026-03-14），下载约 4.1e7，Rust 1.85，仓库 haraldh/rust_uds_windows。
6. **github** · haraldh/rust_uds_windows（40★，pushed 2026-03-14）· 需求 2 · 稳定 crate 本体。
7. **github** · smol-rs/smol `examples/windows-uds.rs` · 需求 2 · 工程上用 `uds_windows` 在 Windows 模拟 Unix socket 的既有做法。
8. **web** · https://crates.io/api/v1/crates/interprocess · 需求 2 · 2.4.3（2026-08-01），约 1.35e7 下载。
9. **github** · kotauskas/interprocess README + docs.rs `local_socket` · 需求 2 · Windows 用 named pipe 实现 local socket；2.0 起不再自带 UDS；等 std Windows AF_UNIX。
10. **github** · rust-lang/rust#109106（2023-03）· 需求 2、5 · Windows 上 Unix socket 的 `exists`/`metadata` 会失败，是 reparse point。
11. **github** · rust-lang/libc#3764（joshtriplett，2024-08）· 需求 3 · 拒绝在 Windows libc 上提供 POSIX `kill`。
12. **web** · https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-getexitcodeprocess · 需求 3 · `STILL_ACTIVE` 表示进程未退出。
13. **github** · rust-lang/rustwide `src/native/windows.rs` · 需求 3 · `windows-sys` 的 `OpenProcess` + `TerminateProcess` 参考实现。
14. **web** · https://crates.io/api/v1/crates/windows-sys · 需求 3 · Microsoft 官方绑定，默认 0.61.2。
15. **github** · microsoft/windows-rs（12708★）· 需求 3 · `windows-sys` 上游。
16. **web** · https://code.claude.com/docs/en/hooks · 需求 4 · Windows hook 可用 PowerShell；`args` 为 exec 形式、不经 shell。
17. **github** · anthropics/claude-code#36156（2026-03）· 需求 4 · Windows 上 hook 走 bash 时 stdin 可能不是 pipe。
18. **本地** · `CONTRIBUTING.md`、`.github/workflows/ci.yml` · 需求 1、5 · 文档与 CI 均无 Windows。

## 缺口

- **X**：关键字 `uds_windows` / `AF_UNIX Windows Rust` 无可用讨论；本轮领域讨论只来自 GitHub issue / libs 帖，不从来自 X。
- **未在本机实测 `uds_windows` bind `.aitrace/daemon.sock`**：结论来自 crate API 与微软文档，不是本仓库的集成测试。技能要求不跑命中里的代码。
- **std Windows UnixStream 何时稳定**：#150487 仍是 unstable，无发布日期。
- **Claude Code Windows 是否保证 hook exec 形式把 JSON 送到 stdin**：官方文档说 command hook 从 stdin 读 JSON；#36156 只针对 bash/PTY。exec `aitrace.exe` 是否同样稳定，未单独核实。
- **路径长度**：Windows `sockaddr_un.sun_path` 约 108 字节；项目路径极深时 `daemon.sock` 可能超限。本轮未测，若发生应改短路径或再评估 named pipe / `interprocess`。
