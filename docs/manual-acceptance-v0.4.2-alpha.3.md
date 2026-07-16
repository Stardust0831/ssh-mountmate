# v0.4.2-alpha.3 deferred Windows acceptance

1. Choose a custom folder on a normal local NTFS drive and confirm preflight succeeds. Repeat under
   a mapped network drive, a directory mounted by another filesystem, and a junction/reparse path;
   confirm saving is blocked with a specific unsupported-volume reason. Drive-letter mounts must
   remain available.
2. Trigger a failed mount and confirm no separate console or full-log window flashes. Confirm the
   connection card retains only the two-line error summary and View full log opens the durable
   in-app viewer.
3. Change a simple OpenSSH-config connection from OpenSSH to Interactive shared SSH. Confirm the
   selection is accepted and password/2FA completes in the app terminal. A profile containing
   ProxyJump or ProxyCommand must fail with the explicit OpenSSH-required message.
4. Use an encrypted modern OpenSSH private key. Confirm Plink does not print `Unable to use key
   file`; authentication should fall back to password/2FA. A PuTTY PPK file should still be passed
   to Plink. The first connection to an unknown host should retain Plink's host-key confirmation.
5. While the connection is mounted, confirm Retry and End session are disabled while Hide remains
   available. After unmounting, End must close the session. Retry after an exited or failed login
   must show a clean terminal without duplicated or corrupted previous content.
6. Select terminal text and verify Ctrl+Shift+C copies it; verify Ctrl+Shift+V pastes into the
   terminal. Confirm closing or hiding the terminal window does not exit SSH MountMate.
