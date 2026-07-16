# v0.4.2-alpha.1 deferred manual acceptance

Run these checks on a Windows host after installing the prerelease through the in-app updater.

## Custom mountpoints

1. Edit a connection and select a custom folder mountpoint on a fixed local NTFS volume. Confirm
   the editor reports that the generated child mountpoint is available and enables Save.
2. Repeat with a mapped network drive and a UNC parent. Confirm the editor reports the concrete
   unsupported-volume or UNC error and keeps Save disabled.
3. Leave the editor open, disconnect or reconnect the selected volume, and retry Save. Confirm the
   save-time preflight rejects a path whose availability changed.

## Interactive SSH

1. Mount a direct manual connection using Interactive shared SSH and a password, key passphrase,
   or 2FA prompt. Confirm all prompts and output stay in the app terminal.
2. While authentication is pending and for at least ten seconds after it succeeds, confirm no
   console windows flash and the main window remains movable and responsive.
3. Confirm the queued mount starts once, hiding and reopening the terminal keeps the session alive,
   and End session closes the Plink session without leaving repeated `plink -shareexists` processes.

## Text size and connection order

1. In Settings, preview Small, Standard, Large, and Extra large. Confirm the main window and an
   already-open interactive terminal update immediately; Cancel restores the saved size and Save
   persists it across restart.
2. At Extra large, resize the main window down to its minimum. Confirm the toolbar and search/sort
   controls wrap without clipped text, overlapping controls, or inaccessible buttons.
3. Confirm every named folder appears before Uncategorized in Saved order, Name, and Host modes.
