# HIGV2 Archive Format Specification

Status: Public format specification draft  
Format version: HIGV2, version `2`  
Normative language: English  
Last updated: 2026-06-22

## Language Index

- English: this document is the normative specification.
- 中文: 本文档公开 `.hig` 可移植归档格式，不描述 Hig CLI、Desktop、daemon、cache engine 或 project snapshot 的内部实现。
- 한국어: 이 문서는 이식 가능한 `.hig` 아카이브 형식을 공개하며 Hig CLI, Desktop, daemon, cache engine, project snapshot 내부 구현은 설명하지 않습니다.
- Deutsch: Dieses Dokument beschreibt das portable `.hig`-Archivformat und nicht die interne Implementierung von Hig CLI, Desktop, Daemon, Cache Engine oder Project Snapshots.
- Русский: Этот документ описывает переносимый формат архива `.hig`, но не внутреннюю реализацию Hig CLI, Desktop, daemon, cache engine или project snapshot.
- 日本語: この文書は portable な `.hig` archive format を公開するものであり、Hig CLI、Desktop、daemon、cache engine、project snapshot の内部実装は説明しません。

## Abstract

HIGV2 is a self-contained archive format for project-oriented file sets. It is designed to support compact archives, authenticated encrypted payloads, repeated-file verification, and efficient representation of many small files. A conforming reader can inspect and extract a HIGV2 archive without access to the Hig cache, daemon, desktop application, or project snapshot system.

This specification intentionally separates the portable archive format from implementation strategy. Cache layout, watcher design, task scheduling, desktop commands, benchmark harnesses, sealed local cache objects, and product-specific optimization techniques are out of scope.

## Scope

This document specifies:

- archive magic and version recognition;
- fixed header layout;
- password and no-encryption archive modes;
- manifest structure and compatibility requirements;
- batch, single, chunk, and solid block semantics;
- compression, hashing, and cryptographic primitives;
- extraction safety rules;
- compatibility and error handling expectations.

This document does not specify:

- local cache index layout;
- daemon protocol;
- project snapshot storage;
- desktop UI behavior;
- benchmark methodology;
- internal scheduling, buffering, or hot-path optimization;
- commercial packaging, signing, licensing, or update channels.

## Conformance Terms

The terms MUST, MUST NOT, REQUIRED, SHALL, SHALL NOT, SHOULD, SHOULD NOT, RECOMMENDED, MAY, and OPTIONAL are to be interpreted as described in RFC 2119 and RFC 8174.

A conforming HIGV2 reader MUST be able to:

1. identify HIGV2 archives;
2. parse the fixed header;
3. derive the archive key when password encryption is used;
4. authenticate and decode the manifest;
5. authenticate, decompress, and verify payload blocks;
6. reconstruct files according to manifest layout;
7. reject unsafe paths and malformed archives.

A conforming HIGV2 writer SHOULD emit archives that can be extracted by an independent reader using only this specification and the declared serialization profile.

## Primitive Types

Unless stated otherwise:

- multi-byte integers are little-endian;
- byte strings are exact byte sequences;
- hashes are 32-byte BLAKE3 digests;
- nonces are 12-byte ChaCha20-Poly1305 nonces;
- salts are 16-byte random values;
- paths inside the archive use `/` as separator;
- sizes and offsets are unsigned 64-bit integers unless otherwise specified;
- timestamps use Unix nanoseconds as signed integer values where HIGV2 fields say `mtime_ns`.

## Archive File Layout

A HIGV2 file has the following high-level layout:

```text
+------------------------------+
| fixed header                 |
+------------------------------+
| protected manifest payload   |
+------------------------------+
| protected block payload 0    |
+------------------------------+
| protected block payload 1    |
+------------------------------+
| ...                          |
+------------------------------+
```

The manifest is stored before all block payloads. HIGV2 does not require a footer index. Readers MUST use the manifest to determine the number and size of block payloads.

Block payloads are stored in manifest order. In current HIGV2 archives, block payload offsets can be reconstructed by starting at:

```text
header_len + manifest_len
```

and then adding each preceding block's `encrypted_size`. Readers SHOULD ignore advisory block offsets in decoded legacy structures when sequential payload order is unambiguous.

## Magic and Version

HIGV2 archives begin with the following 8-byte magic:

```text
48 49 47 56 32 00 00 00
```

ASCII:

```text
HIGV2\0\0\0
```

The header version field MUST be `2`.

Legacy HIGV1 archives use:

```text
HIGV1\0\0\0
```

