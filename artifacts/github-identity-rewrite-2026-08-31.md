# GitHub Public-History Identity Correction

Date: 2026-08-31

Repository: <https://github.com/Hydite/Hig>

## Scope

The 41 commits reachable from the `Public` branch incorrectly recorded the
internal GitLab identity `Kee <1-root@users.noreply.git.haokir.com>` as both
author and committer. The history was rewritten to use the GitHub-associated
identity `Yike Wang <283410442+Aiomx@users.noreply.github.com>`.

The correction changed commit metadata only. Every commit retained its
original tree, parent topology, subject and body, author date, and committer
date. The pre-rewrite and identity-only post-rewrite `Public` heads both point
to tree `277518768d2e91bc3b9d4df4d2d827a57db3377b`.

The separate `v1.9.4` lightweight tag was not rewritten because its commit
already used a GitHub noreply identity. The annotated `v1.10.0` tag retained
its target release content, timestamp, and message while its tagger identity
was corrected. The redundant lowercase `public` branch was removed; `Public`
is the sole public development branch.

## Principal revision mapping

| Purpose | Pre-correction revision | Identity-corrected revision |
|---|---|---|
| v1.10.0 release commit | `272a1e87f5211f5ccc5f70b881aee84926cc4806` | `2487341eeec3046a4e34f5d44f27f1636301952f` |
| Qualified cold-path implementation | `82543018e9baa4d5850835dd8664e09caf81209e` | `ca07031a49baa138faead466149d46c8c29815b6` |
| Repository watcher reconciliation | `56e18c3d57484e2b91205b5d2c52a8c39786fa01` | `7cc216864a1a377506c073ba8a5dc118d0847436` |
| Completion evidence head | `6dd9e3d142359ce947f6b49308d32d497edb1001` | `b05439a716715628fa0f6b73e5294ceefe3acf1e` |
| Annotated v1.10.0 tag object | `24878c831a6d570e4febca68be715785c6ad5f96` | `bd518f733fd7be5e0acd461a2738a880bda8c053` |

Historical GitHub Actions reports continue to identify the pre-correction
revision because that is the immutable revision executed by the runner. The
two release evidence documents record both the executed revision and its
content-identical identity-corrected equivalent. New validation runs execute
against the corrected `Public` history.
