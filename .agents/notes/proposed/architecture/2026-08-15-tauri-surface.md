# Agent Note: Tauri Desktop Surface

Status: proposed

English | [中文](2026-08-15-tauri-surface.zh.md)

## Problem

DSH ships two surfaces: `web` (browser over HTTP/WebSocket) and `headless` (no UI). Neither covers a distributable desktop application that uses native OS capabilities (system dialogs, tray, global shortcuts, file associations) and avoids the HTTP marshal overhead of the browser carrier. The `webserver` README already anticipates non-browser shells — it cites Electron loading dist over `file://` with fetch over an IPC bridge — but no such surface exists, and the `client/connection` carrier abstraction has only two implementations: `WebApiClient` (browser) and `FixtureApiClient` (test in-process).

Native desktop capabilities today are reachable only through `directory-picker-native` and `host.openPath`, both gated to the local Web carrier's trust fence. A Tauri shell that reused the browser carrier would inherit the loopback trust fence, HTTP marshal costs, and leave no path for native OS features to register as DSH seams. A separate Rust/JS UI built over the JSON-RPC SDK (`packages/examples/jsonrpc-demo`) would discard the entire `client/ui-*` React ecosystem.

The gap is a third surface sibling to `web` and `headless`, using a third carrier sibling to `WebApiClient` and `FixtureApiClient`, with Tauri's native features registered as DSH seams rather than trapped in the shell.

## Proposal

### Core principles

- Tauri is a **surface**, not a wrapper: a new bundle (`dsh-tauri-app`) sibling to `dsh-web-app` and `dsh-headless`, patching `dsh-base`.
- The Tauri carrier is a **third `IApiClient` implementation**, sibling to `WebApiClient` and `FixtureApiClient`, satisfying the same two-stream abstraction (unary/respond plus mux/host downlinks).
- Native OS features register as **DSH seams**, not shell-private code: `pickDirectory`/`openPath` get a Tauri provider sibling to `directory-picker-native`; future seams (tray, global shortcuts) follow the same pattern.
- `client-connection` and `client-tauri-carrier` are **mutually exclusive** — cordis enforces this by throwing on a second `ctx.apiClient` Service registration; the `client-tauri-carrier` package invariant only narrows the error message, it does not re-implement the check.

### Package topology

| Package | npm name | Responsibility |
| --- | --- | --- |
| `client/tauri-carrier` | `@deepseek-ai/dsh-client-tauri-carrier` | `TauriApiClient extends AbstractApiClient`: `doFetch` via `invoke()`, `openMux`/`openHost` via `event.listen()`; plus `MockTauriCarrier` for composition tests |
| `host/directory-picker-tauri` | `@deepseek-ai/dsh-host-directory-picker-tauri` | `pickDirectory`/`openPath` provider via Tauri `dialog`/`opener` plugins; sibling to `directory-picker-native`, cordis-mutex via duplicate `ctx.directoryPicker` Service |
| `bundle/tauri-app` | `@deepseek-ai/dsh-tauri-app` | Surface bundle: patches `dsh-base` to disable `client-connection`/`frontend-static`/`host-webserver`/`client-hmr`/`directory-picker-native`, insert Tauri carrier, picker, and `client-modules-tauri-wiring` rows |
| `client/modules-tauri-wiring` | `@deepseek-ai/dsh-client-modules-tauri-wiring` | Tauri-side wiring for `ClientModuleRegistry`: subscribes to `onGraphChanged`, writes the composed graph to the sidecar's stdout control channel; replaces the web bundle's `webServer.tapIndex` + `/plugins` route effects |
| `apps/tauri` | `@deepseek-ai/dsh-tauri-shell` (private workspace member, no npm publish) | Tauri Rust shell: spawns `dsh --profile tauri` sidecar, registers IPC commands, injects boot manifest. Workspace sibling to `apps/web`/`apps/cli` (already covered by `apps/*` in `pnpm-workspace.yaml`) |

### Carrier contract

`TauriApiClient` extends `AbstractApiClient` ([client/connection/src/client/api.ts](../../../../packages/client/connection/src/client/api.ts)) and implements the three abstract members:

