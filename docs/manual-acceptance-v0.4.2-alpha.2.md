# v0.4.2-alpha.2 deferred Windows acceptance

1. At every text-size preset, confirm the batch mount buttons and connection search/sort controls
   have normal compact vertical spacing. Resize narrowly and confirm both responsive rows wrap.
2. Mount with OpenSSH transport and confirm no system console appears before, during, or after the
   mount. Repeat a failed mount and confirm only the main-view error tail remains; use View log to
   open the persistent in-app log explicitly.
3. Import a simple SSH config profile containing HostName, User, and Port but no IdentityFile.
   Select Interactive shared SSH, complete password and 2FA in the app terminal, then send any
   final Enter required by the remote prompt. Confirm the queued mount starts after the shared
   session becomes ready and no Plink console appears.
4. End and retry an interactive session. Confirm the session becomes Exited/Failed as appropriate,
   the main application remains running, and Retry creates a fresh terminal. If the process exits,
   capture the app trace through the existing `SSH_MOUNTMATE_TRACE_FILE` diagnostic environment.
