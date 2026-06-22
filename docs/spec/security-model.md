# HIGV2 Security Model

Status: Public security model draft  
Format version: HIGV2, version `2`  
Normative language: English  
Last updated: 2026-06-22

## Language Index

- English: this document is the normative security model.
- 中文: 本文档描述 HIGV2 公开归档格式的安全边界，不公开产品内部实现。
- 한국어: 이 문서는 HIGV2 공개 아카이브 형식의 보안 경계를 설명하며 제품 내부 구현은 공개하지 않습니다.
- Deutsch: Dieses Dokument beschreibt die Sicherheitsgrenzen des HIGV2-Formats und nicht die interne Produktimplementierung.
- Русский: Этот документ описывает границы безопасности HIGV2, но не внутреннюю реализацию продукта.
- 日本語: この文書は HIGV2 format の security boundary を説明し、製品内部実装は公開しません。

## Abstract

The HIGV2 security model protects portable archive extraction through password-based key derivation, authenticated encryption, content-addressed verification, and strict path validation. Its primary goal is to ensure that a reader either reconstructs exactly the archive contents intended by the writer or fails without producing trusted partial output.

This security model applies to the `.hig` archive format. It does not define the security model of the Hig CLI implementation, desktop app, daemon, local cache, project watcher, release infrastructure, or benchmark harness.

## Security Goals

HIGV2 is designed to provide the following properties in password mode:

1. Confidentiality of manifest and file payloads against parties without the password.
2. Integrity and authenticity of the manifest and block payloads through AEAD.
3. End-to-end content verification through BLAKE3 hashes.
4. Detection of wrong passwords before trusted extraction.
5. Detection of tampered payloads.
6. Path traversal prevention during extraction.
7. Compatibility-safe rejection of unsupported versions and flags.

In no-encryption mode, HIGV2 provides structural and content verification but does not provide confidentiality or AEAD authentication.

## Non-Goals

HIGV2 does not claim:

- resistance to weak user passwords;
- deniability;
- hidden file names in no-encryption mode;
- protection after successful extraction to an untrusted filesystem;
- malware detection in archived file contents;
- safety for symlinks, devices, or special files;
- forward secrecy;
- multi-recipient public-key encryption;
- secure deletion of temporary files;
- protection of local cache or daemon state.

Implementations MAY add product-level controls for some of these areas, but those controls are outside the portable archive specification.

## Threat Model

### Attacker Capabilities

The attacker may:

- read, copy, truncate, or modify an archive file;
- provide a malicious archive to a reader;
- attempt path traversal through archive entries;
- guess archive passwords offline;
- replay old valid archives;
- attempt decompression bombs or resource exhaustion;
- attempt to exploit parser ambiguity.

### Trusted Inputs

A reader may trust only:

- the user-selected output directory;
- the user-provided password, if any;
- its own implementation limits and policy.

A reader MUST NOT trust:

- archive paths before validation;
- manifest metadata before authentication and decoding;
- block sizes before range and resource checks;
- file content before hash verification;
- mtime or permission metadata as proof of content identity.

## Cryptographic Profile

### Key Derivation

Password archives use Argon2id version 0x13 with:

- 16-byte random salt;
- 32-byte output key;
- memory, time, and parallelism parameters recorded in the archive header.

Readers MUST reject invalid KDF parameters. Readers SHOULD apply local policy limits to prevent hostile archives from requesting excessive memory or CPU.

### Authenticated Encryption

Password archives use ChaCha20-Poly1305 with:

- 32-byte key from Argon2id;
- 12-byte nonce per protected payload;
- independent protection for the manifest and each block payload.

Readers MUST authenticate before decompression or use of plaintext. If authentication fails, extraction MUST fail.

### Hashing

HIGV2 uses BLAKE3 for:

- compressed block identifiers;
- file content hashes;
- chunk hashes;
- manifest root hash.

BLAKE3 is used for integrity verification and content addressing. It is not encryption.

## Confidentiality Boundaries

### Password Mode

Password mode encrypts:

- manifest contents, including paths and file metadata;
- compressed block payloads.

Observers without the password can still see:

- archive file size;
- header magic and version;
- KDF parameters;
- encryption mode flag;
- manifest protected payload length;
- total archive length.

Depending on archive construction, observers may infer coarse size information from total archive size. Implementations that need stronger traffic-analysis resistance would require padding, which is not part of HIGV2.

### No-Encryption Mode

No-encryption mode does not provide confidentiality. It should be used only when compression and integrity checks are desired without secrecy.

## Integrity Model

HIGV2 uses layered integrity checks:

1. manifest protection;
2. block payload protection in password mode;
3. BLAKE3 hash of compressed block bytes;
4. zstd decompression size checks;
5. BLAKE3 hash of chunks or reconstructed files;
6. manifest root hash.

A reader MUST fail if any required check fails.

## Extraction Safety

### Path Validation

Readers MUST reject archive paths that:

- are absolute;
- contain `..` components;
- contain platform-specific drive roots or device paths;
- resolve outside the selected output directory;
- require special file creation not supported by the regular-file profile.

Readers SHOULD normalize separators to `/` for archive interpretation, then map to platform paths only after validation.

### Write Discipline

A safe reader SHOULD:

1. authenticate and decode the manifest;
2. authenticate, decompress, and verify all referenced blocks;
3. reconstruct and verify file contents;
4. write to temporary files or staging storage;
5. atomically move verified files into final locations when possible.

At minimum, a reader MUST NOT treat output as trusted until all corresponding verification succeeds. If extraction fails, a reader SHOULD remove partial output it created.

### Overwrite Policy

Readers SHOULD refuse to overwrite existing files by default. Overwrite MUST require explicit caller consent.

## Resource Exhaustion Controls

Readers SHOULD enforce limits for:

- maximum manifest length;
- maximum number of files;
- maximum number of blocks;
- maximum raw block size;
- maximum reconstructed file size;
- maximum total extracted bytes;
- maximum path length;
- maximum nesting depth;
- maximum Argon2id memory and time cost accepted by policy.

When a limit is exceeded, the reader SHOULD return a `resource_limit` style error and avoid partial trusted output.

## Parser Robustness

Readers MUST reject:

- unknown magic;
- unsupported versions;
- unknown header flags;
- unsupported manifest schemas;
- unknown codecs;
- out-of-range block or chunk indices;
- integer overflows in offset/length arithmetic;
- duplicate paths that would overwrite each other unless explicitly handled by policy;
- ambiguous overlapping chunk layouts.

Readers SHOULD parse using bounded allocation and checked arithmetic.

## Compatibility Security

A reader MUST NOT attempt to interpret an unsupported future version as HIGV2. If future HIGV3 changes cryptographic semantics or manifest layout, HIGV2 readers must fail closed.

Writers SHOULD use HIGV2 for current portable archives and SHOULD NOT silently downgrade to HIGV1 unless compatibility is explicitly requested.

## Public Disclosure Boundary

This public security model intentionally discloses:

- archive security goals;
- cryptographic primitives;
- KDF and AEAD usage;
- content verification rules;
- path safety requirements;
- extraction failure requirements.

It intentionally does not disclose:

- CLI internal architecture;
- daemon task protocol;
- cache engine internals;
- project watcher implementation;
- benchmark corpus or private performance traces;
- release signing material;
- product roadmap or proprietary optimization techniques.

## Recommended Independent Reader Policy

An independent reader should implement the following default policy:

| Area | Recommended default |
| --- | --- |
| unknown magic/version | reject |
| unknown flags | reject |
| unsupported codec | reject |
| password missing | reject password archive |
| wrong password | reject before output |
| unsafe path | reject entire archive |
| existing output path | refuse unless overwrite enabled |
| no-encryption archive | allow only after caller accepts no confidentiality |
| resource limits | fail closed |
| symlink/device entries | reject unless a future profile defines them |

## Security Review Checklist

Before publishing a HIGV2 reader or writer, review:

- KDF parameters are parsed with sane limits.
- AEAD failures cannot be ignored.
- Nonces are unique per protected payload within an archive.
- Manifest and block sizes use checked arithmetic.
- zstd decompression is bounded by expected raw size.
- BLAKE3 checks are enforced.
- Path validation occurs before writing.
- Temporary files are not trusted until verification completes.
- Error messages do not reveal passwords or derived keys.
- Test vectors include positive and negative cases.

## Multilingual Summary

### 中文摘要

HIGV2 的安全模型依赖 Argon2id、ChaCha20-Poly1305、BLAKE3 和严格路径校验。密码模式提供 manifest 和 payload 的保密性与认证完整性；无加密模式只提供压缩和哈希校验，不提供保密性。reader 必须在认证、解压、hash 校验和路径校验都成功后，才把输出视为可信文件。

### 한국어 요약

HIGV2 보안 모델은 Argon2id, ChaCha20-Poly1305, BLAKE3, 엄격한 path validation에 의존합니다. Password mode는 manifest와 payload의 confidentiality 및 authenticated integrity를 제공합니다. No-encryption mode는 confidentiality를 제공하지 않습니다. Reader는 인증, 압축 해제, hash 검증, path 검증이 모두 성공한 후에만 output을 신뢰해야 합니다.

### Deutsche Zusammenfassung

Das HIGV2-Sicherheitsmodell basiert auf Argon2id, ChaCha20-Poly1305, BLAKE3 und strenger Pfadvalidierung. Der Passwortmodus bietet Vertraulichkeit und authentifizierte Integritaet fuer Manifest und Payloads. Der unverschluesselte Modus bietet keine Vertraulichkeit. Reader duerfen Ausgaben erst nach erfolgreicher Authentifizierung, Dekompression, Hash-Pruefung und Pfadvalidierung als vertrauenswuerdig behandeln.

### Русское резюме

Модель безопасности HIGV2 основана на Argon2id, ChaCha20-Poly1305, BLAKE3 и строгой проверке путей. Password mode обеспечивает конфиденциальность и аутентифицированную целостность manifest и payload. No-encryption mode не обеспечивает конфиденциальность. Reader должен считать output доверенным только после успешной аутентификации, распаковки, hash verification и проверки путей.

### 日本語概要

HIGV2 security model は Argon2id、ChaCha20-Poly1305、BLAKE3、厳格な path validation に基づきます。Password mode は manifest と payload の confidentiality と authenticated integrity を提供します。No-encryption mode は confidentiality を提供しません。Reader は authentication、decompression、hash verification、path validation がすべて成功した後にのみ output を trusted として扱うべきです。
