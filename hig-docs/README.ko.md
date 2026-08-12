# Hig

## Language Index

- English: [../README.md](../README.md)
- 中文: [README.zh-CN.md](README.zh-CN.md)
- 한국어: [README.ko.md](README.ko.md)
- Deutsch: [README.de.md](README.de.md)
- Русский: [README.ru.md](README.ru.md)
- 日本語: [README.ja.md](README.ja.md)

## 초록

우리는 Hig를 빠르고 작고 암호화된 프로젝트 아카이브를 만들기 위한 데스크톱 애플리케이션으로 개발했습니다. 목표는 active development 중에도 project snapshot을 자주 실행할 수 있을 만큼 빠르게, 보관하거나 이동하기 좋을 만큼 작게, 그리고 검증 가능하게 만드는 것입니다.

`zip`, `tar.gz`, `tar.zst`와 직접 비교한 최신 공개 벤치마크에서 Hig는 더 작은 아카이브를 생성하면서 측정된 프로젝트 아카이브 워크플로를 훨씬 빠르게 완료했습니다.

## 핵심 장점

| 장점 | 최신 공개 비교 결과 |
| --- | --- |
| 속도 | 프로젝트 CLI wall 시간은 `164.008 ms`였습니다. zip, tar.gz, tar.zst 대비 측정 시간은 각각 `96.0%`, `98.4%`, `97.6%` 감소했습니다. |
| 아카이브 크기 | Hig 아카이브는 `57,108,395 bytes`였습니다. zip보다 `15.7%`, tar.gz보다 `6.9%`, tar.zst보다 `12.0%` 작았습니다. |
| 증분 워크플로 | single-edit 및 five-edit 아카이브 작업은 각각 `253.535 ms`, `96.243 ms`에 완료되었습니다. |
| 이벤트 burst 처리 | 1000-event catch-up은 `111.635 ms`에 완료되었고 watcher overflow는 `0`이었습니다. |
| 정확성 | 벤치마크 correctness digest match는 `true`였습니다. |
| 데스크톱 준비 상태 | v1.9.4 데스크톱 패키지 빌드, 체크섬 검증, lint, 테스트, UI overflow 검사가 통과했습니다. |

## Hig가 하는 일

Hig는 프로젝트를 복구 가능한 암호화 아카이브로 패키징하며, 반복적인 프로젝트 스냅샷 워크플로에 중점을 둡니다. 우리는 위험한 변경 전 프로젝트 상태 저장, 작은 아카이브의 머신 간 이동, 빠른 로컬 복구 지점 유지, 검증된 릴리스 아티팩트 보존 같은 흐름을 기준으로 설계했습니다.

이 공개 데스크톱 릴리스 저장소는 사용자용 애플리케이션, 문서, 다운로드 가능한 패키지에 초점을 맞춥니다.

## 벤치마크 방법

최신 공개 비교 데이터셋은 `15,330`개 파일, 총 `198,974,618`바이트(`198.97 MB`, `189.76 MiB`)의 테스트 코퍼스를 사용했습니다. Hig, zip, tar.gz, tar.zst 모두 동일한 코퍼스로 비교했습니다.

환경 상태: `ENVIRONMENT_NOT_QUALIFIED`  
Correctness digest match: `true`  
Watcher overflow count: `0`

환경이 완전히 qualified로 표시되지 않았으므로 아래 수치는 보편적 성능 보장이 아니라 투명한 벤치마크 스냅샷으로 해석해야 합니다.

## 벤치마크 결과

| 도구 또는 시나리오 | 시간 | 아카이브 크기 | 기준 대비 시간 감소 | 기준 대비 크기 감소 |
| --- | ---: | ---: | ---: | ---: |
| Hig project CLI wall | `164.008 ms` | - | - | - |
| Hig project burst archive | `120.430 ms` | `57,108,395 bytes` | - | - |
| zip | `4,088 ms` | `67,749,381 bytes` | Hig CLI wall 시간이 `96.0%` 낮았고 `24.9x` 빨랐습니다 | Hig 아카이브가 `15.7%` 작았습니다 |
| tar.gz | `10,098 ms` | `61,313,475 bytes` | Hig CLI wall 시간이 `98.4%` 낮았고 `61.6x` 빨랐습니다 | Hig 아카이브가 `6.9%` 작았습니다 |
| tar.zst | `6,724 ms` | `64,898,790 bytes` | Hig CLI wall 시간이 `97.6%` 낮았고 `41.0x` 빨랐습니다 | Hig 아카이브가 `12.0%` 작았습니다 |

반복 pack 및 hot-path 측정:

| 시나리오 또는 단계 | 측정값 |
| --- | ---: |
| 동일 테스트 코퍼스 warm pack sample #2, 전체 아카이브 쓰기 | `171,100 us` / `171.100 ms` |
| 동일 테스트 코퍼스 warm pack sample #3, 전체 아카이브 쓰기 | `150,134 us` / `150.134 ms` |
| 동일 테스트 코퍼스 warm pack median, 20개 전체 쓰기 sample | `108,916 us` / `108.916 ms` |
| 동일 테스트 코퍼스 warm pack p95, 20개 전체 쓰기 sample | `455,894 us` / `455.894 ms` |
| 프로젝트 metadata verify, warm median | `10,102 us` |
| Planning, warm median | `2,639 us` |
| Manifest serialization, warm median | `1,004 us` |
| Manifest encryption, warm median | `690 us` |
| Output file create, warm median | `119 us` |
| Read 및 compression, warm median | `0 us` / `0 us` |
| Single-edit pack | `253.535 ms` |
| Five-edit pack | `96.243 ms` |
| 1000-event burst catch-up | `111.635 ms` |

## v1.9.4 데스크톱 릴리스

최신 공개 빌드: `v1.9.4`  
기본 패키지: `hig-v1.9.4-desktop-macos-universal.dmg`  
SHA-256: `b7075058b98b848a332efeca31f5320ccfe1ccd2accd83173145b5e00df7a7af`  
패키지 크기: 약 `21 MB`

| 검증 항목 | 결과 |
| --- | --- |
| 데스크톱 패키지 빌드 | 통과 |
| macOS universal 빌드 | 통과 |
| 번들 CLI 버전 | `hig 1.9.4` |
| DMG SHA-256 검증 | 통과 |
| 릴리스 체크섬 검증 | 통과 |
| 핵심 품질 검사 | 통과 |
| 데스크톱 lint, 테스트, 빌드 | 통과 |
| 프런트엔드 테스트 | 통과, 9개 테스트 |
| UI overflow 샘플 검사 | 통과 |

이 앱 번들은 로컬에서 사용 가능한 Apple Development ID로 서명되었고 hardened runtime이 적용되었습니다. Developer ID 공증 자격 증명이 설정되지 않아 이 빌드에는 notarization이 수행되지 않았습니다.

## 해석

이 데이터를 바탕으로 우리는 Hig가 반복 아카이브 작업, 작은 출력 크기, 정확성 검사가 모두 중요한 프로젝트 스냅샷 워크플로에서 가장 강하다고 봅니다. 범용 아카이브 도구는 여전히 넓은 호환성을 제공하지만, 이 측정된 프로젝트 워크로드에서는 Hig가 훨씬 낮은 wall time과 더 작은 출력을 제공했습니다.

## 개발자

Yike Wang  
GitHub: [Aiomx](https://github.com/Aiomx)  
게시 조직: [Hydite](https://github.com/Hydite)
