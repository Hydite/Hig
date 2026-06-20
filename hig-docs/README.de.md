# Hig

## Language Index

- English: [../README.md](../README.md)
- 中文: [README.zh-CN.md](README.zh-CN.md)
- 한국어: [README.ko.md](README.ko.md)
- Deutsch: [README.de.md](README.de.md)
- Русский: [README.ru.md](README.ru.md)
- 日本語: [README.ja.md](README.ja.md)

## Abstract

Wir entwickeln Hig als Desktop-Anwendung fuer schnelle, kompakte und verschluesselte Projektarchive. Unser Ziel ist, Projekt-Snapshots im aktiven Entwicklungsalltag praktikabel zu machen: schnell genug fuer haeufige Nutzung, klein genug zum Aufbewahren oder Uebertragen, und streng genug fuer nachvollziehbare Verifikation.

In unserem neuesten oeffentlichen Benchmark mit direktem Vergleich zu `zip`, `tar.gz` und `tar.zst` erzeugte Hig ein kleineres Archiv und schloss den gemessenen Projektarchiv-Workflow deutlich schneller ab als die Vergleichswerkzeuge.

## Wichtigste Vorteile

| Vorteil | Ergebnis im neuesten oeffentlichen Vergleich |
| --- | --- |
| Geschwindigkeit | `164.008 ms` project CLI wall time. Gegenueber zip, tar.gz und tar.zst wurde die gemessene Zeit um `96.0%`, `98.4%` und `97.6%` reduziert. |
| Archivgroesse | `57,108,395 bytes`. Das Archiv war `15.7%` kleiner als zip, `6.9%` kleiner als tar.gz und `12.0%` kleiner als tar.zst. |
| Inkrementeller Workflow | Single-edit- und five-edit-Archivoperationen wurden in `253.535 ms` und `96.243 ms` abgeschlossen. |
| Burst-Verarbeitung | Ein 1000-event catch-up wurde in `111.635 ms` abgeschlossen, mit `0` watcher overflows. |
| Korrektheit | Der Benchmark correctness digest match war `true`. |
| Desktop-Reife | v1.9.4 Desktop-Paket-Build, Pruefsummenverifikation, Linting, Tests und UI-Overflow-Pruefungen wurden bestanden. |

## Was Hig macht

Hig paketiert ein Projekt in ein wiederherstellbares verschluesseltes Archiv und ist besonders auf wiederholte Projekt-Snapshots ausgelegt. Wir orientieren das Produkt an Workflows wie dem Sichern eines Projektzustands vor riskanten Aenderungen, dem Uebertragen kompakter Archive zwischen Maschinen, schnellen lokalen Wiederherstellungspunkten und verifizierten Release-Artefakten.

Dieses oeffentliche Desktop-Release-Repository konzentriert sich auf die nutzerseitige Anwendung, Dokumentation und herunterladbare Pakete.

## Benchmark-Methode

Der neueste oeffentliche Vergleich verwendete einen Testkorpus mit `15,330` Dateien und insgesamt `198,974,618` Bytes (`198.97 MB`, `189.76 MiB`). Derselbe Korpus wurde fuer Hig, zip, tar.gz und tar.zst verwendet.

Umgebungsstatus: `ENVIRONMENT_NOT_QUALIFIED`  
Correctness digest match: `true`  
Watcher overflow count: `0`

Da die Umgebung nicht als vollstaendig qualifiziert markiert war, sollten die folgenden Zahlen als transparenter Benchmark-Schnappschuss und nicht als allgemeine Leistungsgarantie gelesen werden.

## Benchmark-Ergebnisse

| Werkzeug oder Szenario | Dauer | Archivgroesse | Zeitreduktion gegenueber Basis | Groessenreduktion gegenueber Basis |
| --- | ---: | ---: | ---: | ---: |
| Hig project CLI wall | `164.008 ms` | - | - | - |
| Hig project burst archive | `120.430 ms` | `57,108,395 bytes` | - | - |
| zip | `4,088 ms` | `67,749,381 bytes` | Hig CLI wall war `96.0%` niedriger, `24.9x` schneller | Hig-Archiv war `15.7%` kleiner |
| tar.gz | `10,098 ms` | `61,313,475 bytes` | Hig CLI wall war `98.4%` niedriger, `61.6x` schneller | Hig-Archiv war `6.9%` kleiner |
| tar.zst | `6,724 ms` | `64,898,790 bytes` | Hig CLI wall war `97.6%` niedriger, `41.0x` schneller | Hig-Archiv war `12.0%` kleiner |

Messungen fuer wiederholtes Packen und Hot-Path:

| Szenario oder Phase | Messwert |
| --- | ---: |
| Same-corpus warm pack sample #2, vollstaendiges Archivschreiben | `171,100 us` / `171.100 ms` |
| Same-corpus warm pack sample #3, vollstaendiges Archivschreiben | `150,134 us` / `150.134 ms` |
| Same-corpus warm pack median, 20 vollstaendige Schreibsamples | `108,916 us` / `108.916 ms` |
| Same-corpus warm pack p95, 20 vollstaendige Schreibsamples | `455,894 us` / `455.894 ms` |
| Projektmetadaten-Verifikation, warm median | `10,102 us` |
| Planung, warm median | `2,639 us` |
| Manifest-Serialisierung, warm median | `1,004 us` |
| Manifest-Verschluesselung, warm median | `690 us` |
| Ausgabedatei erstellen, warm median | `119 us` |
| Lesen und Kompression, warm median | `0 us` / `0 us` |
| Single-edit pack | `253.535 ms` |
| Five-edit pack | `96.243 ms` |
| 1000-event burst catch-up | `111.635 ms` |

## v1.9.4 Desktop-Release

Neuester oeffentlicher Build: `v1.9.4`  
Primaeres Paket: `hig-v1.9.4-desktop-macos-universal.dmg`  
SHA-256: `b7075058b98b848a332efeca31f5320ccfe1ccd2accd83173145b5e00df7a7af`  
Paketgroesse: etwa `21 MB`

| Verifikationspunkt | Ergebnis |
| --- | --- |
| Desktop-Paket-Build | Bestanden |
| macOS universal Build | Bestanden |
| CLI-Version im Bundle | `hig 1.9.4` |
| DMG SHA-256-Verifikation | Bestanden |
| Release-Pruefsummenverifikation | Bestanden |
| Core-Qualitaetspruefungen | Bestanden |
| Desktop Linting, Tests und Build | Bestanden |
| Frontend-Tests | Bestanden, 9 Tests |
| UI-Overflow-Stichproben | Bestanden |

Das App-Bundle wurde mit der lokal verfuegbaren Apple Development-Identitaet signiert und mit hardened runtime gebaut. Notarization wurde fuer diesen Build nicht ausgefuehrt, da keine Developer ID Notarization-Zugangsdaten konfiguriert waren.

## Interpretation

Unsere Lesart der Daten ist, dass Hig besonders in Projekt-Snapshot-Workflows stark ist, wenn wiederholte Archivoperationen, kompakte Ausgaben und Korrektheitspruefungen gleichzeitig wichtig sind. Traditionelle allgemeine Archivwerkzeuge bleiben breit kompatibel und nuetzlich, aber in dieser gemessenen Projektlast lieferte Hig deutlich niedrigere wall time und kleinere Ausgabe.

## Entwickler

Yike Wang  
GitHub: [Aiomx](https://github.com/Aiomx)  
Veroeffentlicht unter: [Hydite](https://github.com/Hydite)
