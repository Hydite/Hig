# Hig

## Language Index

- English: [../README.md](../README.md)
- 中文: [README.zh-CN.md](README.zh-CN.md)
- 한국어: [README.ko.md](README.ko.md)
- Deutsch: [README.de.md](README.de.md)
- Русский: [README.ru.md](README.ru.md)
- 日本語: [README.ja.md](README.ja.md)

## 概要

私たちは Hig を、高速でコンパクトな暗号化プロジェクトアーカイブを作成するためのデスクトップアプリケーションとして開発しています。目標は、active development の中で project snapshot を実用的にすることです。頻繁に実行できる速度、保存や移動に適したサイズ、そして検証可能な厳密さを重視しています。

`zip`、`tar.gz`、`tar.zst` と直接比較した最新の公開 benchmark では、Hig はより小さいアーカイブを生成し、測定対象の project archive workflow を baseline tools より大幅に速く完了しました。

## 主な利点

| 利点 | 最新の公開比較結果 |
| --- | --- |
| 速度 | project CLI wall time は `164.008 ms`。zip、tar.gz、tar.zst と比較して、測定時間はそれぞれ `96.0%`、`98.4%`、`97.6%` 削減されました。 |
| アーカイブサイズ | `57,108,395 bytes`。zip より `15.7%`、tar.gz より `6.9%`、tar.zst より `12.0%` 小さい結果でした。 |
| インクリメンタル workflow | single-edit と five-edit の archive operations は `253.535 ms`、`96.243 ms` で完了しました。 |
| burst 処理 | 1000-event catch-up は `111.635 ms` で完了し、watcher overflow は `0` でした。 |
| 正確性 | benchmark correctness digest match は `true` でした。 |
| デスクトップの準備状況 | v1.9.4 desktop package build、checksum verification、linting、tests、UI overflow checks はすべて合格しました。 |

## Hig が行うこと

Hig は、プロジェクトを復元可能な暗号化アーカイブとしてパッケージ化し、繰り返しの project snapshot workflow を重視します。私たちは、リスクの高い変更前のプロジェクト状態保存、コンパクトなアーカイブのマシン間移動、すばやいローカル recovery point、検証済み release artifact の保存といった workflow を想定して設計しています。

この公開 desktop release repository は、ユーザー向けアプリケーション、ドキュメント、ダウンロード可能なパッケージに焦点を当てています。

## Benchmark 方法

最新の公開比較 dataset は、`15,330` ファイル、合計 `198,974,618` bytes（`198.97 MB`、`189.76 MiB`）の test corpus を使用しました。同じ corpus を Hig、zip、tar.gz、tar.zst の比較に使用しています。

Environment status: `ENVIRONMENT_NOT_QUALIFIED`  
Correctness digest match: `true`  
Watcher overflow count: `0`

環境は完全に qualified とマークされていないため、以下の数値は普遍的な性能保証ではなく、透明な benchmark snapshot として読むべきです。

## Benchmark 結果

| Tool または scenario | 時間 | アーカイブサイズ | baseline に対する時間削減 | baseline に対するサイズ削減 |
| --- | ---: | ---: | ---: | ---: |
| Hig project CLI wall | `164.008 ms` | - | - | - |
| Hig project burst archive | `120.430 ms` | `57,108,395 bytes` | - | - |
| zip | `4,088 ms` | `67,749,381 bytes` | Hig CLI wall は `96.0%` 低く、`24.9x` 高速 | Hig archive は `15.7%` 小さい |
| tar.gz | `10,098 ms` | `61,313,475 bytes` | Hig CLI wall は `98.4%` 低く、`61.6x` 高速 | Hig archive は `6.9%` 小さい |
| tar.zst | `6,724 ms` | `64,898,790 bytes` | Hig CLI wall は `97.6%` 低く、`41.0x` 高速 | Hig archive は `12.0%` 小さい |

繰り返し pack と hot-path の測定:

| シナリオまたは段階 | 測定値 |
| --- | ---: |
| Same-corpus warm pack sample #2, full archive write | `171,100 us` / `171.100 ms` |
| Same-corpus warm pack sample #3, full archive write | `150,134 us` / `150.134 ms` |
| Same-corpus warm pack median, 20 full-write samples | `108,916 us` / `108.916 ms` |
| Same-corpus warm pack p95, 20 full-write samples | `455,894 us` / `455.894 ms` |
| Project metadata verify, warm median | `10,102 us` |
| Planning, warm median | `2,639 us` |
| Manifest serialization, warm median | `1,004 us` |
| Manifest encryption, warm median | `690 us` |
| Output file create, warm median | `119 us` |
| Read と compression, warm median | `0 us` / `0 us` |
| Single-edit pack | `253.535 ms` |
| Five-edit pack | `96.243 ms` |
| 1000-event burst catch-up | `111.635 ms` |

## v1.9.4 Desktop Release

最新公開 build: `v1.9.4`  
主要 package: `hig-v1.9.4-desktop-macos-universal.dmg`  
SHA-256: `b7075058b98b848a332efeca31f5320ccfe1ccd2accd83173145b5e00df7a7af`  
Package size: 約 `21 MB`

| Verification item | 結果 |
| --- | --- |
| デスクトップパッケージの build | 合格 |
| macOS universal build | 合格 |
| bundle 内 CLI version | `hig 1.9.4` |
| DMG SHA-256 verification | 合格 |
| Release checksum verification | 合格 |
| Core quality checks | 合格 |
| Desktop lint、tests、build | 合格 |
| Frontend tests | 合格、9 tests |
| UI overflow checks | 合格 |

この app bundle はローカルで利用可能な Apple Development identity で署名され、hardened runtime が有効です。Developer ID notarization の認証情報が設定されていないため、この build では notarization は実行されていません。

## 解釈

私たちはこのデータを、Hig が繰り返しの archive operations、コンパクトな output、correctness checks が同時に重要な project snapshot workflow において特に強いことを示すものと見ています。従来の general-purpose archive tools は広い互換性を持ち有用ですが、この測定された project workload では Hig が大幅に低い wall time と小さい output を示しました。

## 開発者

Yike Wang  
GitHub: [Aiomx](https://github.com/Aiomx)  
公開組織: [Hydite](https://github.com/Hydite)
