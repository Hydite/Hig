# HIGV2 Public Test Vectors

Status: Public interoperability guidance draft  
Format version: HIGV2, version `2`  
Normative language: English  
Last updated: 2026-06-22

## Language Index

- English: this document is the normative test-vector plan.
- 中文: 本文档定义公开测试向量，用于验证第三方 reader/writer 的兼容性，不包含真实项目数据。
- 한국어: 이 문서는 실제 프로젝트 데이터를 포함하지 않는 공개 test vector 계획입니다.
- Deutsch: Dieses Dokument definiert oeffentliche Testvektoren ohne reale Projektdaten.
- Русский: Этот документ определяет публичные тестовые векторы без реальных проектных данных.
- 日本語: この文書は実プロジェクトデータを含まない公開 test vector を定義します。

## Purpose

Test vectors provide independent implementers with stable examples for validating HIGV2 archive parsing, authentication, decompression, path safety, and error handling. They are intentionally synthetic. No benchmark corpus, user project, private path, internal cache entry, daemon state, or production secret belongs in a public test vector.

This document defines the public vector set that should accompany the HIGV2 format specification. Binary vector files MAY be published later under `docs/spec/vectors/` or a separate release artifact. Until binary files are published, this document is the canonical vector manifest.

## Publication Rules

Public test vectors MUST:

- use artificial file names and artificial contents;
- avoid user names, internal paths, private repositories, and real benchmark corpora;
- avoid real passwords used by developers or tests outside this specification;
- include expected pass/fail outcome;
- include expected reconstructed file tree;
- include expected error class for negative vectors;
- be small enough for manual inspection except where the vector specifically tests chunking;
- be reproducible from a documented generator.

Public test vectors MUST NOT:

- include daemon cache files;
- include project snapshot files;
- include desktop app settings;
- include signing material;
- include benchmark data from proprietary repositories;
- include optimization traces or internal performance telemetry.

## Common Fixture Metadata

Unless a vector states otherwise:

- archive format: HIGV2;
- manifest profile: compact;
- compression: zstd;
- password for encrypted vectors: `correct horse battery staple`;
- wrong password for negative checks: `wrong password`;
- path separator: `/`;
- file mode for regular files: `0644`;
- directories are inferred from file paths.

The public password above is for test vectors only. It MUST NOT be reused for product, release, or benchmark secrets.

## Vector Categories

### TV-001: Empty Directory

Purpose: verify a valid HIGV2 archive with zero files.

Input tree:

```text
.
```

Expected:

- archive parses as HIGV2;
- manifest file list is empty;
- root hash is BLAKE3 over an empty concatenation;
- extraction creates the requested output directory;
- no file output is produced.

Required checks:

- reader accepts the archive;
- reader does not require at least one block;
- reader does not create unexpected placeholder files.

### TV-002: Single Small Text File

Purpose: verify single-file packing, hashing, zstd decompression, and metadata handling.

Input tree:

```text
hello.txt
```

File content:

```text
hello hig
```

Expected output tree:

```text
hello.txt
```

Expected semantic checks:

- `BLAKE3(file_content)` equals the manifest file content hash;
- the file is restored byte-for-byte;
- permissions are restored when supported;
- mtime is treated as advisory.

### TV-003: Nested Paths

Purpose: verify path separator normalization and directory creation.

Input tree:

```text
src/main.txt
src/lib/util.txt
docs/readme.txt
```

Expected:

- all archive paths use `/`;
- extraction creates parent directories;
- no absolute paths are produced;
- files are restored byte-for-byte.

### TV-004: Batch Block

Purpose: verify HIGV2 small-file batching.

Input tree:

```text
a.txt
b.txt
c.txt
```

Contents:

```text
a.txt: alpha
b.txt: beta
c.txt: gamma
```

Expected block semantics:

- files MAY be represented as one `Batch` or `Solid` block;
- raw block payload is file contents concatenated in manifest order;
- each file layout references an inline slice;
- each inline slice is within the raw block bounds.

