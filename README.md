# GMO TempTools

One Windows 11 desktop app for temporary mail, SMS and OTP, plus a matching
web app. There are no separate Mail, SMS or OTP applications to install:
`dist/gmo-temptools.exe` is the desktop product. Version **0.3.0** is the single
version for the app and its tools.

## Hosted web version

The browser version is published with GitHub Pages from this repository's
`docs/` folder:

- **App:** <https://drifter1987.github.io/gmo-temp-tools/> (the Pages URL after
the repository is renamed from `gmo-temp-email`).
- **API:** <https://gmo-temptools-web-api-z1jsh3.freebuff.site>.
- **Repository:** <https://github.com/Drifter1987/gmo-temp-tools>.

GitHub Pages only serves static files, so the Rust desktop process cannot run
there. The hosted Mail/SMS API in `server/worker.js` runs on Freebuff Sites with
D1 storage, fetches upstream providers server-side (avoiding browser CORS), and
allows only the Pages origin. Mail defaults to Guerrilla Mail and falls back
there when Mail.tm is unavailable from the API host. OTP extraction and the TOTP
authenticator remain entirely in the browser and never call the API.

The hosted build has no account system. A random browser client ID scopes the
Mail/SMS metadata stored in D1, and **Settings → Clear all data** removes it.
Do not treat public temporary inboxes as private. Mailbox contents and upstream
provider availability are outside this app's control.

| Tab | What it does | Data source | Local API |
|---|---|---|---|
| **Mail** | Creates disposable addresses and shows their inboxes | Mail.tm, with Guerrilla Mail fallback in the hosted API | 8781 |
| **SMS (Beta)** | Browses public numbers by country and reads received texts | receive-sms.io public pages | 8782 |
| **OTP** | Extracts codes offline and runs a TOTP authenticator | none — stays local | — |
| **Settings** | Appearance, notifications, hosted access, release notes and cleanup | local files or hosted D1 | 8780 |

The two service APIs and the web app run inside the same process. Closing the
window hides it in the system tray; use **Quit GMO TempTools** in the tray menu
to stop the process. Background mail/SMS polling keeps working while the window
is hidden.

## Install and run

The generated release folder `dist/` has exactly two Windows executables:

- `dist/GMO-TempTools-Setup.exe` — the installer.
- `dist/gmo-temptools.exe` — the app itself.

Run `dist/GMO-TempTools-Setup.exe` once. It copies the app to
`%LOCALAPPDATA%\Programs\GMO TempTools`, creates Start Menu and desktop
shortcuts, and launches it. The installer and app target 64-bit Windows 11.
Only the 64-bit Windows 11 app and its installer are included.

For a development checkout:

```bash
npm ci                      # once, from package-lock.json
./scripts/build.sh          # compiles web assets and dist/gmo-temptools.exe
./scripts/run-all.sh        # opens the desktop window and web app
./scripts/run-all.sh token  # prints the local installation key
./scripts/run-all.sh status
./scripts/run-all.sh stop
```

`scripts/build.sh` uses `target/` by default. That folder is a disposable
compiler cache and is intentionally not part of the product; set
`CARGO_TARGET_DIR` elsewhere when you want to keep the checkout small. The
installer is built with:

```bash
./scripts/build-installer.sh
```

Dependencies are locked by `Cargo.lock` and `package-lock.json`. `npm ci`
reproduces the browser toolchain; `cargo update` applies compatible Rust
upgrades. Major upgrades are deliberate and must pass `./scripts/check.sh`.

The local desktop web app is <http://127.0.0.1:8780/>. Ports can be moved with
`TEMPTOOLS_WEB_PORT`, `TEMPTOOLS_EMAIL_PORT` and `TEMPTOOLS_SMS_PORT`.

To rebuild the static Pages bundle after changing `webapp/` or `web/ui.ts`:

```bash
npm ci
TEMPTOOLS_API_BASE=https://gmo-temptools-web-api-z1jsh3.freebuff.site \
  ./scripts/build-pages.sh
```

`scripts/build-pages.sh` typechecks the browser code, writes `docs/index.html`,
`docs/app.js`, `docs/app.css` and `docs/ui.js`, and injects the API origin into
the page. Commit `docs/` because Pages has no build step. The API itself is
`server/worker.js` plus `server/manifest.json`; deploy it with the Sites tooling
and keep its `DB` D1 binding. There are no API secrets in this repository.

## Project layout

The Rust crates form one Cargo workspace rooted at `Cargo.toml`. `scripts/`
contains the developer commands; `dist/` contains generated binaries.

| Path | Contents |
|---|---|
| `src/` | Unified desktop shell, tray and local web server |
| `temp-email/`, `temp-sms/`, `temp-otp/` | Feature crates and their local state folders |
| `temp-ui/` | Shared native palette and widgets |
| `local-auth/`, `webhook-sign/`, `desktop-notifications/` | Auth, carrier-signature and notification support |
| `web/`, `webapp/` | Shared browser helpers and the single-page web UI |
| `server/` | Serverless Mail/SMS API and Sites deployment manifest |
| `docs/` | Generated static GitHub Pages bundle |
| `installer/` | C# installer source |
| `scripts/` | Build, check and run commands |
| `dist/` | Generated app and installer (ignored) |
| `e2e/`, `tests/` | Playwright and HTTP integration tests |
| `temp-*/state/`, `settings.json` | Local runtime data (ignored) |

## Settings

Open **Settings** in the native window or the web app:

- **Appearance** — Dark or Light, saved on this device and applied to the
  desktop window immediately; the web app follows the same saved preference.
- **Desktop notifications** — enable or disable toasts, choose Mail and/or SMS,
  and send a test notification. OTP codes are never notified.
