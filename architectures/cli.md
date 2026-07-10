# CLI Executor 设计说明

## 定位

`agent-executor-cli` 是 `agent-executors` workspace 里的第一个真实执行器。

它的职责不是理解用户目标，也不是替 agent 做决策，而是把一个已经被 agent/cognition/action 层决定好的 CLI 执行请求，转成真实的本地进程执行，并返回结构化结果。

CLI executor 必须保持通用，不依赖 semion，不直接创建 capsule，也不持有 AgentOS 的语义。它只产出稳定的结构化数据，让上层系统自行决定如何保存、路由或展示。

在整个 agent 系统里，CLI executor 应该位于这个位置：

```text
CliWorker cognition
  -> RunAction(cli)
  -> action intent / executor request
  -> agent-executor-cli
  -> executor result
  -> capsule
  -> CliWorker cognition 继续判断
```

也就是说，CLI executor 是一个执行边界，不是一个新的 agent。

## 为什么第一个执行器是 CLI

PC 上大量任务最终都可以转成命令行操作：

- 构建和测试项目
- 读写和检查文件
- 调用系统工具
- 运行脚本
- 调用已有 CLI 程序
- 执行语言工具链命令，例如 `cargo`、`npm`、`python`

所以第一个执行器做 CLI 是最实用的。它可以先覆盖大部分真实工作，再逐步补齐更专业的执行器。

## 两种执行模式

CLI executor 必须明确区分两类输入。

### 裸命令

裸命令是：

```text
program + args
```

例如：

```text
program = "cargo"
args = ["test"]
```

这种模式适合没有 shell 语义的简单命令。它不依赖 shell 展开，也不应该把整段命令字符串交给 shell 解析。

优点：

- 参数边界清楚
- 不需要 shell
- 更容易做安全检查
- 更容易结构化记录

### Shell 脚本

shell 脚本是：

```text
shell + script
```

当前代码里对应的是 `ShellRequest { command, shell }`。`shell` 由外部调用方显式选择，支持：

- `ShellKind::Sh`
- `ShellKind::Zsh`
- `ShellKind::Bash`
- `ShellKind::Cmd`
- `ShellKind::Custom(path)`

默认值在 Unix 上是 `sh`，Windows 上是 `cmd.exe`。如果外部 agent 想使用用户 macOS 环境里的 zsh 语义，可以传 `ShellKind::Zsh`。

这种模式适合需要 shell 语义的场景：

- 管道
- 重定向
- 通配符展开
- 环境变量展开
- 多命令串联
- 条件判断
- 简短脚本逻辑

例如：

```text
cargo fmt --all && cargo test --workspace
```

这类输入不是一个单独命令，而是一段 shell 脚本。

## 当前已经具备的能力

当前 `agent-executor-cli` 已经有一个可用的 async one-shot executor：

- `CommandRequest`
- `ShellRequest`
- `ExecutionStdin`
- `ExecutionOutput`
- `Executor::execute(...).await`
- `CliExecutor`
- `CommandPolicy`
- `CliExecutionRequest`
- `CliExecutionResult`

已经支持：

- program + args
- shell command
- selectable shell: sh / zsh / bash / cmd.exe / custom
- cwd
- env
- stdin text
- stdin bytes
- stdin file
- stdin null
- timeout
- stdout 捕获
- stderr 捕获
- non-zero exit 结构化返回，或按需升级为 error
- timeout 结构化返回
- duration_ms
- stdout/stderr 截断标记
- output size limit
- shell/timeout/program/cwd/env policy
- Unix process group cleanup

这说明它已经可以作为第一版 CLI 运行器的基础。

## 目前不完善的地方

当前代码已经从“命令工具”推进到“低层 CLI executor 基础层”，但还不是完整的 agent 集成层。

主要缺口如下。

### 1. typed request/result 还只是 executor-local 形态

现在已经有 executor-local 的统一请求：

- `CliExecutionRequest::Command(CommandRequest)`
- `CliExecutionRequest::Shell(ShellRequest)`
- `CliExecutionResult`

但它还没有和 agent action protocol 最终命名对齐。后续如果 agent 项目定义了正式 action payload，可以再做一层更靠近协议的命名，例如：

```text
CliExecutionRequest
  kind: Command | Shell
  cwd
  env
  stdin
  timeout
```

结果也应该有统一类型，例如：

```text
CliExecutionResult
  status
  stdout
  stderr
  exit_code
  pid
  duration
  timed_out
```

当前 crate 已经提供足够结构化的数据，agent 侧可以先稳定对接，再决定是否增加 protocol-specific wrapper。

### 2. timeout 已经是结构化结果

当前超时返回：

```text
status = TimedOut
stdout/stderr = 已捕获内容
duration_ms = 实际耗时
```

这符合 agent 循环的需要：timeout 是一次执行结果，CLI worker 可以继续分析，而不是被当成 executor 内部异常。

### 3. non-zero exit 已经适配 agent 循环

当前可以通过 `fail_on_non_zero` 控制 non-zero exit 是 error 还是结构化 output。默认更适合 agent 的方式是：

```text
exit_code != 0 -> ExecutionOutput { status: Failed(code), stderr, stdout }
```

