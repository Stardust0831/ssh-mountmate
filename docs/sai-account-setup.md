# SAI account setup adapter

MountMate checks the service's versioned capabilities before starting login.
Directory discovery is an optional extension to the existing connection and device-key
flow. Services without that extension retain manual mount-path configuration.

## Client boundary

`mountmate-sai` owns discovery, device authorization, permitted connections, public-key
registration and directory discovery. The app's `sai_setup.rs` owns temporary editor state.
For initial setup, the adapter fills an ordinary `ConnectionDraft` only after the selected
site reports `applied`. Existing validation, save, host-key review and mount handling remain
responsible for the connection. The saved connection schema stays unchanged.

Existing SAI connections expose directory selection beside their mount-point controls.
This uses the same device authorization and directory picker with only
`openid sai.ssh.read sai.ssh.mounts.read`. The returned authorized SSH host, port and
Unix username must match the existing editor target. Multiple matching sites require an
explicit choice. Only selected directory mappings are appended; existing credentials,
local destinations and active mappings retain their current ownership and save guards.
This flow never generates or enrolls a key, and tokens remain temporary editor state.

The first version uses Native SFTP and a dedicated local Ed25519 key, without writing SSH
config entries or requiring an SSH CA. Keys live under the app data directory at
`sai-keys/<local hash>/id_ed25519`, using existing Unix permissions (0700 directory, 0600 file)
and Windows ACL helpers. Account/site identifiers never become literal filenames. Private
keys are never uploaded or downloaded. Access tokens stay in memory during setup and are
dropped on completion, cancellation or leaving the editor. No offline access, refresh-token
cache or credential sharing with another client is used. Expired authorization requires a
new sign-in. Ordinary subsequent mounts use the SSH key.

Cancelling an HTTP request cannot undo a registration already accepted by the service.
Deleting a connection also does not revoke the remote public key: use the SAI console to
remove it. Local keys remain available for retry and connections that reference them.

Only a `key_revoked` or `key_expired` result offers **Authorize with a new key**. This
explicit action rechecks the current operation's terminal state, writes a new private key
to `id_ed25519.<new request ID>` in the existing account/site directory, then atomically
updates its `current-key` selector under the existing lock. Older private files and saved
connection paths stay intact. Retries, cancellation and sign-in after restart use the selected
key and request ID, including after an ambiguous registration response. A stale recovery
action resumes an already replaced current key rather than rotating it again. Invalid or
missing selected-key data fails closed instead of falling back to a retired key.

Production trust roots are Portal `https://www.open-sai.com` and Identity issuer
`https://id.open-sai.com/realms/sai`. There is no production HTTP/issuer override. HTTP keeps
normal system proxy behavior, TLS validation, no redirects, 10-second connect and 20-second
request timeouts, and a 128 KiB response limit for login and key operations. Directory
pages have a separate bounded 2 MiB limit. Discovery issuer must match exactly;
token/device/verification URLs must share the trusted issuer origin, without URL credentials
or fragments. Errors exclude response bodies and OAuth secrets. The browser receives the
plain verification URL; users enter the displayed code there.

## Proposed Identity registration

Register public client `ssh-mountmate` for standard Device Authorization (RFC 8628), without
client secret or password grant. Browser login, MFA, consent and sensitive-action policy
belong to Identity. Requested scopes are `openid sai.ssh.read sai.ssh.keys.write`; resource
and audience are `sai-portal`. Do not grant unrelated billing, compute/storage or offline scopes.
When the service advertises `mount_roots=ssh.mount-roots/v1`, the same device authorization
also requests optional `sai.ssh.mounts.read`. This consent permits directory names, paths
and read/write metadata. Existing SSH grants alone do not permit directory discovery.

Agree on a Portal-compatible subject sector and explicit authorized-party allowlist. Portal
binds the verified `(iss, sub)` to its account; MountMate does not derive Unix usernames from
email, nickname or subject. Send `client_id` and `resource` to device and token endpoints,
and scopes to the device endpoint. The OAuth library handles pending and `slow_down`; login
is bounded by the advertised expiry and 15 minutes. Granted scopes, when supplied, must
contain the three basic scopes. Missing directory consent leaves ordinary setup available
and reports directory access as unauthorized. Tokens must be Bearer with positive `expires_in`. Portal owns
signature, issuer, audience, azp, expiry, scope and account-binding verification.

