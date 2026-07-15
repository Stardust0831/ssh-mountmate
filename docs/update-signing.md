# Signed update trust and key operations

SSH MountMate update manifests use Ed25519. The application embeds only public keys from
`distribution/update-public-keys.json`; a signed manifest identifies one trusted `key_id` and
binds the version, channel, and the name, byte size, and SHA-256 of every canonical platform ZIP.
The updater also requires GitHub's REST API digest, size, asset name, and download URL to match the
signed values. A GitHub digest is an additional consistency check and never substitutes for the
Ed25519 signature.

## Production custody

The active PKCS#8 private key is stored only as the protected GitHub Environment secret
`UPDATE_ED25519_PRIVATE_KEY_PKCS8_B64` in `production-update-signing`. Its matching environment
variable is `UPDATE_ED25519_KEY_ID`. The environment must require owner approval and restrict
deployment to approved `v*` tags. The release workflow builds and tests the signing program before
the private-key step, signs through stdin, and creates a draft Release. GitHub gives draft assets an
`untagged-*` download URL, so the workflow first verifies the real draft sizes and REST digests with
the deterministic canonical URL each asset will receive on publication. It then publishes, verifies
the actual public REST metadata with the same client-side verifier, and automatically restores the
Release to draft if that post-publication check fails.

The chosen custody policy deliberately has **one GitHub secret and no offline backup**. This avoids
copy proliferation but is high risk: accidental deletion or loss of Environment access destroys
the only signing key. Existing clients that trust only that key could then be stranded unless they
are manually updated. GitHub does not automatically rotate arbitrary Ed25519 secrets and cannot
recover the plaintext value after storage.

Generate a production key only on a controlled maintainer machine. The command refuses to print a
private key to a terminal; pipe it directly to the Environment secret store:

```bash
set -o pipefail
cargo run --release -p mountmate-core --bin update-signing -- \
  generate distribution/update-public-keys.json \
  | gh secret set UPDATE_ED25519_PRIVATE_KEY_PKCS8_B64 \
      --env production-update-signing --repo Stardust0831/ssh-mountmate

key_id="$(jq -r '.keys[0].key_id' distribution/update-public-keys.json)"
gh variable set UPDATE_ED25519_KEY_ID --body "${key_id}" \
  --env production-update-signing --repo Stardust0831/ssh-mountmate
```

Never pass the private key as a command-line argument, enable shell tracing around the signing
step, print the environment, upload runner diagnostics containing process environments, or commit
temporary key material. Test and dry-run jobs use a new ephemeral test key and never receive the
production Environment secret.

## Rotation

The client accepts multiple public keys, but each manifest has one signature. Rotation therefore
requires a bridge release:

1. Generate the next key and add its public record alongside the current public key.
2. Publish a bridge version signed by the current key, so updated clients trust both keys.
3. After a documented adoption period, replace the Environment secret and active `key_id` with the
   next key.
4. Publish with the next key. Keep the prior public key in clients while supported older releases
   and rollback paths may still need it.

Deleting the current key before a bridge release or silently changing a `key_id` strands existing
clients. The key ID is the `ed25519-` prefix plus the first 16 lowercase hexadecimal characters of
SHA-256 over the raw 32-byte public key; the full public-key SHA-256 must be recorded and confirmed
out of band before the first production tag is published.

## First-trust and remaining limits

Versions through v0.4.0 do not contain the Ed25519 trust root. Their first update to a signed build
is still authenticated only by the older GitHub metadata/SHA-256 mechanism; an attacker controlling
that bootstrap path could substitute both the program and its embedded public key. Users needing a
strong first trust should manually verify the first signed package and public-key fingerprint from
an independent channel. Once a signed build is installed, invalid or unsigned later releases are
visible but cannot be installed automatically.

The application packages themselves are still not backed by Windows Authenticode or Apple
Developer ID/notarization. Ed25519 protects the SSH MountMate updater path, not downloads installed
manually through a browser. Current Releases also provide no canonical Windows/Linux onedir update
asset; those directory layouts remain manual-update only.

## Initial production trust root

Pending explicit owner confirmation before the first tag:

- `key_id`: `ed25519-563e14d2c6b880f9`
- SHA-256 of the raw 32-byte public key:
  `563e14d2c6b880f9326f71c809a49474ec74cf74ca2347cc5ac3bf6efad27a2a`

The private key was piped directly into the protected Environment secret and has no repository or
local-file copy. Under the selected no-backup policy, the GitHub secret is the only copy.