A HIGV2 reader MAY implement HIGV1 extraction for compatibility. A HIGV1-only reader MUST reject HIGV2 archives rather than attempting best-effort extraction.

## Fixed Header

The fixed header length is 64 bytes.

| Offset | Length | Field | Type | Description |
| ---: | ---: | --- | --- | --- |
| 0 | 8 | `magic` | bytes | `HIGV2\0\0\0` |
| 8 | 4 | `version` | u32 | MUST be `2` |
| 12 | 4 | `kdf_memory_cost_kib` | u32 | Argon2id memory cost in KiB |
| 16 | 4 | `kdf_time_cost` | u32 | Argon2id time cost |
| 20 | 4 | `kdf_parallelism` | u32 | Argon2id lanes/parallelism |
| 24 | 4 | `flags` | u32 | encryption mode and header flags |
| 28 | 16 | `salt` | bytes | KDF salt; random for password archives |
| 44 | 12 | `manifest_nonce` | bytes | nonce for manifest protection |
| 56 | 8 | `manifest_len` | u64 | byte length of protected manifest payload |

Note: The offset table describes the logical field order. The total byte count is:

```text
8 + 4 + 4 + 4 + 4 + 4 + 16 + 12 + 8 = 64
```

Implementations MUST use the portable 64-byte fixed header size defined above.

### Header Flags

The following `flags` values are public:

| Value | Meaning |
| ---: | --- |
| `0x00000010` | password encryption; low bits encode 16-byte salt length |
| `0x80000010` | no encryption; low bits encode 16-byte salt length |

For password archives, readers MUST use the KDF fields and salt to derive the archive key.

For no-encryption archives, readers MUST ignore the KDF fields for confidentiality purposes. Implementations MAY still require syntactically valid KDF fields for header uniformity.

Readers MUST reject unknown flag values.

## Compression

HIGV2 currently defines one portable compression codec:

```text
zstd
```

Writers MAY choose different zstd levels. Readers MUST NOT rely on a particular level; zstd frame data is self-describing for decompression. Manifest fields record compressed and raw sizes for validation and allocation control.

Readers MUST reject unknown codec names unless a future extension registry defines them.

## Hashing

HIGV2 uses BLAKE3 for:

- file content hashes;
- compressed block identifiers;
- manifest root hash;
- chunk hashes for chunked files;
- optional writer-side object keys, when disclosed by a writer.

Public archive verification depends on file, chunk, block, and root hashes. Local cache keys are not part of the portable archive format.

### Root Hash

The manifest root hash is:

```text
BLAKE3(concat(relative_path_utf8, content_hash) for files in manifest order)
```

Where `content_hash` is the 32-byte BLAKE3 hash of the reconstructed file content.

For compact manifests that omit per-file hashes, readers MUST verify the root hash after reconstructing all files.

## Encryption and Authentication

HIGV2 supports two encryption modes:

1. password mode;
2. no-encryption mode.

### Password Mode

Password mode uses:

- KDF: Argon2id, version 0x13;
- output key length: 32 bytes;
- AEAD: ChaCha20-Poly1305;
- nonce length: 12 bytes;
- salt length: 16 bytes.

The derived key is:

```text
Argon2id(password_utf8, salt, memory_cost_kib, time_cost, parallelism, out_len=32)
```

The manifest and each block payload are independently protected with ChaCha20-Poly1305.

Current public archives use no additional associated data. Future versions MAY introduce AAD only with a new format version or explicitly negotiated extension.

Readers MUST authenticate before using plaintext. If authentication fails, readers MUST fail extraction and MUST NOT write trusted output files.

### No-Encryption Mode

No-encryption mode stores protected payload fields as plain bytes:

- manifest payload is compressed manifest bytes;
- block payloads are compressed block bytes.

No-encryption mode provides no confidentiality and no AEAD authentication. It still provides structural and content integrity checks through BLAKE3 verification. Readers and user interfaces SHOULD label this mode as not confidential.

## Manifest Encoding

The protected manifest payload is decoded as follows:

1. unprotect the manifest payload according to header encryption mode;
2. zstd-decompress the result;
3. decode the decompressed manifest bytes.

HIGV2 has two public manifest profiles:

- Compact manifest profile, identified by prefix `HCM1`;
- Legacy manifest profile, serialized without the `HCM1` prefix.

Independent implementations SHOULD implement the compact manifest profile first. Legacy support is RECOMMENDED for compatibility with early HIGV2 archives.

### Compact Manifest Prefix

Compact manifests begin with:

```text
48 43 4D 31
```