## Proposed Portal v1 contract

Paths are relative to the fixed Portal origin. All responses use JSON; additive fields are
allowed. No response supplies executable commands, private keys or credential forwarding URLs.

### Public capability

`GET /v1/auth/config?client=ssh-mountmate`, without token:

```json
{
  "schema_version": "ssh-mountmate.connection/v1",
  "identity_issuer": "https://id.open-sai.com/realms/sai",
  "oidc_client_id": "ssh-mountmate",
  "oidc_resource": "sai-portal",
  "ssh_access": "public-key/v2"
}
```

Publish only after client registration, APIs and site application are ready. 400/404/501/503
mean unavailable; mismatched or malformed configuration stops before login.
`public-key/v2` includes the exact browser confirmation below. The client also
accepts the earlier `public-key/v1` contract, which cannot supply a browser confirmation
URL. Older clients reject v2 capability before starting login.

### Allowed connections

`GET /v1/account/ssh/connections`, Bearer token with `sai.ssh.read`:

```json
{
  "schema_version": "ssh.connections/v1",
  "account_id": "opaque-stable-portal-account-id",
  "connections": [
    {"site_id": "example-site", "name": "Example cluster", "host": "ssh.example.org", "port": 12022, "username": "example_user"}
  ]
}
```

Return at most 100 sites with current SSH access for this actor. Empty lists are valid.
`account_id` is stable and opaque, scoped to this Portal, used for local key reuse (1–255
UTF-8 bytes, nonblank, no control characters). Site IDs are unique; `site_id`/`username` are
1–128 ASCII letters/digits/underscore/hyphen/dot without a leading hyphen. Host is a DNS name
or IP address; port is 1–65535; name is 1–128 UTF-8 bytes, nonblank, without controls. Do not
substitute query-gateway metadata. Recheck access when registering a key.

### Optional mount directories

The public capability may include `"mount_roots":"ssh.mount-roots/v1"` after the matching
Identity scope and Portal API are available. Unknown or absent directory capability keeps
manual paths available and does not request the additional scope.

`GET /v1/account/ssh/mount-roots?site_id=example-site&limit=20` uses the same trusted Portal
origin and a Bearer token with `sai.ssh.mounts.read`. A page contains:

```json
{
  "schema_version": "ssh.mount-roots/v1",
  "site_id": "example-site",
  "items": [
    {"id":"opaque-directory-id", "display_name":"Shared results", "kind":"org_share", "path":"/data/shared results", "role":"read"}
  ],
  "next_cursor": null
}
```

Directory kinds are `home`, `org_share`, `org_user_dir` and `project_root`; roles are
`read` or `write`. Paths are server-resolved absolute paths visible through the selected
SSH endpoint, including exact invited subdirectories. Names and paths may contain Unicode
and spaces. The client never constructs them from usernames or organization identifiers.

Each page is bound to the selected site and validates its unique IDs, canonical paths and
known roles. The client requests 20 candidates and allows up to 2 MiB, enough for the
maximum Unicode names and paths, including JSON escapes. `next_cursor` is an opaque value
of at most 101 characters, passed back only to this
endpoint. An empty page can still have a cursor; **More directories** loads the next page.
Only a null cursor ends the list. Denial, expiry, unavailable service and invalid responses
are shown as errors, preserving manual path entry.

Pages retain the server's order: Home, shared folders, projects and member folders, with
stable paths within those groups. The client does not fetch every page to construct an
initial list. Cursors remain opaque and at most 101 characters. A `400` response with
`detail.code=invalid_mount_roots_cursor` invalidates the temporary list and its unsubmitted
selection. **Reload directories** starts with no cursor; saved and draft mappings remain
unchanged. Ordinary continuation pages retain the current selection.

The setup panel lets users select several directories and shows their read/write state.
During initial setup, explicit choices are applied after key enrollment succeeds.
Adding directories to an existing connection requires only directory authorization.
A fresh unused primary mapping receives the first choice; further choices append ordinary
mappings without overwriting existing paths or local destinations. Existing paths are not duplicated.
Selections respect the existing editor's remaining capacity. Users finish local destinations
and save through the ordinary 64-mapping validation. Remote path whitespace and literal
backslashes remain unchanged through saving, mount targets and capacity queries.
Selection provides no new permission; SFTP and the remote filesystem enforce actual access.

