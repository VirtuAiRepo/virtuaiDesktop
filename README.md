# VirtuAI Desktop

VirtuAI in a native window, plus the local connector: the app keeps the VirtuAI
runner going from the menu bar, so agents — from the web chat, Slack, Google
Chat or any other channel — can work on this computer, and it asks before
running anything its command guard does not clear.

```
VirtuAI.app (Tauri 2)
├── window ───────────► https://<your VirtuAI server>/   the server's own web chat
├── menu bar: status · Open · Pause · Working folder · Settings… · Quit
├── Settings window: server · working folder · connector   (⌘, on macOS)
└── child process: virtuai run --desktop                 the VirtuAI CLI, bundled
       ▲  stdin/stdout, one JSON line per message
       └── approval requests → native dialog "Allow / Deny"
```

## Download

From [Releases](../../releases/latest):

| System | File |
|---|---|
| macOS 11+, Apple silicon | `VirtuAI-<version>-macos-arm64.dmg`, signed and notarized by Apple |
| Linux x86_64 (Debian, Ubuntu) | `VirtuAI-<version>-linux-amd64.deb` |
| Linux x86_64 (others) | `VirtuAI-<version>-linux-x86_64.AppImage` |

The app asks for your VirtuAI server's address the first time, then shows its
sign-in. Agents work in a folder you choose (`~/virtuai` by default); the app
refuses the disk's root and any folder that holds its own data.

**Settings** (⌘, or VirtuAI → Settings… on macOS, the menu bar icon anywhere,
or the gear in the chat's title bar) changes the server, the working folder,
and pauses the connector. A new server needs its own sign-in, and its own
pairing: Settings → CLI → Connect in the chat.

On macOS, the first time the connector starts it reads its token from the
keychain, and macOS asks; choose **Always Allow**.

The browser tools drive the **Google Chrome** installed on the computer; without
it they fail and the rest works.

On Linux the window keeps the system's title bar; the menu bar icon needs
`libayatana-appindicator3` (installed with the `.deb`).

## Build it

You need Rust **1.90+** (rustup), Node 18+, and [`uv`](https://docs.astral.sh/uv/).
On macOS, the Xcode Command Line Tools; on Linux,
`libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev`.

```bash
npm install
npm run dev          # the app, using the virtuai CLI installed on this machine
npm run build        # the app with the CLI frozen inside: a .dmg, or .deb/.AppImage
```

- `npm run dev` finds the CLI as `$VIRTUAI_CLI`, then `~/.local/bin`,
  `/opt/homebrew/bin`, `/usr/local/bin`, then `PATH`. It needs a CLI with
  `virtuai run --desktop`: `uv tool install 'virtuai-cli[browser]'`.
- `npm run build` runs `runner/build.sh`, which freezes the CLI release pinned
  in [`runner/cli-version`](runner/cli-version) from PyPI with PyInstaller
  (`--onedir`), and checks the binary (`runner/smoke.sh`). To freeze a local
  checkout of the CLI instead: `VIRTUAI_CLI_SOURCE=/path/to/cli npm run build`.
- `npm run check:runner` drives the frozen runner end to end against a fake
  server (`-- --browser` also drives Chrome; not as root).
- Unsigned, a macOS build runs only on the machine that built it.

Settings live in `~/Library/Application Support/com.ucloudstore.virtuai.desktop/`
on macOS. The runner's token is in the system keychain.

## Release

**Actions → Release → Run workflow**, with the version (`0.2.0`): it tags the
commit `v0.2.0`. Pushing a tag does the same:

```bash
git tag v0.2.0 && git push origin v0.2.0
```

[`release.yml`](.github/workflows/release.yml) builds it:

- **macOS (Apple silicon):** signs the runner and the app with the Developer ID
  certificate, notarizes and staples both the app and the disk image, and
  checks the result with Gatekeeper.
- **Linux:** builds the `.deb` and the AppImage.

Once both have built, one job publishes them as the tag's Release. The app's
version is the tag's.

It needs these repository secrets:

| Secret | What |
|---|---|
| `APPLE_CERTIFICATE` | `base64 -i DeveloperID.p12` — the "Developer ID Application" certificate with its private key |
| `APPLE_CERTIFICATE_PASSWORD` | the password it was exported with |
| `APPLE_API_KEY_P8` | the contents of the App Store Connect **team** API key (`AuthKey_XXXX.p8`) |
| `APPLE_API_KEY` | its Key ID |
| `APPLE_API_ISSUER` | its Issuer ID |

A new CLI first: publish it to PyPI, set its version in `runner/cli-version`,
then tag the app.

## Tests

```bash
cargo test --manifest-path src-tauri/Cargo.toml   # runner protocol, settings
scripts/check-macos.sh                            # the macOS-only Rust, from Linux
```

[`ci.yml`](.github/workflows/ci.yml) runs the Rust tests on Linux and macOS for
every push and pull request.

## How it works

- **The window** loads the server's own web chat. The app grants that one origin
  two window permissions (drag, maximize) and nothing else. On macOS the page
  draws the title bar, so it follows the page's theme; `src-tauri/src/titlebar.rs`
  narrows AppKit's own title bar view to the traffic lights, or it would take
  every click there.
- **The connector** is `virtuai run --desktop --workdir <folder>`, supervised by
  the app (`src-tauri/src/runner.rs`). Messages run over its pipes: status lines
  on stdout prefixed `@@virtuai `, approval answers on stdin. When the app
  quits, the runner's stdin closes and it stops.
- **Pairing** happens in the web chat ("Connect this computer"): the page hands
  the app a one-time code through a `virtuai://pair?code=…` link the app
  intercepts, and the app runs `virtuai pair` with it.

`src-tauri/icons/` is generated from `src-tauri/icons/app-icon-source.png`
(`npx tauri icon src-tauri/icons/app-icon-source.png`).

## License

[Apache License 2.0](LICENSE). It covers this code, not the VirtuAI name or
logo.