ASCII:

```text
HCM1
```

The bytes after `HCM1` are a bincode-encoded `CompactManifestV1`.

### Serialization Profile

Current public HIGV2 manifests use Rust bincode v1-style serialization for the named data model. This draft treats the data model as public and the exact byte-level bincode profile as an interoperability requirement for current archives.

To avoid ecosystem lock-in, a future HIGV3 SHOULD specify a language-neutral canonical encoding. HIGV2 readers MUST use a decoder compatible with current HIGV2 archives.

## Compact Manifest Data Model

### `CompactManifestV1`

| Field | Type | Description |
| --- | --- | --- |
| `schema` | u16 | MUST be `1` |
| `root_hash` | [u8; 32] | manifest root hash |
| `files` | Vec<CompactFileEntry> | file entries in deterministic archive order |
| `blocks` | Vec<CompactBlockEntry> | block entries in payload order |
| `chunk_refs` | Vec<CompactChunkRef> | shared chunk reference table |

### `CompactFileEntry`

| Field | Type | Description |
| --- | --- | --- |
| `relative_path` | string | archive path using `/` separators |
| `size` | u64 | reconstructed file size |
| `mtime_ns` | i128 | Unix nanoseconds, advisory metadata |
| `permissions` | u32 | POSIX-style permissions where supported |
| `layout` | CompactFileLayout | reconstruction layout |

### `CompactFileLayout`

`Empty`:

- file has zero bytes;
- no block payload is consumed.

`Inline`:

| Field | Type | Description |
| --- | --- | --- |
| `block_index` | u32 | index into `blocks` |
| `offset` | u64 | byte offset in decompressed block raw payload |
| `len` | u64 | number of bytes to copy |

`Chunked`:

| Field | Type | Description |
| --- | --- | --- |
| `first_chunk_ref` | u32 | start index into `chunk_refs` |
| `chunk_ref_count` | u32 | number of chunk refs |

### `CompactChunkRef`

| Field | Type | Description |
| --- | --- | --- |
| `block_index` | u32 | index into `blocks` |
| `file_offset` | u64 | destination offset in reconstructed file |
| `len` | u64 | chunk length |
| `chunk_hash` | [u8; 32] | BLAKE3 hash of chunk raw bytes |

### `CompactBlockEntry`

| Field | Type | Description |
| --- | --- | --- |
| `block_id` | [u8; 32] | BLAKE3 hash of compressed block bytes |
| `raw_size` | u64 | decompressed raw block size |
| `compressed_size` | u64 | compressed block byte length before protection |
| `payload_size` | u64 | stored payload length after protection |
| `nonce` | [u8; 12] | block protection nonce |
| `codec` | enum | currently `Zstd` |
| `level` | i8 | writer-selected zstd level, advisory |
| `kind` | BlockKind | batch, single, chunk, or solid |

## Block Kinds

HIGV2 defines four block kinds.

| Kind | Purpose | Reconstruction |
| --- | --- | --- |
| `Batch` | multiple small files in one raw block | copy each file slice from a shared raw block |
| `Single` | one file in one block | copy full raw block or inline slice |
| `Chunk` | one chunk of a large file | place raw chunk at `file_offset` |
| `Solid` | multiple related small files optimized as a group | same extraction semantics as `Batch` |

Block kind can guide diagnostics and future policy. Extraction semantics are determined by file layout and block payload bytes.

## Batch and Solid Raw Payloads

For batch-like blocks, raw payload is the concatenation of member file contents in manifest order. Each member file records its `offset` and `len`. Empty files MUST NOT require raw payload bytes.

Readers MUST verify that every inline range is within the corresponding raw block.

## Chunked Files

Chunked files are reconstructed by allocating a file-sized buffer and applying chunk references. A reader MUST verify:

- every referenced block exists;
- every chunk raw length equals `len`;
- every chunk BLAKE3 hash equals `chunk_hash`;
- every chunk destination range is within file bounds;
- chunks do not produce inconsistent final file length.

Writers SHOULD avoid overlapping chunk ranges. Readers MUST reject overlapping ranges if they would create ambiguous output.

## Block Payload Decoding

For each block in manifest order:

1. read `payload_size` bytes from the archive stream;
2. unprotect using password or no-encryption mode;
3. verify `BLAKE3(compressed_bytes) == block_id`;
4. zstd-decompress to exactly `raw_size` bytes;
5. store the raw block by `block_id` until all referencing files are verified.

Readers SHOULD impose implementation-specific maximum sizes to prevent resource exhaustion. Such limits MUST produce explicit errors rather than partial extraction.

