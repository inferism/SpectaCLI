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

## Not affiliated

SpectaCLI is an independent project. It is not affiliated with, endorsed by, or sponsored by Anthropic, OpenAI, Google, GitHub, or any other CLI vendor. It launches the official CLIs you have installed, unchanged. All product names are trademarks of their respective owners.

## License

See [LICENSE](LICENSE).
