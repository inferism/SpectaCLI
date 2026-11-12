# SpectaCLI

A lightweight desktop app that runs all your installed AI coding CLIs side by side in one window, with per-agent RAM and status.

> Early development. Not usable yet. See [PLAN.md](PLAN.md).

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
