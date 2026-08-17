# dsh-tauri-shell (PoC)

Tauri desktop shell for DeepSeek Harness. This is a **proof of concept** that
validates the Tauri surface proposed in
[.agents/notes/proposed/architecture/2026-08-15-tauri-surface.md](../../.agents/notes/proposed/architecture/2026-08-15-tauri-surface.md).

## What it does

The Rust shell spawns the existing `dsh web` profile as a Node sidecar with
`--port 0` (OS-assigned free port), parses the `dsh web: http://127.0.0.1:<port>`
stdout line, and navigates the OS WebView (WebView2 / WKWebView / WebKitGTK) to
that URL. The full React UI from `client/ui-*` loads unchanged. When the window
closes, the sidecar is killed.

This avoids the browser carrier's HTTP marshal cost and leaves a path for native
OS features to register as DSH seams in later iterations.

## Layout

```
apps/tauri/
├── package.json              # @deepseek-ai/dsh-tauri-shell (private, @tauri-apps/cli devDep)
├── ui/index.html             # placeholder loading page (spinner until sidecar URL arrives)
└── src-tauri/
    ├── Cargo.toml            # dsh-tauri-shell crate
    ├── tauri.conf.json       # window config, identifier, frontendDist=../ui
    ├── src/lib.rs            # sidecar spawn + URL parse + eval navigation + cleanup
    ├── src/main.rs           # windows_subsystem entry
    └── capabilities/default.json
```

## App icon

The app icon is the DeepSeek fish logo (the same path as
`website/public/favicon.svg`, fill `#4D6BFE`) on a white background. All
platform icons under `src-tauri/icons/` (`icon.png`, `icon.ico`, `icon.icns`,
the Windows `Square*Logo.png` set, and the iOS/Android trees) are generated
from a single 1024×1024 PNG source via `tauri icon`. Regenerate by placing a
new `icon-source.png` next to this README and running:

```sh
cd apps/tauri && pnpm tauri icon ../apps/tauri/icon-source.png
```

## Prerequisites

- Rust toolchain (`rustc`/`cargo`) — tested with 1.94.0
- Node.js ≥ 22.19 and pnpm (for the `dsh web` sidecar)
- Tauri system dependencies:
  - **Windows**: WebView2 (preinstalled on Win10/11) + MSVC build tools
  - **macOS**: Xcode Command Line Tools (WKWebView ships with macOS)
  - **Linux**: `webkit2gtk-4.1`, `librsvg`, `libgtk-3`, and related deps
- `DEEPSEEK_API_KEY` in the repo root `.env` (required for a full conversation;
  the shell itself launches without it, but the sidecar's LLM calls will fail)

## Build

```sh
# debug binary
cd apps/tauri/src-tauri && cargo build

# release binary
cd apps/tauri/src-tauri && cargo build --release

# full installer bundle (MSI/NSIS on Windows, .app on macOS, .deb/.AppImage on Linux)
cd apps/tauri && pnpm tauri build
```

`cargo build` and `cargo build --release` have been verified on Windows. The
full `tauri build` installer bundle is not exercised in CI for this PoC.

## Run

From the repo root:

```sh
cd apps/tauri && pnpm tauri dev
```

or run the built binary directly:

```sh
./apps/tauri/src-tauri/target/release/dsh-tauri-shell.exe
```

A window titled "DeepSeek Harness" opens showing a loading spinner, then
navigates to the sidecar URL once `dsh web` reports it on stdout.

## Verification checklist

This PoC is validated manually. The matrix below tracks what has been run.

| Platform | cargo build (debug) | cargo build --release | Window opens | UI loads | One conversation |
|----------|--------------------|-----------------------|--------------|----------|------------------|
| Windows 10 | ✅               | ✅                    | ✅           | ✅       | TODO (needs API key) |
| macOS    | TODO               | TODO                  | TODO         | TODO     | TODO |
| Linux    | TODO               | TODO                  | TODO         | TODO     | TODO |

### Windows 10 verification

- `cargo build` exit 0 in 1m 10s (debug, first build fetches crates)
- `cargo build --release` exit 0 in 13m 21s
- `pnpm run workspace-constraints` (via `tsx scripts/check-workspace-constraints.ts`) passes
- `pnpm run knip` passes (`apps/tauri` in `ignoreWorkspaces`)
- Manual run of `target\debug\dsh-tauri-shell.exe` on Windows 10: window titled
  "DeepSeek Harness" opens, loading spinner shows, then the React UI from
  `dsh --profile web` loads in WebView2. Closing the window kills the sidecar.

### Manual verification steps (run on each platform)

1. `cd apps/tauri && pnpm tauri dev`
2. Confirm a window titled "DeepSeek Harness" opens (1200×800, resizable)
3. Confirm the loading spinner shows briefly, then the React UI replaces it
4. Type a prompt and confirm one full conversation round completes
5. Close the window and confirm the `dsh web` sidecar process terminates
   (Task Manager / `ps aux | grep dsh`)

## Known limitations

- **No native seams yet.** This PoC only validates WebView compatibility. Native
  directory picker, file system access, and other OS features land as DSH seams
  in follow-up work per the proposal's phased plan.
- **Sidecar lifecycle is process-coupled.** If the Rust shell crashes, the
  sidecar may be orphaned. The `on_window_event(Destroyed)` handler kills it on
  clean window close; a panic in the shell does not.
- **Port discovery is stdout-scraped.** `parse_dsh_url` matches the literal
  `dsh web: http://127.0.0.1:<port>` prefix. If `dsh web` changes its log line
  format, navigation breaks silently.
- **No CSP.** `app.security.csp` is `null` for the PoC. Production must lock
  this down to the sidecar origin.
- **`pnpm run dsh` requires a built tree.** The sidecar runs `pnpm run dsh web`
  from the repo root, which invokes `node --import tsx/esm apps/cli/src/bin.ts`.
  This is the source-launch contract; a packaged installer would need a bundled
  Node runtime and a built `lib/` tree.
- **macOS/Linux not yet run.** The Rust code is platform-agnostic Tauri v2, but
  the sidecar spawn uses `pnpm` (must be on `PATH`) and the window event handler
  is untested on WKWebView/WebKitGTK. File-system paths assume the repo layout;
  `CARGO_MANIFEST_DIR` resolution is platform-independent.
- **Windows `pnpm.cmd` shim.** Rust's `std::process::Command` does not resolve
  `.cmd` shims via `PATHEXT` automatically. `lib.rs` shells out through
  `cmd /C pnpm ...` on `cfg!(windows)` to find `pnpm.cmd`; Unix spawns `pnpm`
  directly. If `pnpm` is not on `PATH` in the Tauri-launched environment, the
  setup hook panics with `program not found`.

## Exemptions from repo gates

`apps/tauri` is a private Rust shell, not an npm release member. It is exempted
from:

- `check-workspace-constraints.ts` publishable-member and publication-files
  policies via `privateAppAllowlist` in `scripts/check-workspace-constraints.ts`
- `knip` via `ignoreWorkspaces` in `knip.json`

These exemptions are scoped to `apps/tauri` only and do not affect other
workspace members.