- `doFetch(input, init)` → `invoke('dsh_rpc', { path, method, headers, body })`, reassembling the Rust response into a `Response`.
- `openMux(payload, signal, onOpen)` → `AsyncGenerator` backed by `event.listen('dsh-mux-frame', ...)`; `signal.abort` calls `unlisten`.
- `openHost(...)` → same pattern on `'dsh-host-frame'`.

Frame parsing reuses `serverRequestSchema` and `muxFrameSchema`/`hostFrameSchema` verbatim from `@deepseek-ai/dsh-host-apiproxy/api/events.schema`. `onEnvelope(full)` fires before yield, matching `WebApiClient`. `transportError` wraps `invoke` failures. The `ConnectionController` handshake (both streams open plus `host.describe`) is unchanged; the carrier only swaps the physical transport. `host.describe`, `host.pickDirectory`, and `host.openPath` are existing entries in `RpcMethodMap` ([host/apiproxy/src/api/rpc-map.ts](../../../../packages/host/apiproxy/src/api/rpc-map.ts)) — the Tauri carrier surfaces them unchanged, no new RPC entry is added.

### Boot manifest injection

`injectBootManifest(html, graph)` is a pure exported function in [client/modules/src/index.ts](../../../../packages/client/modules/src/index.ts); the `<`-escape and `<head>`-before-shell ordering live there. The web bundle's wiring (`ctx.webServer.tapIndex(html => injectBootManifest(html, this.composed))`) is one call site; the Tauri surface adds a second.

The Tauri shell never serves index.html over HTTP. The Rust shell reads the built `apps/web/dist/index.html` (or a Tauri-embedded resource) as a string, calls the Node sidecar over the sidecar control channel (see Process model) to fetch the composed `WebBootGraph`, runs the `<`-escape + `<script>` injection on the Rust side (mirroring `injectBootManifest` verbatim — the function is small enough to port, or the sidecar returns the already-injected html), and passes the result to WebView `set_html()` before the shell bundle runs. No new apiproxy RPC is added: the boot graph is composed host-side by `ClientModuleRegistry`, and the sidecar control channel is a separate stdio stream, not the carrier.

### ClientModules `webServer` dependency

`ClientModuleRegistry` currently declares `static inject = ['webServer', 'loader']` and its constructor unconditionally calls `ctx.webServer.register(...)` for the `/plugins` bundle route and `ctx.webServer.tapIndex(...)` for boot manifest injection ([client/modules/src/index.ts](../../../../packages/client/modules/src/index.ts)). Disabling `host-webserver` therefore throws at construction — the Tauri bundle cannot simply remove `host-webserver` without breaking `client-modules`.

The proposal splits `ClientModuleRegistry` along its existing seam:

- The **graph composer** (table, `compose()`, `flush()`, `onGraphChanged()`, `graph()`, `rebuilt()`) has no `webServer` dependency. It moves to the base class or stays on `ClientModuleRegistry` with `webServer` removed from `static inject`.
- The **web wiring** (`webServer.register('/plugins', ...)` + `webServer.tapIndex(injectBootManifest)`) moves to a new `client-modules-web-wiring` package that the `dsh-web-app` bundle inserts. This is a pure effect move; no behavior changes for the web surface.
- The **Tauri wiring** is `client-modules-tauri-wiring` (above): it subscribes to `onGraphChanged`, writes the graph to the sidecar control channel, and serves the `/plugins/<id>/client.js` bundle bytes by reading them from disk and shipping via `invoke('dsh_bundle', { id })`. The HMR row (`dsh-client-hmr`) stays disabled; the Tauri wiring has no HMR branch.

This split is the only refactor to an existing core package. It is in scope because the current constructor couples two unrelated effects (graph composition vs HTTP route registration); the split is symmetric with how the carrier abstraction already factors transport from protocol.

### Native picker provider

`directory-picker-tauri` extends `DirectoryPicker` from [host/directory-picker/src/index.ts](../../../../packages/host/directory-picker/src/index.ts) and returns a `kind: 'native'` capability backed by Tauri's `dialog::pick` and the `opener` plugin's `open::that`. Reusing the `native` kind (rather than declaration-merging a new `tauri` kind) is correct: Tauri `dialog` IS the OS chooser, just routed through Rust rather than `koffi`/`osascript`/PowerShell. `host.describe` returns `canOpenPath: true` unchanged.

