# Phase 03 — 产品视觉身份

**Status**: `completed`
**目标**: 用新的 Codex Usage 图标和一致素材替换上游视觉身份，并验证 Windows 显示质量。
**前置**: Phase 02

## 验收判据

- 新图标为原创派生项目素材，不沿用或仿冒上游及 OpenAI 官方商标。
- EXE、窗口/托盘和 README 使用一致的视觉资产。
- 图标在常用尺寸、深浅任务栏及 100%–200% DPI 下清晰可辨。
- release 构建和实际运行验证通过。

## Tasks

- [x] 制作并评估 Codex Usage 图标候选方案 (`target/icon-candidates/candidates.png`; 选择 A“C 形额度环 + 三条剩余量刻度”，B 易误读为耳机、C 与上游进度条过近)
- [x] 生成 Windows 多尺寸 ICO 与仓库展示素材 (`scripts/generate_icons.py`; `src/icons/icon.ico`: 16/20/24/32/40/48/64/128/256; `.github/codex-usage-icon.png`)
- [x] 接入构建资源、托盘图标和 README (`build.rs` 使用 `src/icons/icon.ico`; `src/tray_icon.rs`: Codex 加载态使用嵌入图标；`README.md`: `.github/codex-usage-icon.png`)
- [x] 验证深浅主题、多 DPI 和实际任务栏渲染 (`target/codex-usage-icon-verification.png`: 16/32/48/128px 深浅背景；ICO 9 sizes；`target/codex-usage-embedded-icon.png`: 从 release EXE 反向提取；PID 9596 运行成功)
- [x] 提交并推送视觉版本 (commit `7ba3e3e`; `git push origin HEAD:main`)

## Notes

- `imagegen` 技能判定现有 SVG/ICO 体系应采用确定性矢量编辑，因此未调用生成式位图；最终资产由 `scripts/generate_icons.py` 可重复生成。
- release EXE 构建成功，891392 bytes，SHA256 `8E48D20AA0175F2EFC4086CC841EFE18BB84957CC85F555D36F5C3886CD7753E`。
