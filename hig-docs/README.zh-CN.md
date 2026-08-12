# Hig

## 语言索引

- English: [../README.md](../README.md)
- 中文: [README.zh-CN.md](README.zh-CN.md)
- 한국어: [README.ko.md](README.ko.md)
- Deutsch: [README.de.md](README.de.md)
- Русский: [README.ru.md](README.ru.md)
- 日本語: [README.ja.md](README.ja.md)

## 摘要

Hig 是一个快速、紧凑、面向项目的归档工具，支持安全加密归档、daemon 项目监听和 macOS 桌面界面。它面向真实开发工作流：在高风险修改前保存可验证快照，在机器之间移动紧凑项目状态，或保留快速本地恢复点。

当前发布候选版本是 **v1.9.6**。本版本不改变 HIGV2 归档格式，也不降低默认安全模型：密码归档继续使用 Argon2id、ChaCha20-Poly1305 认证加密、BLAKE3 完整性校验和路径穿越防护。

## 下载产物

| 产物 | 路径 |
| --- | --- |
| macOS Universal DMG | `artifacts/hig-v1.9.6-desktop-macos-universal.dmg` |
| DMG SHA-256 | `artifacts/hig-v1.9.6-desktop-macos-universal.dmg.sha256` |
| 源码包 | `artifacts/hig-v1.9.6-source.tar.gz` |
| 源码 SHA-256 | `artifacts/hig-v1.9.6-source.tar.gz.sha256` |

macOS App 使用当前机器可用的 Apple Development 身份签名。由于没有配置 Developer ID 公证凭据，本构建 **未执行 notarization**，因此不宣称适合公开互联网分发。

DMG SHA-256：

```text
be8ea2247ee552eeaa794e1400c69ce69f433d76ff02736a88c0cc3d1f4862de
```

## 快速开始

### 桌面 App

1. 打开 DMG 并启动 Hig。
2. 在 **Runtime** 页面启动 daemon；如果需要安全的重复归档速度，可以解锁 session。
3. 在 **Projects** 页面初始化目录，让 Hig 在后台准备项目快照。
4. 在 **Create Archive** 页面把目录打包为 `.hig`。
5. 在 **Open Archive** 页面检查和解压 `.hig` 归档。
6. 在 **Cache** 页面执行 GC/compact dry-run 和确认操作，在 **Diagnostics** 页面运行 benchmark 对比。

### CLI

```bash
hig pack <dir> -o <archive.hig> --password <password>
hig inspect <archive.hig> --password <password> --json
hig unpack <archive.hig> -d <output-dir> --password <password>
```

Project Mode：

```bash
hig init <dir>
hig session unlock --password <password> --cache-dir <cache-dir>
hig pack <dir> -o <archive.hig> --use-session --cache-dir <cache-dir>
```

Benchmark：

```bash
hig bench /Volumes/Build/lobehub \
  --compare \
  --bench-suite lobehub-watch \
  --daemon required \
  --cache-dir /private/tmp/hig-v196-cache \
  --bench-dir /private/tmp/hig-v196-bench \
  --password benchmark-password \
  --json
```

## 安全模型

| 模式 | 安全行为 |
| --- | --- |
| 默认 balanced + password | secure Argon2id KDF、随机 archive salt、独立 block 认证，默认不信任 metadata。 |
| Session | 只派生一次 secure key，并只在 daemon 内存中保留到 TTL 到期。密码和 key 不写入磁盘。 |
| Project Mode | 使用 daemon 持有的已验证快照。watcher 溢出、重启或失去可信状态时，Hig 会退回全量验证或在 required 模式下失败。 |
| Fastest | 显式极速模式。可能信任 metadata 并复用 sealed encrypted cache，因此 CLI 和 UI 都会显示风险提示。 |
| No encryption | 只提供压缩和 hash 校验，不提供保密性或 AEAD 认证加密。 |

## Cache 与 Daemon

Hig 在用户选择的 cache 目录下保存压缩对象和索引元数据，用于加速重复归档和项目监听工作流。cache 不保存明文密码或派生加密密钥。cache 损坏只能导致 fail-fast 或 cache miss；已经生成的 `.hig` 归档仍然自包含。

daemon 持有 project watcher、任务队列、session key、cache 状态和 diagnostics 能力。桌面 App 和 CLI 通过同一套 daemon/task 语义提交 pack、unpack、rebuild、GC 和 compact 任务。

## Benchmark 解读

v1.9.6 benchmark 报告会写出：

- `environment_status`：所选卷是否通过 256MiB copy baseline。
- `release_gate_status`：绝对 gate 是否通过、是否在合格卷失败，或是否因为环境不合格而不能声明绝对达标。
- `io_hotspot_summary`：warm path 中最大的瓶颈阶段。
- 同一语料下的 zip、tar.gz、tar.zst 对比。

如果环境显示为 `ENVIRONMENT_NOT_QUALIFIED`，Hig 仍会报告相对速度和体积结果，但不会宣称该次运行满足绝对 `<150ms` project warm-pack 指标。

v1.9.6 LobeHub RC 测试选择 `/private/tmp`，256MiB copy median 为 `538.21 MiB/s`，低于 `650 MiB/s` 资格线。Hig 归档为 `57,110,242` 字节，zip 为 `67,749,385`，tar.gz 为 `61,332,985`。Project warm median 为 `169.99ms`，剩余波动主要来自输出写入和 flush。

## 文档

- 桌面使用指南：[desktop-guide.zh-CN.md](desktop-guide.zh-CN.md)
- English README：[../README.md](../README.md)

v1.9.6 的发布候选文档以英文为准。其他语言可能不会同步全部发布细节。

## 开发者

Yike Wang  
GitHub：[Aiomx](https://github.com/Aiomx)  
发布组织：[Hydite](https://github.com/Hydite)
