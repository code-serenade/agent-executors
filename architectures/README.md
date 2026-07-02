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

## 当前阶段

这个项目目前处在第一个执行器阶段。

当前目标不是在这里实现所有 policy、capsule 集成或 agent 流程，而是先保持一个干净的本地进程 runner。之后 agent 项目可以通过稳定的执行边界调用它。

后续重要工作：

- 引入和 agent action protocol 对齐的 typed request/result
- 把 timeout 和 non-zero exit 表达为结构化执行结果
- 设计长任务 session 的启动、查询和停止
- 设计 executor input/output 如何映射到 capsule
- 在低层进程 runner 之外增加安全策略