- **Installation key and access** — shows the key file location, can copy the
  key, and opens the key folder.
- **Local paths** — every data folder, the OTP vault, settings, logs and key
  path, each with a Copy button.
- **Release notes and changelog** — a history for each integrated tool (Mail,
  SMS and OTP), kept in the app rather than in separate documents.
- **Clear all data** — deletes Mail and SMS state, the OTP vault, settings, the
  installation key and logs after an explicit confirmation.
  Restart afterward so a fresh installation key is created.

## Installation key and access

The key is a **per-install token**, generated locally on first launch and stored
at `.run/auth-token` beside the executable (the installer's app folder when
installed). It unlocks only this PC's local web app and local APIs. It is not
an account, subscription, activation code or remote license.

- A user gets their key from **Settings → Installation key and access**, or from
  the key file shown there. In a development checkout, `./scripts/run-all.sh token`
  prints the same value.
- Each installation creates its own key. Installing on another PC does not
  share or copy the first PC's key.
- **Owner `drifter1987`** owns this build and can distribute the installer, but
  this build has no central key issuer, user database, entitlement check or
  revocation service. There is nothing in it that lets the owner remotely
  create or revoke a user's access.
- To give someone access today: send them `dist/GMO-TempTools-Setup.exe`, let them
  run it, and tell them to open **Settings → Installation key and access**. You
  do not send them a key; their installation creates one locally. The key only
  unlocks their own browser UI. To operate a paid or centrally managed license,
  the project needs a separate issuer/backend (or an offline signed-license
  scheme), and that is a different feature from this local-only build. This
  README does not claim that such a service exists.

Never post the key in a public issue, URL, screenshot or shared command line.

## Security and carrier credentials

The **desktop** service APIs require the installation token. Browser mutations
also require a same-origin request and the `X-TempTools-CSRF: 1` header; the
login cookie is `HttpOnly` and `SameSite=Strict`. The service ports accept
signed carrier deliveries only on their `/api/inbound/...` routes, verified by
the carrier's signature. There are no default webhook credentials: an unset
contract answers 503 rather than accepting unsigned data.

The **hosted web API** is public and has no installation key. It scopes Mail/SMS
metadata by an `X-TempTools-Client` browser ID, allows the Pages origin through
CORS, and never stores OTP secrets. It does not expose carrier webhook routes.
This is a convenience boundary, not user authentication: anyone who obtains a
client ID can read that client's temporary Mail/SMS metadata. The IDs and data
expire with the inbox/number TTL, and **Clear all data** deletes them early.

```bash
TWILIO_AUTH_TOKEN=... TELNYX_PUBLIC_KEY=... WEBHOOK_SECRET=... ./scripts/run-all.sh
```

For a number you rent yourself, post its texts to
`http://127.0.0.1:8782/api/inbound/sms` or
`POST /api/inbound/<provider>`.

The desktop APIs listen on `127.0.0.1` only. Use a tunnel when a carrier must
reach the signed inbound routes; do not expose the desktop app publicly. The
hosted web API is intentionally public at the Sites URL above and is limited to
the Mail/SMS proxy routes described in `server/worker.js`.

If you open a service port directly in a browser (`8781` or `8782`), it
redirects the page to the authenticated app at `http://127.0.0.1:8780/`, where
the install-token login is available. The service `/api/...` routes remain
token-guarded and are never redirected into an unauthenticated response.

## Where data is stored

With the default installation:

- `%LOCALAPPDATA%\Programs\GMO TempTools\` — app, installer-created shortcuts
  target, and `.run\auth-token`.
- `temp-email\state\inboxes.json` and `acceptance.json` — mail inboxes and
  provider health.
- `temp-sms\state\numbers.json` — temporary numbers and texts.
- `temp-otp\state\vault.json` — DPAPI-encrypted for the current Windows user;
  writes are serialized with a cross-process lock.
- `settings.json` — appearance and notification preferences.
- `.run\logs\` — development launcher logs.

In a development checkout these paths sit under the repository root, so the
existing `temp-*/state/` folders are the app's data folders, not separate
applications. `TEMPTOOLS_STATE_DIR` can move the state root for isolated tests.

For the **hosted web app**, Mail/SMS inbox and number metadata lives in the
Sites D1 `DB` database, keyed by the browser client ID. OTP secrets and the
chosen appearance/notification preference stay in that browser's
`localStorage`. The hosted API does not persist message bodies beyond the
provider cache/TTL.

## Tests

```bash
./scripts/check.sh
PLAYWRIGHT_CHANNEL=msedge npm run test:e2e
```

`scripts/check.sh` typechecks the browser assets and runs the Rust unit and HTTP
integration tests for the shared libraries and the unified app. The Playwright
suite covers Mail, SMS, OTP and Settings through the real local web app.
The app must be running (`./scripts/run-all.sh`) for Playwright; the Rust
integration tests start their own transient listeners.

## Honest limits

- GitHub Pages hosts only the static UI. The hosted API is a separate service;
  the Rust desktop app is not available at the Pages URL.
- Upstream mail and SMS services are not ours. Rate limits and missing public
  numbers are reported, not replaced with invented data. Mail.tm may reject the
  hosted API host; the app then creates a Guerrilla Mail inbox and labels the
  fallback.
- The hosted web build is not an account system. Its client ID is not a
  password, and public SMS inboxes are shared with anyone who opens the same
  page; upstream availability varies. SMS is therefore labelled **Beta**.
- Carrier signature checks are tested with locally generated keys, not live
  carrier traffic.
- The app is local-only by design. It is not a remote account system, and the
  installation key is not a substitute for one.
