# Agent Executors 架构说明

## 设计目的

`agent-executors` 是给 agent 使用的真实执行器 workspace。

agent runtime 负责判断要做什么，生成结构化的执行请求，并把请求和结果作为数据胶囊保存。这个项目负责真正的执行机制：启动本地进程、运行 shell 脚本、处理超时、收集 stdout/stderr，并返回结构化结果。

第一个执行器是 `cli`，因为本地命令执行是 agent 决策连接真实电脑工作的最小实用入口。

## 边界

agent 项目负责：

- cognition
- planning
- routing
- action intent
- capsule storage

executor 项目负责：

- 执行命令
- 执行 shell 脚本
- 管理 cwd/env/stdin/timeout
- 捕获 stdout/stderr/exit code
- 返回结构化执行结果

也就是说，agent 可以发出：

```text
执行这个 CLI action，参数包括 program、args、cwd、env、stdin、timeout。
```

executor 返回：

```text
进程已经结束，结果包括 stdout、stderr、exit_code、pid 和状态信息。
```

executor 不判断用户任务是否完成，不创建 worker，不路由消息，不写 memory，也不解释业务目标。这些判断都留在 agent 的 cognition 循环里。

## 通用执行系统

`agent-executors` 应该设计成通用执行系统，而不是 AgentOS 的内部模块。

因此它不应该知道：

- agent
- cognition
- action
- worker
- semion
- capsule

它只应该知道：

- execution request
- execution result
- executor trait
- executor status
- executor error

这样同一个 executor 可以被不同上层系统复用：

- AgentOS 可以把 request/result 封装成 capsule。
- 普通 CLI app 可以直接打印 result。
- Web service 可以把 result 转成 HTTP response。
- 测试工具可以直接用 result 做断言。

executor 输出应该是 capsule-ready data，而不是 capsule 本身。capsule 封装属于 agent adapter/runtime 层。

## Workspace 形态

这个 workspace 为多个执行器预留空间：

```text
agent-executors/
  crates/
    agent-executor/
    core/
    cli/
```

当前对外 facade crate 是 `agent-executor`，基础 crate 是 `agent-executor-core`，第一个执行器 crate 是 `agent-executor-cli`。

`agent-executor-core` 负责放置跨执行器共享的稳定基础类型，比如 workspace 级 `Error` 和 `Result`。具体执行器可以继续隐藏自己的内部错误细节，并通过 core 提供的公共错误入口返回给调用方。

`agent-executor` 负责作为外部统一入口，通过 feature 选择启用哪些执行器。例如外部只需要 CLI executor 时，可以依赖：

```toml
agent-executor = { version = "0.1.0", features = ["cli"] }
```

然后使用：

```rust
use agent_executor::{cli::CliExecutor, Result};
```

各 executor crate 仍然保持独立发布和独立使用能力。如果外部只想要最小 CLI 依赖，也可以直接依赖 `agent-executor-cli`。

未来如果增加其它执行器，应该继续新增 crate，而不是把 `cli` crate 变成混合工具集合。

## CLI Executor 目标

CLI executor 提供两种命令模式：

- 裸命令：`program` + `args`
- shell 脚本：把脚本文本交给系统 shell 执行

两种模式都应该支持：

- 工作目录
- 环境变量
- stdin
- timeout
- stdout/stderr 捕获
- non-zero exit 处理

之所以需要 shell 模式，是因为很多本地操作并不是一个单独命令，而是一段 shell 脚本。管道、重定向、通配符展开、环境变量展开、条件执行、多命令串联，都属于 shell 模式。

CLI crate 内部按层组织：

- `types`：请求、输出、执行状态等稳定数据结构
- `runner`：同步命令执行入口
- `process`：底层进程配置、stdin/stdout/stderr、wait/timeout 处理
- `shell`：平台相关 shell 命令构造
- `policy`：低层执行安全策略入口

one-shot 执行结果使用结构化状态表达：

- `Success`：命令退出码为 0
- `Failed(code)`：命令已经结束，但退出码非 0
- `TimedOut`：命令超过 timeout 后被终止
- `Started`：后台命令已启动

`fail_on_non_zero` 只决定非 0 退出码是否升级成 `Error`；即使不升级，调用方仍然可以从 `ExecutionOutput.status` 读到 `Failed(code)`。timeout 不再作为普通执行失败抛出，而是返回 `TimedOut` 状态，真正的 `Error` 留给 spawn/io/policy 这类执行器自身失败。

当前 public API 只保留一个执行入口：`CliExecutor::execute(...).await`。长任务 session 暂时不进入 public API；如果后续要支持可观察长任务，应该先设计统一的 session request/result，再决定是否增加第二条清晰边界，而不是把多个零散方法直接暴露出去。

安全策略在 `CommandPolicy` 里提供统一入口，包括是否允许 shell、是否允许后台任务、最大 timeout、可执行程序 allowlist、cwd root allowlist、以及请求级 env var allowlist。以后继续扩展更细粒度限制时，应该放在 policy 层，而不是混进 process runner。

在 Unix 平台上，执行器会尽量把启动的进程放进独立进程组；timeout 会先尝试杀掉整个进程组，再杀直接 child。这能覆盖常见 shell 子进程清理场景。Windows 目前仍只处理直接 child。

## 当前阶段

这个项目目前处在第一个执行器阶段。

当前目标不是在这里实现所有 policy、capsule 集成或 agent 流程，而是先保持一个干净的本地进程 runner。之后 agent 项目可以通过稳定的执行边界调用它。

后续重要工作：

- 和 agent action protocol 对齐 typed request/result 命名
- 设计 executor input/output 如何映射到 capsule
- 扩展 policy 层，加入命令参数规则、环境继承策略等更细粒度限制
- 重新设计长任务 session 的统一 request/result 边界
