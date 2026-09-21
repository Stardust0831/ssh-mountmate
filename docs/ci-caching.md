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