Mutual exclusion between `directory-picker-native` and `directory-picker-tauri` is enforced by cordis: `DirectoryPicker extends Service`, and `super(ctx, 'directoryPicker')` throws on the second registration of the same Service name. The existing `host-directory-picker-native/invariant.ts` is an empty `InvariantInstaller` (`() => {}`); the `host-directory-picker-tauri` invariant stays empty too. The acceptance criterion asserts the cordis-thrown error, not a custom invariant — the proposal adds no new invariant code for mutex.

### Profile registration

`PROFILE_TEMPLATES` in [packages/boot/app-boot/src/profile.ts](../../../../packages/boot/app-boot/src/profile.ts) is a compile-time constant. The Tauri surface adds one row:

```ts ignore-check
tauri: ['@deepseek-ai/dsh-base', '@deepseek-ai/dsh-tauri-app'],
```

The test lives in [packages/boot/app-boot/tests/profile.spec.ts](../../../../packages/boot/app-boot/tests/profile.spec.ts), in the existing `describe('loadProfile')` block named `auto-initializes only shipped templates and fails loud otherwise` — the new assertion is a one-line sibling to the existing `expect(PROFILE_TEMPLATES.web).toContain('@deepseek-ai/dsh-base')`. This is the only `core/`-adjacent source file the proposal changes. No configuration mechanism exists for runtime profile registration, and adding one is out of scope.

### Process model

Tauri's Rust shell spawns `dsh --profile tauri` as a Node sidecar (matching `packages/examples/jsonrpc-demo`'s sidecar pattern). Two logical streams share the sidecar's stdin/stdout:

1. **Control channel** (newline-delimited JSON, request-response): Rust ↔ Node for boot graph fetch (`boot_graph` request), bundle fetch (`bundle` request), and shutdown. Used before `set_html()` and for non-streaming needs.
2. **Carrier downlinks** (newline-delimited JSON frames, one-way Node → Rust): the same `RpcRequest<MuxFrame>` and `RpcRequest<HostFrame>` envelopes the browser carrier would have sent over WebSocket. Rust forwards each frame to the WebView via `event.emit('dsh-mux-frame' | 'dsh-host-frame', payload)`.

stdin/stdout is the recommended transport: cross-platform parity, no platform-specific ACL setup (Windows named pipes need explicit ACLs for the WebView2 process to read them). A named pipe is added only if profiling shows the control channel contending with carrier downlinks — premature today. The Rust side enforces newline-delimited JSON and drops frames that fail `JSON.parse`, mirroring `WebApiClient`'s malformed-frame handling. Embedding Node via `node-addon-api` is rejected (see Alternatives). The sidecar is killed on window close.

### WebView runtime and CSP

Tauri v2 uses the OS webview: WebView2 on Windows, WKWebView on macOS, WebKitGTK on Linux. WebView2 is bootstrap-installed by Tauri's Windows installer (either the bootstrapper EXE or a fixed-version runtime bundled next to the app); macOS and Linux rely on the system webview. The acceptance test asserts `cargo tauri build` succeeds on all three, but does not pin a WebView2 version — Tauri's own runtime detection handles it.

`tauri.conf.json` carries a `csp` field. The inline `<script>window.__DSH_BOOT__ = ...</script>` that `injectBootManifest` produces requires either `'unsafe-inline'` (acceptable for a desktop app whose threat model is the host filesystem, not the network) or a hash/nonce. The proposal uses `'unsafe-inline'` in production: the boot graph is composed host-side from packages already on disk, so the inline-script threat surface equals the bundle-file threat surface. `'self'` covers the shell bundle loaded from Tauri's `resources`/`asset` protocol; `file://` is avoided because WebView2's `file://` CSP defaults block inline scripts.

### Dev-mode reload

