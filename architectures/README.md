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

## Workspace 形态

这个 workspace 为多个执行器预留空间：

```text
agent-executors/
  crates/
    core/
    cli/
```

当前基础 crate 是 `agent-executor-core`，第一个执行器 crate 是 `agent-executor-cli`。

`agent-executor-core` 负责放置跨执行器共享的稳定基础类型，比如 workspace 级 `Error` 和 `Result`。具体执行器可以继续隐藏自己的内部错误细节，并通过 core 提供的公共错误入口返回给调用方。

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
- `session`：长任务 session 的启动、查询和停止
- `policy`：低层执行安全策略入口

同步执行结果使用结构化状态表达：

- `Success`：命令退出码为 0
- `Failed(code)`：命令已经结束，但退出码非 0
- `TimedOut`：命令超过 timeout 后被终止
- `Started`：后台命令或 session 已启动

`fail_on_non_zero` 只决定非 0 退出码是否升级成 `Error`；即使不升级，调用方仍然可以从 `CmdOutput.status` 读到 `Failed(code)`。timeout 不再作为普通执行失败抛出，而是返回 `TimedOut` 状态，真正的 `Error` 留给 spawn/io/policy 这类执行器自身失败。

长任务使用 `CmdSessionManager` 管理。它负责启动命令、返回 session id/pid、查询是否仍在运行、以及停止进程。session 是 executor 内部运行态，不承担 agent capsule 写入；agent 项目可以把 `CmdOutput` 或 `CmdSessionStatus` 映射成自己的 capsule 数据。

安全策略目前在 `CommandPolicy` 里提供最小入口，包括是否允许 shell、是否允许后台任务、以及最大 timeout。更高层的 allowlist、cwd 限制、环境变量过滤等策略以后应该继续扩展在 policy 层，而不是混进 process runner。

## 当前阶段

这个项目目前处在第一个执行器阶段。

当前目标不是在这里实现所有 policy、capsule 集成或 agent 流程，而是先保持一个干净的本地进程 runner。之后 agent 项目可以通过稳定的执行边界调用它。

后续重要工作：

- 和 agent action protocol 对齐 typed request/result 命名
- 设计 executor input/output 如何映射到 capsule
- 扩展 policy 层，加入 cwd/env/command allowlist 等更细粒度限制
