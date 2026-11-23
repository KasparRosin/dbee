# Security policy

dbee holds database passwords and SSH keys and connects to your servers, so reports of vulnerabilities are welcome.

## Reporting a vulnerability

Report it privately, in either of these ways:

- through GitHub's private vulnerability reporting on this repository (the **Security** tab, **Report a vulnerability**);
- by email to kaspar@rosin.dev.

Please don't open a public issue, pull request or discussion for a vulnerability. Say what you found, how to reproduce it, and which commit or version you tested. You'll get a reply in private, and the fix and its disclosure are agreed with you.

## Supported versions

dbee is pre-alpha, so there is no table of supported versions: fixes land on `main`.

## What dbee sends by itself

One thing: a check for a newer release. A few seconds after it starts, and once a day while it is open, an installed copy asks GitHub for the newest release's checksum file (`https://github.com/KasparRosin/dbee/releases/latest/download/SHA256SUMS.txt`). GitHub sees the machine's network address, as with any request, and `User-Agent: dbee/<version>`. Nothing else is sent: no id, no cookie, nothing about the machine, its connections or its databases. The answer is kept for six hours, so restarts within that time ask nobody. dbee installs nothing by itself: the button the check can show opens the release's page in the browser.

To turn the check off, set `DBEE_NO_UPDATE_CHECK=1` in dbee's environment, or put `"checkForUpdates": false` in `settings.json` in the settings folder (the command `app.setUpdateCheck` writes it). A copy run from a build folder, with `DBEE_NO_PREFS`, or by `cargo xtask run-isolated` never checks.
