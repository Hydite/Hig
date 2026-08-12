# Hig 桌面使用指南

本文档覆盖 v1.9.6 发布候选桌面 App。macOS 构建已签名，但未执行 notarization。

## 创建归档

1. 打开 **Create Archive**。
2. 选择输入目录和输出 `.hig` 路径。
3. 安全归档建议保持默认 **Balanced + Password**。
4. 只在启动任务时输入密码。Hig 不保存密码。
5. 只有在兼容或诊断需要时才调整高级设置：
   - HIGV1/HIGV2 和 compact/legacy manifest。
   - 显式 zstd level 或 worker 数。
   - cache 目录、project mode、solid mode、batch 和 chunk 阈值。
6. 私密数据不要使用 **No encryption**。该模式不提供保密性或 AEAD 认证加密。
7. 只有接受 metadata/sealed-cache 风险提示时才使用 **Fastest**。

## 打开归档

1. 打开 **Open Archive**。
2. 选择 `.hig` 文件。
3. 如果归档已加密，输入密码。
4. 点击 **Inspect** 认证并读取 manifest。
5. 选择输出目录并解压。
6. 只有确认已有文件可以被替换时才开启 overwrite。

## Projects

1. 在 **Projects** 页面初始化项目目录。
2. Hig 会创建 `.hig/project.json`，并让 daemon 监听目录。
3. Ready 表示 daemon 已经持有已验证快照。
4. Dirty 或 Invalid 表示 Hig 会根据 project mode 进行 rebuild、fallback 或失败。
5. Rebuild 会作为 daemon task 进入 **Tasks** 页面。

## Runtime

在 **Runtime** 页面启动、重启或停止与 cache 绑定的 daemon。Session unlock 只把派生 key 保存在 daemon 内存中，到 TTL 后失效。重启 daemon 会清除 session。

## Tasks

pack、unpack、project rebuild、cache GC、cache compact 和 diagnostics 都作为 daemon task 管理。只要 daemon 还保留结果，App 重启后仍可恢复 completed、failed、cancelled 任务记录。

## Cache

在 **Cache** 页面查看 cache 大小、journal 状态和 compact 建议。GC/compact 会先执行 dry-run，确认后才作为 daemon task 执行。

## Diagnostics

Diagnostics 通过受限 sidecar 调用已有 CLI benchmark harness。benchmark 密码通过受控子进程环境传递，不保存到 settings、task history 或日志。

## 排错

- **Daemon unavailable**：在 Runtime 启动 daemon，或重启后重试。
- **No session**：解锁 session，或使用密码归档任务。
- **Wrong password**：归档无法认证，不应写出可信输出。
- **Environment not qualified**：benchmark 卷未通过 copy baseline。本次运行不能声明绝对速度 gate 达标。
