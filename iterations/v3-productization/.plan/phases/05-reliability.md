# Phase 05 — 可靠性强化

**Status**: `completed`
**目标**: 让认证、网络、更新和长期运行失败都具备明确反馈与安全恢复能力。
**前置**: Phase 04

## 验收判据

- 用户能区分凭据缺失/失效、网络失败、限流和服务端错误。
- 更新失败不会破坏当前可运行 EXE，并有可验证的回滚路径。
- 关键迁移、更新与错误分类逻辑有自动化测试。
- 长时间运行未出现明显句柄、GDI 对象或私有内存持续泄漏。

## Tasks

- [x] 梳理并实现认证、网络、限流和服务端错误分类 (`src/poller.rs`: `PollError` categories；`src/window.rs`: 中文“网络/限流/服务/错误”与通用 `NET/429/5XX/ERR`；指数退避分类日志)
- [x] 强化更新下载校验、替换失败恢复和回滚 (`src/updater.rs`: 同 Release `codex-usage.exe.sha256` 流式校验；等待旧进程退出；新进程稳定 2 秒后提交，否则恢复 `.old`)
- [x] 增加迁移、更新和错误处理测试 (`cargo test`: 17 passed；覆盖 HTTP 分类、界面标签、checksum 解析、备份保留、无效 EXE 重启回滚)
- [x] 执行长期运行资源监测并修复发现的问题（10 分钟、10 秒间隔、60 个样本；Private -0.172 MB、Working Set +0.656 MB、Handles +6、GDI 0、USER +1；未发现持续泄漏）
- [x] 完善诊断日志与用户故障排查说明 (`src/main.rs`: version/channel/path；`docs/troubleshooting.md`; `README.md`; 诊断日志敏感标记扫描 0 matches)
- [x] 提交并推送可靠性版本（实现提交 `cf3ba08`）

## Notes

- 资源监测结果保存在 `target/resource-monitor/summary.json` 与 `samples.csv`。进程 PID 34696 在两次 5 分钟自动刷新周期内保持运行；Private 最大 3.227 MB，Working Set 最大 22.086 MB，Handles 最大 327，GDI 恒为 19，USER 最大 14。
- 最终验证：`cargo fmt -- --check`、`cargo test`（17 passed）、`cargo clippy --all-targets`（退出成功，9 条既有风格警告）、`cargo build --release`、`git diff --check` 均通过。
