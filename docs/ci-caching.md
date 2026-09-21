# Release CI caching

The release workflow also runs on `main` when dependency manifests, patches or
build settings change. To validate any branch manually, run **Release SSH
MountMate** with an empty `tag` and `publish: false`. Only an actual version tag
or an explicit publish request can reach the production signing environment.
Branch validation still builds the complete release set and exercises signing
with an ephemeral test key; it does not change an existing release.

Quality checks and the six native builds run in parallel. Both must succeed
before the release set is accepted. Windows and Linux prepare their embedded
helpers before the Rust tests and final release build, so GUI checks exercise
the shipping binary and packaging does not compile it a second time. The real
mount, upload, unmount, host trust, update and rollback checks remain enabled.

## Cache ownership and keys

Only branch runs on `main` without a tag override write caches. Tag and other
branch builds can restore the default branch caches but cannot create redundant
copies. A cache miss always falls back to a normal build.

* `mountmate-rust-v1-*`: the pinned Rust cache action retains dependency artifacts
  and Cargo downloads, excluding workspace crates and incremental artifacts.
  Quality and each runner/architecture have separate keys. The action hashes
  Rust toolchains, compiler environment, Cargo configuration and dependency
  manifests; it normalizes local package versions, so an application version
  bump alone does not invalidate the cache. On dependency changes, compatible
  previous results can be restored and Cargo rebuilds affected dependencies.
* `mountmate-rclone-v1-*`: exact keys cover the pinned source/hash and flags in
  the build script, every patch, verification script, composite action, runner,
  architecture, Go version/environment and native C compiler identity. No
  partial-key restore is permitted. Restored binaries must pass SHA-256,
  version, OS, architecture, Go version, cmount and Lustre-option checks. Invalid
  entries are rebuilt. The SFTP source tests run on a cache miss, and final
  package integration tests run on every build, including cache hits.

Signing jobs do not restore executable build caches. Connection profiles,
credentials, application caches and signing keys are never included. WinFsp and
Plink continue to use pinned downloads and verification.

## Storage and maintenance

After main builds finish, `cache-budget` measures all repository cache entries.
Above an 8 GiB target it deletes only `mountmate-*` entries, oldest access first
within each priority tier. Windows and Intel macOS Rust builds have priority
over other Rust builds; compact rclone binaries have the highest priority.
The job reports before/after bytes and deletions in its summary. It does not
change the repository's GitHub storage cap or enable paid capacity.

To invalidate caches after a build-policy change, bump the corresponding
`mountmate-*-v1` prefix. Suspected corrupt entries can also be deleted in the
repository's Actions cache UI. No application release is required to prewarm:

```sh
gh workflow run release.yml --ref main -f publish=false
```

Compare a cold run with a second manual run on the same commit. Check restore
hits, per-step timings and storage size rather than assuming every run has the
same runner speed or queue delay. Main caches expire under GitHub's normal
retention rules; an infrequent release may legitimately need a cold build.

## Measured validation (2026-09-21)

The baseline is the successful [v0.6.20 run](https://github.com/Stardust0831/ssh-mountmate/actions/runs/35581081955).
The [cold run](https://github.com/Stardust0831/ssh-mountmate/actions/runs/35594998953)
and [warm run](https://github.com/Stardust0831/ssh-mountmate/actions/runs/35598687369)
both used commit `1c39fb6` and passed quality, all six native package integrations
and the signed release-set exercise. Publishing was intentionally skipped.

| Job / scope | Before | Cold cache | Warm cache |
| --- | ---: | ---: | ---: |
| Complete validation, excluding publish | 38.4 min | 30.1 min | 16.5 min |
| Quality | 7.1 min | 7.6 min | 3.7 min |
| Windows x64 | 23.7 min | 22.1 min | 8.7 min |
| Windows ARM64 | 23.6 min | 25.0 min | 13.5 min |
| macOS Intel | 29.4 min | 28.7 min | 14.4 min |
| macOS ARM64 | 11.3 min | 16.0 min | 8.1 min |
| Linux x64 | 16.6 min | 15.4 min | 7.6 min |
| Linux ARM64 | 17.5 min | 17.7 min | 10.7 min |

Complete validation is measured from the first job starting to release-set
completion, including the original quality/build dependency and inter-job waits.
Individual rows include each job's setup, tests and cache handling. They cannot
be added because jobs run concurrently. Runner speed varies, so these are
observations from successful runs rather than a runtime guarantee.

The warm run reduced complete validation time by about 57%. Storage after the
cold run was 7.77 GiB, retaining all seven Rust caches and six current rclone
caches, plus two small rclone entries from the initial workflow debugging.
Windows/Linux packaging took 5–7 seconds after eliminating the second compile.
