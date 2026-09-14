## CI 故障处理手册

### Container 构建出现跨代际 Cargo 错误

如果 `container / container` 报告结构体字段与当前源码不一致（例如在同一
提交中出现 `prompt_used` 缺失），先把它视为缓存污染，而不是立即回滚源码。
旧版流程会把 `/app/backend/target` 通过 `buildkit-cache-dance` 持久化；Docker
`COPY` 的时间戳归一化可能让 Cargo 误判旧的 rmeta 仍然新鲜，导致假红，也可能
把过期二进制带入镜像形成假绿。

#### 现场清创与重跑

1. 在仓库页面打开失败的 Actions run，记录 run ID、提交 SHA 和完整 job 名称。
2. 只删除对应的 `container-cargo-*` 缓存，不要清空 Rust 或 pnpm 的其他缓存：

   ```bash
   gh cache list --repo yuzaicn/codex-proxy-rs --limit 100 \
     --sort created_at --order desc
   gh cache delete <cache-id> --repo yuzaicn/codex-proxy-rs --confirm
   ```

   先核对缓存 key 包含 `container-cargo-`、目标提交对应的 ref，以及异常发生
   时间；删除操作不可恢复。
3. 仅重跑失败的 jobs，保留原始 run 的提交和权限上下文：

   ```bash
   gh run rerun <run-id> --repo yuzaicn/codex-proxy-rs --failed
   ```

4. 把缓存 ID、run ID、错误摘要和重跑结果记录到 issue；不要上传包含凭据的
   `docker compose config` 或 `docker inspect` 输出。

#### 机制性防复发

Container 流程只缓存 Cargo registry/git 下载内容，不再缓存
`backend/target`，Dockerfile 也不再声明 target cache mount。每次容器构建都会
从当前源码重新生成目标文件，避免 Cargo fingerprint 与 Docker COPY 时间戳
组合造成跨源码代际错配；依赖下载缓存仍可复用。

### Main 健康信号

`CI` 在 `main` 的 push 事件上强制运行 backend、frontend 和 container 三类检查，
只有 pull request 才使用路径过滤。看到 main run 中某一类 job 为 `skipped` 时，
先确认运行的是旧 workflow 提交；修复后的 main push 不应把 skipped 当成 passed。

每次合并修复后，检查最近一次 main run 的三个 job 均有实际结论，并在发布前保留
一次手动 rerun 作为审计证据。