Production builds have no HMR. Development uses Tauri's `tauri dev` watcher for Rust changes; for client bundle changes, the sidecar subscribes to `ClientModuleRegistry.onRebuilt` (the same hook `client-hmr` consumes) and emits a `dsh-reload` Tauri event that the shell bundle listens for and calls `window.location.reload()`. This is simpler than porting `client-hmr`'s incremental module swap, and the cost (full shell reload on each bundle change) is acceptable for desktop dev. The `dsh-tauri-app` bundle disables `client-hmr` in both dev and prod; a future `dsh-tauri-app-dev` overlay could re-enable it if incremental swap becomes worthwhile.

## Alternatives considered

### Why not Tauri as a thin shell over `dsh --profile web`?

Reuses the browser carrier with zero DSH code changes, but inherits the HTTP marshal overhead, the loopback trust fence, and traps all native OS features in the Tauri shell with no path back to `ctx.*`. The carrier abstraction that exists precisely for this purpose goes unused. Suitable as a PoC step (the WebView compatibility PoC below is exactly this), not as the shipped surface.

### Why not Tauri over the JSON-RPC SDK (`packages/examples/jsonrpc-demo`)?

The SDK provides a stdio JSON-RPC protocol that a Rust shell could speak directly. But stdio is request-response with no event-stream downlink, so the entire `client/ui-*` React ecosystem would need a replacement UI. The two-stream carrier abstraction that `client/connection` already factors out is the cheaper integration point.

### Why not Electron instead of Tauri?

Electron is the shell the `webserver` README names. It shares the WebView-plus-IPC shape with Tauri, but Tauri's Rust core is smaller, its IPC is `invoke`/`event` (already matched by the carrier contract), and the project has no existing Electron code. The carrier abstraction makes the shell swappable: an `electron-carrier` implementing the same `IApiClient` is a future sibling, not a redesign. Picking Tauri first does not foreclose Electron.

### Why not embed Node in the Tauri Rust process via `node-addon-api`?

The `native/` directory already ships a Node addon (`node-addon-landlock-run`), so the toolchain is not foreign. But embedding the full DSH runtime in-process couples Rust↔JS object lifetimes, complicates ESM loading (DSH requires `node --import tsx/esm`), and gains little over a sidecar that already speaks the carrier contract. The sidecar model matches `packages/examples/jsonrpc-demo` and keeps the Rust shell a thin IPC bridge.

### Why not extend `directory-picker-native` instead of a new Tauri provider?

`directory-picker-native` uses OS dialogs through `koffi`/`osascript`/PowerShell, reachable only from the Node host. A Tauri shell's native dialogs live in the Rust process, not the Node sidecar. Forcing the sidecar to call back into Rust for a dialog it cannot open would invert the process boundary. A sibling provider over Tauri `dialog` is the symmetric solution.

### Why not add a custom mutex invariant to `directory-picker-tauri`?

The current `host-directory-picker-native/invariant.ts` is an empty `InvariantInstaller` (`() => {}`); it asserts nothing at runtime. Cordis itself throws when a second `Service` registers under the same name (`super(ctx, 'directoryPicker')`), so loading both `directory-picker-native` and `directory-picker-tauri` already fails at service registration with a clear cordis error. A custom invariant that re-walks the fiber tree to detect the sibling provider would duplicate cordis' check and add a second failure mode. The proposal leaves both invariants empty and relies on cordis.

### Why not keep `host-webserver` loopback-only in the Tauri profile?

Avoids the `client-modules` split but ships a listening HTTP server that the Tauri shell never uses. The server still binds a port, still has the trust fence code path, and still invites "just hit it from a browser" drift. The split pays the refactor cost once and removes the dead surface permanently; keeping the server pays the drift cost forever.

## Acceptance criteria