### Register this device

`POST /v1/account/ssh/keys`, Bearer token with `sai.ssh.keys.write`:

```json
{
  "request_id": "64-lowercase-hex-digits",
  "site_id": "example-site",
  "public_key": "ssh-ed25519 <OpenSSH public key base64>",
  "label": "SSH MountMate"
}
```

`request_id` is a SHA-256 digest of local account/site context and the public key. It is an
idempotency identifier, not authorization. Bind it to the verified actor, site and exact key;
repeated identical requests return the original operation, conflicting content is rejected.
Preserve deduplication while the key/operation is active. All reads/writes are actor-bound.
Reuse the canonical SSH-key writer and asynchronous site provisioning path.

POST (200/201/202) and `GET /v1/account/ssh/keys/{request_id}` (requiring `sai.ssh.read`)
return the same structure:

```json
{
  "request_id": "64-lowercase-hex-digits",
  "site_id": "example-site",
  "public_key_fingerprint": "SHA256:<OpenSSH SHA-256 fingerprint>",
  "state": "pending"
}
```

States are `pending`, `applied`, `failed`. `applied` must reflect the selected site's current
confirmation for this actor/key; queuing a task is only `pending`. The client polls every two
seconds for at most two minutes and verifies request/site/fingerprint every time. Pending,
failed, mismatched or unknown states never produce a connection. Timeout/network failure
retains the local key; explicit retry repeats the same request, also after restart and sign-in
to the same account. Expired authorization requires sign-in again.

### Browser confirmation (`public-key/v2`)

Before creating or applying the key, a pending operation may additionally return:

```json
{
  "reason": "verification_required",
  "verification_url": "https://www.open-sai.com/portal/#/portal/ssh-device?request_id=<the-exact-request-id>"
}
```

The ordinary request, site, fingerprint and state fields remain present. The browser must
authenticate the same actor, show the immutable site and fingerprint, and complete the
existing human-verification policy before the canonical key writer runs. This is a product
action confirmation, separate from OAuth device authorization.

MountMate accepts only this exact fixed-origin route and its current request ID, with no
extra query or fragment fields. It opens the verified address once per explicit setup
attempt, offers a manual reopen, and continues polling the original operation. It never
sends a bearer token to that browser URL. Pending responses after confirmation omit the URL
and reason; only `applied` completes setup. Cancellation stops polling without deleting the
local key or undoing an already accepted operation.

Failed operations may use `approval_expired`, `key_revoked`, `key_expired` or `sync_failed`.
Unknown reasons and incompatible state/URL combinations are rejected. If approval expires
before a key or site side effect exists, an explicit POST retry can reopen confirmation for
the same immutable actor/site/key/request after current eligibility is checked. GET does not
renew approval. An already created, revoked or expired key cannot be resurrected by retrying
its old operation. MountMate does not overwrite a private key referenced by saved connections.
The explicit new-key action creates a new operation which still requires current access,
fresh browser confirmation and the site's `applied` result.

Return 401 for invalid/expired authorization, 403 for missing permission or required account
action. Remote key revocation continues to use the existing SAI console. Enable v2 capability
only after the matching browser page, API and site-application chain are deployed together.

## Validation and rollout

Local HTTP fixtures exercise login through ordinary draft validation, polling/denial/expiry,
cancellation, issuer/endpoint mismatch, redirects, response limits, credential destinations,
site application states, durable retry and private-key protection. Editor generations reject
late results; save is disabled during setup and existing mounted/source/saving guards apply.
The optional `generated_key_reaches_native_sftp_after_browser_approval` test uses a local
synthetic authorization service and a loopback rclone SFTP server. With
`SSH_MOUNTMATE_TEST_RCLONE` set to a verified binary, it checks the generated key through
ordinary connection validation, the real host-trust prompt, byte-preserving upload/readback,
wrong-host-key rejection and wrong-user-key rejection. It does not require FUSE and is not
evidence of a real SAI login, site enrollment or filesystem mount.

Before enabling the service, separately verify client registration, consent/scopes/audience/
subject binding, cross-account denial, permission revocation, deduplication, real site
callbacks, remote key removal, Windows/macOS file protection and actual SFTP mounting.
Fixture tests and a network-failure GUI check do not establish live authorization.