## Path Semantics

Archive paths are relative logical paths.

Writers MUST:

- use `/` separators;
- not write absolute paths;
- not write `..` path components;
- not write platform device names or drive prefixes as portable paths;
- preserve deterministic file ordering.

Readers MUST reject:

- absolute paths;
- paths containing `..`;
- paths that escape the output directory after normalization;
- paths that require creating unsafe symlinks or device files.

Current HIGV2 public archives describe regular files. Directory entries may be inferred from file paths.

## Metadata Semantics

`mtime_ns` and `permissions` are advisory metadata. Readers SHOULD restore permissions where the platform supports them. Readers MAY ignore or clamp unsupported permission bits.

Metadata MUST NOT be used as a substitute for content authentication during extraction.

## Reader Error Handling

A conforming reader MUST fail the archive if any of the following occurs:

- unknown magic;
- unsupported version;
- unsupported flags;
- invalid KDF parameters;
- manifest authentication failure;
- manifest decompression or decoding failure;
- unsupported manifest schema;
- manifest root hash mismatch;
- unknown block codec;
- block authentication failure;
- block hash mismatch;
- block decompression failure;
- block raw size mismatch;
- file hash mismatch;
- unsafe archive path;
- inline or chunk range out of bounds;
- overwrite refusal when overwrite is not explicitly allowed;
- resource limit exceeded.

Readers MUST perform verification before writing trusted output. A safe implementation SHOULD verify all file contents into memory or temporary files before replacing final output paths.

## Compatibility Policy

HIGV2 is the current public portable format. Additive metadata extensions MAY be introduced only when older readers can safely ignore them. Any incompatible change to header layout, cryptographic interpretation, manifest semantics, path rules, or payload decoding MUST use a new archive magic/version such as HIGV3.

HIGV1 is legacy. Public readers MAY support HIGV1 extraction, but HIGV2 writers SHOULD NOT emit HIGV1 unless explicitly requested for compatibility.

## Security Boundary Statement

Publishing this format does not publish Hig's internal implementation. The following remain out of scope and should not be inferred from this document:

- daemon scheduling and task protocol;
- local cache key layout and cache maintenance;
- project watcher internals;
- desktop application command routing;
- benchmark corpus and harness internals;
- product-specific optimization strategy.

## Multilingual Summary

### 中文摘要

HIGV2 是 `.hig` 的公开可移植归档格式。它定义 magic、version、固定 header、manifest、block/chunk/batch 结构、zstd 压缩、BLAKE3 校验、Argon2id 密钥派生、ChaCha20-Poly1305 认证加密、路径安全、兼容策略和错误处理。本文档不公开 Hig CLI、Desktop、daemon、cache engine 或 project snapshot 的内部实现。

### 한국어 요약

HIGV2는 `.hig`의 공개 이식 가능 아카이브 형식입니다. 이 문서는 magic, version, fixed header, manifest, block/chunk/batch 구조, zstd 압축, BLAKE3 검증, Argon2id KDF, ChaCha20-Poly1305 인증 암호화, path safety, compatibility, error handling을 정의합니다. Hig CLI, Desktop, daemon, cache engine, project snapshot 내부 구현은 공개하지 않습니다.

### Deutsche Zusammenfassung

HIGV2 ist das oeffentliche portable `.hig`-Archivformat. Es definiert Magic, Version, Header, Manifest, Block/Chunk/Batch-Strukturen, zstd-Kompression, BLAKE3-Verifikation, Argon2id-Schluesselableitung, ChaCha20-Poly1305-authentifizierte Verschluesselung, Pfadsicherheit, Kompatibilitaet und Fehlerbehandlung. Interne Hig-Implementierungen bleiben ausserhalb dieses Dokuments.

### Русское резюме

HIGV2 является публичным переносимым форматом архивов `.hig`. Документ определяет magic, version, fixed header, manifest, структуры block/chunk/batch, zstd, BLAKE3, Argon2id, ChaCha20-Poly1305, правила безопасности путей, совместимость и обработку ошибок. Внутренняя реализация Hig CLI, Desktop, daemon, cache engine и project snapshot не раскрывается.

### 日本語概要

HIGV2 は `.hig` の公開 portable archive format です。この文書は magic、version、fixed header、manifest、block/chunk/batch、zstd、BLAKE3、Argon2id、ChaCha20-Poly1305、path safety、compatibility、error handling を定義します。Hig CLI、Desktop、daemon、cache engine、project snapshot の内部実装は公開しません。