Required reader behavior:

- decode the shared block once;
- restore each file by slicing the raw block;
- verify each file hash, or verify root hash when compact per-file hashes are omitted.

### TV-005: Empty File

Purpose: verify zero-byte file semantics.

Input tree:

```text
empty.bin
nonempty.txt
```

Expected:

- `empty.bin` has size `0`;
- empty file layout is `Empty`;
- no payload bytes are required for the empty file;
- extraction creates a zero-byte regular file.

### TV-006: Chunked Large File

Purpose: verify chunk references and reconstruction of a file larger than the chunk threshold.

Input tree:

```text
large.bin
```

Recommended generated content:

```text
for i in 0..(2 MiB + 17):
  byte[i] = i mod 251
```

Expected:

- file MAY be represented as multiple `Chunk` blocks;
- every chunk has a `chunk_hash`;
- chunk ranges reconstruct the exact original file;
- chunk references do not exceed file bounds.

Required negative sub-check:

- changing one stored chunk byte MUST cause either block hash mismatch, chunk hash mismatch, decompression failure, or root hash mismatch.

### TV-007: Password-Encrypted Archive

Purpose: verify Argon2id and ChaCha20-Poly1305 processing.

Input tree:

```text
message.txt
```

Content:

```text
public vector payload
```

Password:

```text
correct horse battery staple
```

Expected:

- correct password extracts successfully;
- wrong password fails before trusted output is written;
- manifest authentication failure is treated as fatal;
- block authentication failure is treated as fatal.

### TV-008: No-Encryption Archive

Purpose: verify no-encryption mode semantics.

Input tree:

```text
plain.txt
```

Content:

```text
no encryption vector
```

Expected:

- no password is required;
- payloads are still zstd-compressed;
- BLAKE3 checks still apply;
- UI or caller SHOULD label the archive as not confidential.

### TV-009: Tampered Manifest

Purpose: verify manifest integrity.

Mutation:

- flip one bit in the protected manifest payload.

Expected:

- password archive: authentication failure;
- no-encryption archive: manifest decode, decompression, or root hash failure;
- no trusted output files are written.

### TV-010: Tampered Block Payload

Purpose: verify block and file integrity.

Mutation:

- flip one bit in a stored block payload.

Expected:

- password archive: AEAD authentication failure, or later validation failure if mutation is outside AEAD mode;
- no-encryption archive: block hash, decompression, file hash, chunk hash, or root hash failure;
- extraction fails before final trusted output replacement.

### TV-011: Unsafe Absolute Path

Purpose: verify path traversal protection.

Malicious manifest path examples:

```text
/tmp/hig-owned
C:/tmp/hig-owned
```

Expected:

- reader rejects the archive;
- no output outside the selected extraction directory is written.

### TV-012: Unsafe Parent Path

Purpose: verify `..` traversal protection.

Malicious manifest path examples:

```text
../escape.txt
safe/../../escape.txt
```

Expected:

- reader rejects the archive;
- no output outside the selected extraction directory is written.

### TV-013: Unknown Magic

Purpose: verify archive type rejection.

Mutation:

- replace `HIGV2\0\0\0` with another 8-byte value.

Expected:

- reader rejects the archive as unsupported or invalid;
- reader does not attempt extraction.

### TV-014: Unsupported Version

Purpose: verify version gating.

Mutation:

- keep HIGV2 magic but set header version to `3`.

Expected:

- HIGV2 reader rejects the archive as unsupported version;
- reader does not attempt best-effort extraction.

### TV-015: Truncated Archive

Purpose: verify short-read handling.

Mutation:

- truncate the archive at several positions: inside header, inside manifest, inside first block, and after the last partial block.

Expected:

- reader reports malformed/truncated input;
- no trusted output is written.

## Recommended Vector Metadata File

Each binary vector SHOULD be accompanied by a JSON metadata file:

```json
{
  "id": "TV-002",
  "format": "HIGV2",
  "version": 2,
  "encrypted": true,
  "password": "correct horse battery staple",
  "expected": {
    "ok": true,
    "files": [
      {
        "path": "hello.txt",
        "size": 9,
        "blake3": "<hex>"
      }
    ]
  }
}
```

Negative vectors SHOULD include:

```json
{
  "expected": {
    "ok": false,
    "error_class": "unsafe_path"
  }
}
```

Error classes are stable categories, not exact human-readable strings.

## Stable Error Classes

Recommended public error classes:

| Class | Meaning |
| --- | --- |
| `unknown_magic` | archive magic is not recognized |
| `unsupported_version` | version is not supported |
| `unsupported_flags` | header flags are not supported |
| `invalid_header` | fixed header cannot be parsed |
| `kdf_failed` | key derivation failed |
| `auth_failed` | AEAD authentication failed |
| `manifest_decode_failed` | manifest cannot be decompressed or decoded |
| `manifest_root_mismatch` | root hash validation failed |
| `unsupported_codec` | block codec is not supported |
| `block_hash_mismatch` | compressed block hash mismatch |
| `block_decompress_failed` | block decompression failed |
| `file_hash_mismatch` | reconstructed file hash mismatch |
| `chunk_hash_mismatch` | chunk hash mismatch |
| `unsafe_path` | archive path is absolute or escapes output root |
| `range_out_of_bounds` | inline or chunk range exceeds valid bounds |
| `overwrite_refused` | output exists and overwrite was not allowed |
| `resource_limit` | implementation safety limit exceeded |
| `truncated_archive` | archive ended before required bytes were read |

## Generation Policy

A future public vector generator SHOULD:

1. create all input trees under a temporary directory;
2. use deterministic file contents;
3. use fixed file mtimes and permissions where the platform supports them;
4. create encrypted and no-encryption variants;
5. produce metadata JSON;
6. run a reference reader and at least one independent reader;
7. verify no generated vector contains private paths.

## Multilingual Summary

### 中文摘要

公开测试向量应覆盖空目录、小文件、嵌套路径、batch block、空文件、chunked 大文件、密码加密、无加密、manifest 篡改、block 篡改、不安全路径、未知 magic、未知版本和截断归档。所有数据必须是人工构造样本，不能包含真实项目、私有路径、benchmark corpus、daemon/cache/project snapshot 或密钥。

### 한국어 요약

공개 test vector는 empty directory, small file, nested paths, batch block, empty file, chunked large file, password encryption, no encryption, tampered manifest, tampered block, unsafe paths, unknown magic, unsupported version, truncated archive를 포함해야 합니다. 모든 데이터는 합성 데이터여야 하며 실제 프로젝트, private path, benchmark corpus, daemon/cache/project snapshot, secret을 포함하면 안 됩니다.

### Deutsche Zusammenfassung

Oeffentliche Testvektoren sollen leere Archive, kleine Dateien, verschachtelte Pfade, Batch-Blocks, leere Dateien, gechunkte grosse Dateien, Passwortverschluesselung, unverschluesselte Archive, manipulierte Manifeste, manipulierte Blocks, unsichere Pfade, unbekannte Magic-Werte, nicht unterstuetzte Versionen und abgeschnittene Archive abdecken. Alle Daten muessen synthetisch sein.

### Русское резюме

Публичные тестовые векторы должны покрывать пустые каталоги, малые файлы, вложенные пути, batch blocks, пустые файлы, chunked large files, password encryption, no encryption, tampered manifest, tampered block, unsafe paths, unknown magic, unsupported version и truncated archive. Данные должны быть синтетическими и не содержать приватной информации.

### 日本語概要

公開 test vector は empty directory、small file、nested paths、batch block、empty file、chunked large file、password encryption、no encryption、tampered manifest、tampered block、unsafe paths、unknown magic、unsupported version、truncated archive を対象にします。データは合成データのみとし、実プロジェクトや秘密情報を含めません。