只有进程无法启动、权限错误、runner 自身异常，才算 executor error。

### 4. 已有执行状态枚举

当前 `ExecutionStatus` 已经包括：

- Success
- Failed
- TimedOut
- Started
- Unknown

后续如有需要，可以再增加 Cancelled 或 SpawnFailed；现在 spawn 失败仍然属于 executor error。

### 5. 已有 duration_ms，暂时没有 started/finished 时间戳

one-shot 执行结果已经包含：

- duration_ms

未来也可以增加：

- started_at
- finished_at

这对日志、debugger、timeout 分析和 capsule 记录都有用。

### 6. 已有低层 managed-process backend，尚无 AgentOS session

当前新增：

- `ProcessBackend::start(ProcessRequest)`
- `StartedProcess::control()` / `StartedProcess::recv()`
- `ProcessControl::write_stdin()` / `ProcessControl::stop()`
- `ProcessEvent::Output` / `Exited` / `IoError`

executor 内部 task 持有 child、stdin 和 pipe reader。调用方只拿 control 与 event receiver；
它们不含 task、agent、capsule、日志 retention 或 cursor。后续 `World.ProcessSessions` 才负责
把这些底层事实收敛为可路由的 AgentOS session。

### 7. Unix timeout 已有 process group cleanup，Windows 仍是 direct child

Unix 平台上，执行器会把 child 放入独立进程组。timeout 会先尝试杀进程组，再杀直接 child。

这能覆盖常见 shell 子进程场景，例如：

```text
sh -c "sleep 100 & wait"
```

Windows 当前仍然只处理 direct child。后续如果要支持 Windows 进程树，需要单独做 Job Object 或平台相关实现。

### 8. 已有输出大小限制，但还没有高级截断策略

`CommandPolicy::max_output_bytes` 可以限制 stdout/stderr 各自保留的最大字节数。reader 会继续 drain pipe，避免子进程因为 pipe 不读而阻塞，但只保存限制内的内容，并通过 `stdout_truncated/stderr_truncated` 标记截断。

后续还可以补：

- 保留头部/尾部
- 是否写入临时文件
- capsule 中保存摘要还是完整输出

### 9. 已有基础安全策略层

当前 `CommandPolicy` 已经支持：

- 是否允许 shell
- 最大 timeout
- 最大 output bytes
- program allowlist
- cwd root allowlist
- request env var allowlist

后续可以继续扩展：

- 命令参数规则
- 环境继承策略
- 是否允许网络命令
- 是否允许破坏性命令
- 是否需要用户确认

### 10. 缺少 capsule 映射规范

我们之前讨论过：agent 的信息都是 capsule。

所以 executor 的输入和输出最终也应该能映射为 capsule：

```text
request capsule:
  semantic_type = agent.executor.cli.request

result capsule:
  semantic_type = agent.executor.cli.result
```

当前 crate 不应该直接依赖 semion 或 agent capsule。它需要提供足够结构化的 request/result，让 agent 项目可以稳定封装成 capsule。

换句话说：

```text
agent-executor-cli -> capsule-ready data
agent runtime      -> SemanticCapsule
```

capsule 的 owner、semantic_type、lifecycle、storage policy 都应该由 agent/runtime 层决定，而不是 executor 决定。

## 和 core crate 的关系

当前 workspace 已经有：

```text
crates/
  agent-executor/
  core/
  cli/
```

这个方向是对的。

`agent-executor` 是对外统一 facade crate。它通过 feature re-export 具体执行器，例如：

```toml
agent-executor = { version = "0.1.0", features = ["cli"] }
```

```rust
use agent_executor::{cli::CliExecutor, Executor, Result};
```

这样外部用户未来可以只依赖一个 crate，同时按 feature 选择启用 CLI、browser、file 等 executor。

`core` 应该放所有执行器共享的基础类型，例如：

- 通用 error
- executor trait
- execution id
- status
- timeout 类型
- stdout/stderr 限制配置
- request/result 的公共字段

`cli` 只放 CLI 执行器自己的内容，例如：

- command request
- shell request
- cli-specific result
- process runner
- session request/result

这样未来添加其它执行器时，不会把公共协议散落到 `cli` crate 里。

## 建议的下一步实现顺序

第一步：继续收敛 agent-facing 命名，确认 `CliExecutionRequest` / `CliExecutionResult` 是否就是 agent action protocol 的最终边界。

第二步：在 AgentOS runtime 中设计长任务 session 的统一 request/result API。

第三步：给 session 增加显式 cleanup/forget API，避免长期保留已结束任务。

第四步：增加 session stdout/stderr 增量 cursor 和 write stdin。

第五步：扩展 output policy，支持 head/tail、临时文件或摘要模式。

第六步：agent 项目对接 typed executor API，把 action result 封装成 capsule。

## 当前原则

- executor 只执行，不理解任务目标。
- CLI worker 分析执行结果，不是 executor 分析执行结果。
- 裸命令和 shell 脚本必须区分。
- non-zero exit 是正常执行结果，不应该默认等同内部错误。
- timeout 对 agent 来说也是执行结果。
- 长任务需要 session，而不是简单 background pid。
- capsule 映射由 agent 项目负责，executor 提供稳定结构化数据。
