# CLI Executor 设计说明

## 定位

`agent-executor-cli` 是 `agent-executors` workspace 里的第一个真实执行器。

它的职责不是理解用户目标，也不是替 agent 做决策，而是把一个已经被 agent/cognition/action 层决定好的 CLI 执行请求，转成真实的本地进程执行，并返回结构化结果。

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

当前代码里对应的是 `ShellCmdRequest { command }`，内部在 Unix 上使用 `sh -c`，Windows 上使用 `cmd.exe /c`。

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

当前 `agent-executor-cli` 已经有一个可用的最小 runner：

- `CmdRequest`
- `ShellCmdRequest`
- `CmdStdin`
- `CmdOutput`
- `CmdTool::run`
- `CmdTool::run_shell`

已经支持：

- program + args
- shell command
- cwd
- env
- stdin text
- stdin bytes
- stdin file
- stdin null
- timeout
- stdout 捕获
- stderr 捕获
- non-zero exit 可选择报错或返回
- background 启动并返回 pid

这说明它已经可以作为第一版 CLI 运行器的基础。

## 目前不完善的地方

当前代码还只是“命令工具”，还不是完整的“agent executor”。

主要缺口如下。

### 1. 缺少统一的 typed request/result

现在裸命令和 shell 命令是两个请求类型：

- `CmdRequest`
- `ShellCmdRequest`

后续应该有一个更靠近 agent action protocol 的统一请求，例如：

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

这样 agent 侧可以稳定对接 executor，而不是直接绑定当前低层 runner API。

### 2. timeout 现在是 error，不是结构化结果

当前超时会返回 `Error::tool_timeout()`。

对 agent 来说，超时不是普通内部错误，而是一次执行结果：

```text
status = TimedOut
timed_out = true
stdout/stderr = 已捕获内容
```

后续应该把 timeout 表达成 result status，让 CLI worker 能分析并决定下一步。

### 3. non-zero exit 的语义还不适合 agent 循环

当前可以通过 `fail_on_non_zero` 控制 non-zero exit 是 error 还是 output。

但对 agent 来说，命令失败通常是正常反馈。例如编译错误、测试失败、命令参数错误，都应该回到 cognition 分析，而不是直接变成 executor 内部错误。

后续默认应该倾向：

```text
exit_code != 0 -> CliExecutionResult { status: Failed, stderr, stdout }
```

只有进程无法启动、权限错误、runner 自身异常，才算 executor error。

### 4. 缺少执行状态枚举

当前 `CmdOutput` 只有：

- stdout
- stderr
- exit_code
- pid

缺少明确状态：

- Success
- Failed
- TimedOut
- Started
- Cancelled
- SpawnFailed

没有状态枚举，agent 侧就必须从 exit_code/error 推断，这会让协议变脆。

### 5. 缺少 duration 和 started/finished 时间

执行结果应该能表达耗时。

至少需要：

- duration_ms

未来也可以增加：

- started_at
- finished_at

这对日志、debugger、timeout 分析和 capsule 记录都有用。

### 6. background 只是返回 pid，还不是 session

当前 `background = true` 会启动进程并返回 pid，但没有 session 管理能力。

真正给 agent 使用时，长任务需要一套 session API：

- start session
- read stdout/stderr 增量输出
- write stdin
- stop/kill
- query status

也就是说，background 不能只是“丢出去一个 pid”，否则 agent 后续无法可靠观察和控制它。

### 7. timeout 只 kill direct child

当前文档已经说明：timeout 只杀直接 child process。

但 shell 脚本可能启动子进程，例如：

```text
sh -c "sleep 100 & wait"
```

未来需要考虑 process group 或平台相关的进程树清理，否则 timeout 后可能留下子进程。

### 8. 输出全部收集进内存

当前 stdout/stderr 会完整读入内存。

这对小命令没问题，但 agent 执行真实任务时，输出可能很大。

后续需要：

- 最大输出大小
- stdout/stderr 截断策略
- 保留头部/尾部
- 是否写入临时文件
- capsule 中保存摘要还是完整输出

### 9. 缺少安全策略层

CLI executor 本身可以保持低层，但 agent 调用它之前必须有策略层。

需要考虑：

- 是否允许破坏性命令
- 是否允许访问 cwd 之外的路径
- 是否允许网络命令
- 是否允许修改系统配置
- 是否需要用户确认
- 是否允许环境变量透传

这些策略不一定写在低层 runner 里，但文档和协议要给它留位置。

### 10. 缺少 capsule 映射规范

我们之前讨论过：agent 的信息都是 capsule。

所以 executor 的输入和输出最终也应该能映射为 capsule：

```text
request capsule:
  semantic_type = agent.executor.cli.request

result capsule:
  semantic_type = agent.executor.cli.result
```

当前 crate 不需要直接依赖 semion 或 agent capsule，但需要提供足够结构化的 request/result，让 agent 项目可以稳定封装成 capsule。

## 和 core crate 的关系

当前 workspace 已经有：

```text
crates/
  core/
  cli/
```

这个方向是对的。

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
- session manager

这样未来添加其它执行器时，不会把公共协议散落到 `cli` crate 里。

## 建议的下一步实现顺序

第一步：保留当前 `CmdTool`，在它之上增加 agent-facing 类型。

```text
CliExecutionRequest
CliExecutionResult
CliExecutionStatus
CliExecutor
```

第二步：把 non-zero exit 和 timeout 改成结构化 result。

第三步：增加 duration_ms 和 timed_out。

第四步：加输出大小限制和截断信息。

第五步：再考虑 session，不要一开始就把 session 和普通命令混在一起。

第六步：agent 项目再对接这个 typed executor API，把 action result 封装成 capsule。

## 当前原则

- executor 只执行，不理解任务目标。
- CLI worker 分析执行结果，不是 executor 分析执行结果。
- 裸命令和 shell 脚本必须区分。
- non-zero exit 是正常执行结果，不应该默认等同内部错误。
- timeout 对 agent 来说也是执行结果。
- 长任务需要 session，而不是简单 background pid。
- capsule 映射由 agent 项目负责，executor 提供稳定结构化数据。
