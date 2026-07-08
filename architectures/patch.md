# Patch Executor 设计说明

## 定位

`agent-executor-patch` 是 `agent-executors` workspace 里的第二个真实执行器。

CLI executor 负责运行命令，patch executor 负责修改文件。它接收一段结构化 patch 文本，解析出新增、更新、删除文件的意图，先做路径和策略校验，再把修改应用到工作目录。

它不是对系统 `patch` 命令的简单包装。它应该保持 Rust 内部解析、校验、应用的执行边界，让 agent 可以拿到稳定的结构化结果。

## 为什么第二个执行器是 Patch

coding agent 最常见的两个真实动作是：

- 运行命令
- 修改文件

第一个动作已经由 `agent-executor-cli` 覆盖。第二个动作如果继续通过 shell 完成，通常会落到 `sed`、`python - <<EOF`、`cat > file`、系统 `patch` 等命令上。这些方式能工作，但边界不够清楚：

- 很难提前知道会改哪些文件
- 很难统一做路径权限校验
- 很难把修改结果结构化记录
- 失败原因容易混在 stdout/stderr 里
- 容易把文件修改语义藏进 shell 脚本

patch executor 把“修改文件”提升成独立执行边界：

```text
patch text + cwd + policy + dry_run
  -> parsed file operations
  -> policy validation
  -> filesystem changes or dry-run result
  -> structured result
```

## 和 Diff 的关系

`diff` 是生成或展示差异，`patch` 是应用差异。

当前不需要单独做 `diff executor`。第一版 patch executor 只负责消费 patch 文本，并提供 dry-run 来预演这段 patch 会修改哪些文件。以后如果 agent 高频需要比较两个文件、两个目录、或者 git worktree，再考虑增加独立的 diff executor。

也就是说当前边界是：

```text
PatchExecutor:
  输入已有 patch
  校验和应用文件修改

Future DiffExecutor:
  输入两个对象
  生成 diff
```

## 当前 API

当前 public API 和其它 executor 保持一致：

```text
Executor::execute(PatchExecutionRequest).await
```

请求包括：

- `patch`：Codex-style patch 文本
- `cwd`：相对路径解析的工作目录
- `dry_run`：是否只校验和预览，不写文件

结果包括：

- `status`：`Applied` / `DryRun` / `Rejected`
- `changed_files`：结构化文件变化列表
- `diagnostics`：诊断信息
- `duration_ms`：执行耗时

当前支持的文件操作：

- `*** Add File: path`
- `*** Update File: path`
- `*** Delete File: path`

## Policy

`PatchPolicy` 当前支持：

- `allowed_cwd_roots`
- `allow_add`
- `allow_update`
- `allow_delete`
- `max_patch_bytes`

路径必须是相对路径，不能使用绝对路径或 `..` 逃出 `cwd`。这条规则属于 patch executor 的底层安全边界，不应该留给上层 agent 自行保证。

## 后续缺口

当前 patch executor 是最小可用版本，还不是完整的编辑系统。

后续可以继续补：

- 更完整的 hunk 上下文匹配
- 保留原文件末尾换行状态
- 文件 move/rename
- dry-run 返回 preview diff
- 更细粒度的 per-path 权限策略
- 应用失败时的部分修改回滚策略
- 和 agent action protocol 对齐 request/result 命名

## 当前原则

- patch executor 只修改文件，不理解任务目标。
- patch 文本是文件修改指令，不是 shell 脚本。
- dry-run 应该验证 patch 能否应用，但不能写文件。
- 路径安全检查属于 executor 边界。
- capsule 映射仍然由 agent/runtime 层负责。
