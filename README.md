# SpectaCLI

A lightweight desktop app that runs all your installed AI coding CLIs side by side in one window, with per-agent RAM and status.

> Early development. Not usable yet. See [PLAN.md](PLAN.md).

## Install

Download the `.exe` (per-user install, no admin needed) or the `.msi` (for managed environments) from the [latest release](https://github.com/inferism/Spectacli/releases/latest).

The installers are not code-signed yet, so Windows SmartScreen may show "Windows protected your PC". To continue, click **More info**, then **Run anyway**. To check your download first, compare its hash with `SHA256SUMS.txt` on the release page:

```powershell
Get-FileHash .\SpectaCLI_0.1.0_x64-setup.exe -Algorithm SHA256
```

There is no auto-update yet: to update, install the newer release over the old one.

## Development

Requires Node.js, Rust (MSVC toolchain), and WebView2 (included in Windows 11).

```bash
npm install
npm run tauri dev
```

Checks: `npm run typecheck`, `npm test`, `cargo clippy` and `cargo test` in `src-tauri`.

Optional newer ConPTY: `npm run conpty -- --dev` puts Microsoft's `conpty.dll` and `x64/OpenConsole.exe` next to the debug exe, and `npm run build:conpty` bundles them into the installer. Without them the app uses the ConPTY built into Windows.

Panics are logged to `%APPDATA%\dev.spectacli.app\logs\panic-<date>.log` (the newest 5 days are kept).

## Not affiliated

SpectaCLI is an independent project. It is not affiliated with, endorsed by, or sponsored by Anthropic, OpenAI, Google, GitHub, or any other CLI vendor. It launches the official CLIs you have installed, unchanged. All product names are trademarks of their respective owners.

## License

See [LICENSE](LICENSE).