- `dsh --profile tauri --dump-config` succeeds; output contains `client-tauri-carrier`, `host-directory-picker-tauri`, and `client-modules-tauri-wiring`, and does not contain `client-connection`, `frontend-static`, `host-webserver`, `client-hmr`, `client-modules-web-wiring`, or `directory-picker-native`.
- `pnpm run test:coverage` passes for all new packages (per-file 100%), including: a `client-modules` test asserting the composer still constructs without `webServer` in `static inject`; a `client-modules-tauri-wiring` test asserting the graph is written to the sidecar control channel on `onGraphChanged`; a `directory-picker-tauri` test asserting that loading it alongside `directory-picker-native` throws a cordis duplicate-Service error.
- A keyless snapshot in `packages/examples/tauri-replay/` replays a one-turn conversation through `MockTauriCarrier` and is registered in `test:snapshot`.
- `pnpm run hygiene`, `pnpm run doc-sync`, and `pnpm run website:build` pass. The `AGENTS.md` Repository layout section and `packages/README.md` package groups table are updated in the same PR to add `apps/tauri`, `client/modules-tauri-wiring`, `client/tauri-carrier`, and `host/directory-picker-tauri`.
- `cargo tauri build` produces a runnable installer on Windows, macOS, and Linux; the window opens, a conversation completes, `pickDirectory` opens the OS dialog, and a reload triggered by a client bundle rebuild in dev mode reaches the WebView.
- The `tauri` row in `PROFILE_TEMPLATES` is covered by an assertion in `app-boot/tests/profile.spec.ts`'s `auto-initializes only shipped templates` block.
- A PoC PR lands first: Tauri shell + `dsh --profile web` loads the existing React UI in WebView2, WKWebView, and WebKitGTK without carrier changes. Carrier work does not begin until the PoC passes on all three platforms.

## Risks

- **`client-modules` split regressions.** Moving `webServer.register`/`tapIndex` out of `ClientModuleRegistry` changes a constructor that has run unchanged since the web bundle shipped. Mitigation: the split is a pure effect move, the web-bundle tests (`apps/web/tests/*.e2e.ts`) cover the unchanged behavior, and `pnpm run test:coverage` gates the new wiring packages at 100%.
- **Boot manifest injection timing.** The shell bundle reads `window.__DSH_BOOT__` synchronously; if the Tauri shell calls `set_html` before the boot graph arrives over the control channel, the shell throws. Mitigation: Rust blocks on the `boot_graph` control-channel request before `set_html()`; the `MockTauriCarrier` test exercises the injection path end-to-end.
- **Sidecar stdout frame parsing across platforms.** Line buffering and JSON framing differ between Windows pipes and POSIX. Mitigation: the Rust side enforces newline-delimited JSON and drops frames that fail `JSON.parse`, mirroring `WebApiClient`'s malformed-frame handling; the carrier downlink frames are emitted on a separate logical channel from control responses so a malformed control response cannot desync the downlink.
- **CI cannot run a real Tauri GUI.** The composition and snapshot tests use `MockTauriCarrier`; real Tauri is manual-only. This matches the project's existing split between keyless snapshots and real-API e2e.
- **`PROFILE_TEMPLATES` upstream conflicts.** The one-row addition to a core package is a merge surface. Mitigation: pre-release stance permits free restructuring; the row is isolated and the test is narrow.
- **Tauri WebView React compatibility.** Tauri's WebView uses the OS webview (WebView2/WebKitGTK/WKWebView), not Chromium. Mitigation: the PoC PR (acceptance above) validates UI load on all three platforms before carrier work begins.
- **Carrier abstraction drift.** If `client/connection` evolves its two-stream contract, `client-tauri-carrier` must follow. Mitigation: the contract is stable (the WebSocket downlink Agent Note owns it); the Tauri carrier is the third implementation of the same abstract class, so any contract change surfaces as a compile error in all three.
- **CSP / inline `__DSH_BOOT__` script.** WebView2's default `file://` CSP blocks inline scripts; the Tauri shell loads assets via the `asset` protocol (not `file://`) and `tauri.conf.json` sets `csp` to allow `'unsafe-inline'`. The threat surface equals the bundle-file threat surface (the graph is composed from on-disk packages), but the decision is explicit in `tauri.conf.json`, not inherited silently.
- **WebView2 runtime availability on Windows.** Tauri's bootstrapper handles install-on-first-run, but enterprise kiosk environments without WebView2 preinstalled will see a one-time install delay. Mitigation: the installer bundles the fixed-version WebView2 runtime as a fallback; the acceptance test covers a clean-install Windows run.
