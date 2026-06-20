# Hig

## Language Index

- English: [../README.md](../README.md)
- 中文: [README.zh-CN.md](README.zh-CN.md)
- 한국어: [README.ko.md](README.ko.md)
- Deutsch: [README.de.md](README.de.md)
- Русский: [README.ru.md](README.ru.md)
- 日本語: [README.ja.md](README.ja.md)

## Аннотация

Мы разрабатываем Hig как настольное приложение для быстрого, компактного и зашифрованного архивирования проектов. Наша цель — сделать project snapshots практичными в активной разработке: достаточно быстрыми для частого запуска, достаточно компактными для хранения и переноса, и достаточно строгими для проверки результата.

В нашем последнем публичном benchmark с прямым сравнением `zip`, `tar.gz` и `tar.zst` Hig создал меньший архив и завершил измеренный workflow архивирования проекта существенно быстрее базовых инструментов.

## Ключевые преимущества

| Преимущество | Результат в последнем публичном сравнении |
| --- | --- |
| Скорость | `164.008 ms` project CLI wall time. По сравнению с zip, tar.gz и tar.zst измеренное время было снижено на `96.0%`, `98.4%` и `97.6%`. |
| Размер архива | `57,108,395 bytes`. Архив был на `15.7%` меньше zip, на `6.9%` меньше tar.gz и на `12.0%` меньше tar.zst. |
| Инкрементальный workflow | Операции single-edit и five-edit archive завершились за `253.535 ms` и `96.243 ms`. |
| Обработка burst-событий | 1000-event catch-up завершился за `111.635 ms`, watcher overflows: `0`. |
| Корректность | Benchmark correctness digest match: `true`. |
| Готовность desktop-релиза | Для v1.9.4 прошли сборка desktop package, проверка checksum, linting, tests и UI overflow checks. |

## Что делает Hig

Hig упаковывает проект в восстанавливаемый зашифрованный архив с акцентом на повторяющиеся project snapshots. Мы проектируем его вокруг таких workflow, как сохранение состояния проекта перед рискованным изменением, перенос компактного архива между машинами, быстрый локальный recovery point и сохранение проверенного release artifact.

Этот публичный desktop release repository ориентирован на пользовательское приложение, документацию и скачиваемые пакеты.

## Метод benchmark

Последний публичный набор сравнения использовал тестовый корпус из `15,330` файлов общим размером `198,974,618` байт (`198.97 MB`, `189.76 MiB`). Один и тот же корпус использовался для Hig, zip, tar.gz и tar.zst.

Статус окружения: `ENVIRONMENT_NOT_QUALIFIED`  
Correctness digest match: `true`  
Watcher overflow count: `0`

Поскольку окружение не было помечено как полностью qualified, приведенные ниже числа следует читать как прозрачный benchmark snapshot, а не как универсальную гарантию производительности.

## Результаты benchmark

| Инструмент или сценарий | Время | Размер архива | Снижение времени относительно baseline | Снижение размера относительно baseline |
| --- | ---: | ---: | ---: | ---: |
| Hig project CLI wall | `164.008 ms` | - | - | - |
| Hig project burst archive | `120.430 ms` | `57,108,395 bytes` | - | - |
| zip | `4,088 ms` | `67,749,381 bytes` | Hig CLI wall был ниже на `96.0%`, быстрее в `24.9x` | Архив Hig был меньше на `15.7%` |
| tar.gz | `10,098 ms` | `61,313,475 bytes` | Hig CLI wall был ниже на `98.4%`, быстрее в `61.6x` | Архив Hig был меньше на `6.9%` |
| tar.zst | `6,724 ms` | `64,898,790 bytes` | Hig CLI wall был ниже на `97.6%`, быстрее в `41.0x` | Архив Hig был меньше на `12.0%` |

Измерения повторного pack и hot-path:

| Сценарий или этап | Значение |
| --- | ---: |
| Same-corpus warm pack sample #2, полная запись архива | `171,100 us` / `171.100 ms` |
| Same-corpus warm pack sample #3, полная запись архива | `150,134 us` / `150.134 ms` |
| Same-corpus warm pack median, 20 samples полной записи | `108,916 us` / `108.916 ms` |
| Same-corpus warm pack p95, 20 samples полной записи | `455,894 us` / `455.894 ms` |
| Проверка metadata проекта, warm median | `10,102 us` |
| Планирование, warm median | `2,639 us` |
| Manifest serialization, warm median | `1,004 us` |
| Manifest encryption, warm median | `690 us` |
| Создание output file, warm median | `119 us` |
| Read и compression, warm median | `0 us` / `0 us` |
| Single-edit pack | `253.535 ms` |
| Five-edit pack | `96.243 ms` |
| 1000-event burst catch-up | `111.635 ms` |

## Desktop release v1.9.4

Последняя публичная сборка: `v1.9.4`  
Основной пакет: `hig-v1.9.4-desktop-macos-universal.dmg`  
SHA-256: `b7075058b98b848a332efeca31f5320ccfe1ccd2accd83173145b5e00df7a7af`  
Размер пакета: около `21 MB`

| Пункт проверки | Результат |
| --- | --- |
| Сборка desktop-пакета | Пройдена |
| macOS universal build | Пройдена |
| Версия CLI в bundle | `hig 1.9.4` |
| Проверка DMG SHA-256 | Пройдена |
| Проверка контрольной суммы релиза | Пройдена |
| Основные проверки качества | Пройдены |
| Desktop lint, тесты и сборка | Пройдены |
| Frontend-тесты | Пройдены, 9 тестов |
| Выборочная проверка UI overflow | Пройдена |

App bundle подписан локально доступной Apple Development identity и собран с hardened runtime. Notarization для этой сборки не выполнялась, потому что учетные данные Developer ID notarization не были настроены.

## Интерпретация

Мы читаем эти данные так: Hig особенно силен в project snapshot workflow, где одновременно важны повторные archive operations, компактный результат и проверки корректности. Традиционные универсальные archive tools остаются широко совместимыми и полезными, но в этой измеренной project workload Hig дал существенно меньшее wall time и меньший output.

## Разработчик

Yike Wang  
GitHub: [Aiomx](https://github.com/Aiomx)  
Опубликовано в организации: [Hydite](https://github.com/Hydite)
