# Agent Note: Tauri 桌面 Surface

Status: proposed

[English](2026-08-15-tauri-surface.md) | 中文

## 问题

DSH 现交付两个 surface：`web`（浏览器经 HTTP/WebSocket）和 `headless`（无 UI）。两者都不覆盖可分发的桌面应用——这种应用需要使用 OS 原生能力（系统对话框、托盘、全局快捷键、文件关联）并避开浏览器 carrier 的 HTTP marshal 开销。`webserver` README 已经预见了非浏览器 shell——它提到 Electron 通过 `file://` 加载 dist 并经 IPC 桥接 fetch——但并无此类 surface，且 `client/connection` 的 carrier 抽象只有两个实现：`WebApiClient`（浏览器）和 `FixtureApiClient`（测试 in-process）。

原生桌面能力今日仅能通过 `directory-picker-native` 和 `host.openPath` 触达，两者都被限定在本地 Web carrier 的信任栅栏内。复用浏览器 carrier 的 Tauri shell 会继承 loopback 信任栅栏、HTTP marshal 成本，并让原生 OS 特性无路径注册为 DSH seam。基于 JSON-RPC SDK（`packages/examples/jsonrpc-demo`）另起 Rust/JS UI 会丢弃整个 `client/ui-*` React 生态。

缺口是一个与 `web`/`headless` 同位的第三个 surface，使用与 `WebApiClient`/`FixtureApiClient` 同位的第三个 carrier，并把 Tauri 原生能力注册为 DSH seam，而非困在 shell 私有代码里。

## 方案

### 核心原则

- Tauri 是 **surface**，不是 wrapper：新增 bundle `dsh-tauri-app`，与 `dsh-web-app`/`dsh-headless` 同位，patch `dsh-base`。
- Tauri carrier 是**第三个 `IApiClient` 实现**，与 `WebApiClient`/`FixtureApiClient` 同位，满足同一双流抽象（unary/respond 加 mux/host 下行）。
- 原生 OS 能力注册为 **DSH seam**，而非 shell 私有代码：`pickDirectory`/`openPath` 新增 Tauri provider，与 `directory-picker-native` 同位；未来 seam（托盘、全局快捷键）沿用同一模式。
- `client-connection` 与 `client-tauri-carrier` **互斥**——cordis 在第二次注册同名 `ctx.apiClient` Service 时抛错来强制；`client-tauri-carrier` 包 invariant 只收窄错误信息，不重新实现该检查。

### 包拓扑

| 包 | npm 名 | 职责 |
| --- | --- | --- |
| `client/tauri-carrier` | `@deepseek-ai/dsh-client-tauri-carrier` | `TauriApiClient extends AbstractApiClient`：`doFetch` 走 `invoke()`、`openMux`/`openHost` 走 `event.listen()`；并附 `MockTauriCarrier` 供组合测试 |
| `host/directory-picker-tauri` | `@deepseek-ai/dsh-host-directory-picker-tauri` | 经 Tauri `dialog`/`opener` 插件提供 `pickDirectory`/`openPath`；与 `directory-picker-native` 同位，经 cordis 重复 `ctx.directoryPicker` Service 实现互斥 |
| `bundle/tauri-app` | `@deepseek-ai/dsh-tauri-app` | Surface bundle：patch `dsh-base`，禁用 `client-connection`/`frontend-static`/`host-webserver`/`client-hmr`/`directory-picker-native`，插入 Tauri carrier、picker、`client-modules-tauri-wiring` 行 |
| `client/modules-tauri-wiring` | `@deepseek-ai/dsh-client-modules-tauri-wiring` | `ClientModuleRegistry` 的 Tauri 侧 wiring：订阅 `onGraphChanged`，把组合好的 graph 写入 sidecar stdout 控制通道；替代 web bundle 的 `webServer.tapIndex` + `/plugins` 路由 effect |
| `apps/tauri` | `@deepseek-ai/dsh-tauri-shell`（私有 workspace 成员，不发布到 npm） | Tauri Rust shell：spawn `dsh --profile tauri` sidecar、注册 IPC 命令、注入 boot manifest。与 `apps/web`/`apps/cli` 同位（`pnpm-workspace.yaml` 的 `apps/*` 已覆盖） |

### Carrier 契约

`TauriApiClient` 继承 [client/connection/src/client/api.ts](../../../../packages/client/connection/src/client/api.ts) 的 `AbstractApiClient`，实现三个抽象成员：

- `doFetch(input, init)` → `invoke('dsh_rpc', { path, method, headers, body })`，将 Rust 响应重组为 `Response`。
- `openMux(payload, signal, onOpen)` → `AsyncGenerator`，由 `event.listen('dsh-mux-frame', ...)` 驱动；`signal.abort` 调 `unlisten`。
- `openHost(...)` → 同模式监听 `'dsh-host-frame'`。

帧解析直接复用 `@deepseek-ai/dsh-host-apiproxy/api/events.schema` 的 `serverRequestSchema` 和 `muxFrameSchema`/`hostFrameSchema`。`onEnvelope(full)` 在 yield 前触发，与 `WebApiClient` 一致。`transportError` 包装 `invoke` 失败。`ConnectionController` 握手（双流打开加 `host.describe`）不变；carrier 只换物理传输。`host.describe`、`host.pickDirectory`、`host.openPath` 已是 [host/apiproxy/src/api/rpc-map.ts](../../../../packages/host/apiproxy/src/api/rpc-map.ts) 中 `RpcMethodMap` 的既有条目——Tauri carrier 原样暴露，不新增 RPC 条目。

### Boot manifest 注入

`injectBootManifest(html, graph)` 是 [client/modules/src/index.ts](../../../../packages/client/modules/src/index.ts) 的纯函数导出；`<` 转义和 `<head>`-before-shell 顺序都在其中。web bundle 的 wiring（`ctx.webServer.tapIndex(html => injectBootManifest(html, this.composed))`）是一个调用点；Tauri surface 增加第二个。

Tauri shell 不经 HTTP 提供 index.html。Rust shell 读取构建好的 `apps/web/dist/index.html`（或 Tauri 嵌入资源）作为字符串，经 sidecar 控制通道（见下文 Process model）向 Node sidecar 请求组合好的 `WebBootGraph`，在 Rust 侧执行 `<` 转义 + `<script>` 注入（与 `injectBootManifest` 字面一致——该函数小到可移植，或 sidecar 直接返回已注入的 html），在 shell bundle 运行前把结果传给 WebView `set_html()`。不新增 apiproxy RPC：boot graph 由 `ClientModuleRegistry` 在 host 侧组合，sidecar 控制通道是与 carrier 分开的 stdio 流。

### ClientModules 对 `webServer` 的依赖

`ClientModuleRegistry` 当前声明 `static inject = ['webServer', 'loader']`，其构造函数无条件调用 `ctx.webServer.register(...)` 注册 `/plugins` bundle 路由、`ctx.webServer.tapIndex(...)` 注入 boot manifest（[client/modules/src/index.ts](../../../../packages/client/modules/src/index.ts)）。因此禁用 `host-webserver` 会在构造时抛错——Tauri bundle 不能简单移除 `host-webserver` 而不破坏 `client-modules`。

方案沿 `ClientModuleRegistry` 既有 seam 拆分：

- **graph composer**（table、`compose()`、`flush()`、`onGraphChanged()`、`graph()`、`rebuilt()`）不依赖 `webServer`。它移到基类或留在 `ClientModuleRegistry` 上，从 `static inject` 移除 `webServer`。
- **web wiring**（`webServer.register('/plugins', ...)` + `webServer.tapIndex(injectBootManifest)`）移到新 `client-modules-web-wiring` 包，由 `dsh-web-app` bundle 插入。这是纯 effect 移动；web surface 行为不变。
- **Tauri wiring** 即上文 `client-modules-tauri-wiring`：订阅 `onGraphChanged`，把 graph 写入 sidecar 控制通道；通过 `invoke('dsh_bundle', { id })` 从磁盘读取并运送 `/plugins/<id>/client.js` bundle 字节。HMR 行（`dsh-client-hmr`）保持禁用；Tauri wiring 无 HMR 分支。

此拆分是对现有 core 包的唯一重构。它在范围内，因为当前构造函数把两个不相关的 effect（graph 组合 vs HTTP 路由注册）耦合在一起；拆分与 carrier 抽象把传输从协议分离的做法对称。

### 原生 picker provider

`directory-picker-tauri` 继承 [host/directory-picker/src/index.ts](../../../../packages/host/directory-picker/src/index.ts) 的 `DirectoryPicker`，返回 `kind: 'native'` capability，由 Tauri `dialog::pick` 和 `opener` 插件的 `open::that` 支撑。复用 `native` kind（而非 declaration-merge 一个新 `tauri` kind）是正确的：Tauri `dialog` 即 OS 选择器，只是经 Rust 路由，而非 `koffi`/`osascript`/PowerShell。`host.describe` 返回 `canOpenPath: true` 不变。

`directory-picker-native` 与 `directory-picker-tauri` 的互斥由 cordis 强制：`DirectoryPicker extends Service`，`super(ctx, 'directoryPicker')` 在第二次注册同名 Service 时抛错。现有 `host-directory-picker-native/invariant.ts` 是空的 `InvariantInstaller`（`() => {}`）；`host-directory-picker-tauri` 的 invariant 也保持空。验收条件断言 cordis 抛出的错误，而非自定义 invariant——方案不为 mutex 新增任何 invariant 代码。

### Profile 注册

[packages/boot/app-boot/src/profile.ts](../../../../packages/boot/app-boot/src/profile.ts) 的 `PROFILE_TEMPLATES` 是编译期常量。Tauri surface 新增一行：

```ts ignore-check
tauri: ['@deepseek-ai/dsh-base', '@deepseek-ai/dsh-tauri-app'],
```

测试位于 [packages/boot/app-boot/tests/profile.spec.ts](../../../../packages/boot/app-boot/tests/profile.spec.ts)，在既有的 `describe('loadProfile')` 块 `auto-initializes only shipped templates and fails loud otherwise` 中——新断言是既有 `expect(PROFILE_TEMPLATES.web).toContain('@deepseek-ai/dsh-base')` 的一行同位。这是方案触及的唯一 `core/`-相邻源文件。不存在运行时 profile 注册机制，新增该机制超出范围。

### 进程模型

Tauri 的 Rust shell spawn `dsh --profile tauri` 为 Node sidecar（沿用 `packages/examples/jsonrpc-demo` 的 sidecar 模式）。两条逻辑流共享 sidecar 的 stdin/stdout：

1. **控制通道**（换行分隔 JSON，请求-响应）：Rust ↔ Node，用于 boot graph 拉取（`boot_graph` 请求）、bundle 拉取（`bundle` 请求）、关停。在 `set_html()` 前及非流式需求时使用。
2. **Carrier 下行**（换行分隔 JSON 帧，单向 Node → Rust）：与浏览器 carrier 经 WebSocket 发送的 `RpcRequest<MuxFrame>` 和 `RpcRequest<HostFrame>` 信封相同。Rust 把每帧经 `event.emit('dsh-mux-frame' | 'dsh-host-frame', payload)` 转发到 WebView。

推荐 stdin/stdout 作为传输：跨平台一致，无需平台相关 ACL（Windows named pipe 需为 WebView2 进程显式配 ACL 才能读取）。仅在 profiling 显示控制通道与 carrier 下行竞争时才加 named pipe——今日为时过早。Rust 侧强制换行分隔 JSON，丢弃 `JSON.parse` 失败的帧，与 `WebApiClient` 的 malformed-frame 处理一致。经 `node-addon-api` 嵌入 Node 已否决（见 Alternatives）。窗口关闭时 sidecar 被杀掉。

### WebView 运行时与 CSP

Tauri v2 使用 OS webview：Windows 用 WebView2、macOS 用 WKWebView、Linux 用 WebKitGTK。WebView2 由 Tauri 的 Windows 安装器 bootstrap 安装（bootstrapper EXE 或紧邻应用捆绑的 fixed-version runtime）；macOS 和 Linux 依赖系统 webview。验收测试断言 `cargo tauri build` 在三者上成功，但不固定 WebView2 版本——Tauri 自身的运行时检测负责。

`tauri.conf.json` 携带 `csp` 字段。`injectBootManifest` 产生的内联 `<script>window.__DSH_BOOT__ = ...</script>` 需要 `'unsafe-inline'`（对桌面应用而言威胁模型是主机文件系统而非网络，可接受）或 hash/nonce。方案在生产用 `'unsafe-inline'`：boot graph 由 host 侧从已在磁盘上的包组合，内联脚本的威胁面等同于 bundle 文件的威胁面。`'self'` 覆盖经 Tauri `resources`/`asset` 协议加载的 shell bundle；避免 `file://`，因为 WebView2 的 `file://` CSP 默认会阻断内联脚本。

### 开发态 reload

生产构建无 HMR。开发态用 Tauri 的 `tauri dev` watcher 跟踪 Rust 变更；对 client bundle 变更，sidecar 订阅 `ClientModuleRegistry.onRebuilt`（与 `client-hmr` 同一钩子），发出 `dsh-reload` Tauri 事件，shell bundle 监听并调 `window.location.reload()`。这比移植 `client-hmr` 的增量模块替换简单，代价（每次 bundle 变更触发整 shell reload）对桌面开发可接受。`dsh-tauri-app` bundle 在 dev 和 prod 都禁用 `client-hmr`；若增量替换日后值得，可加 `dsh-tauri-app-dev` overlay 重新启用。

## 备选方案

### 为什么不让 Tauri 作 `dsh --profile web` 之上的薄壳？

零 DSH 代码改动即可复用浏览器 carrier，但继承 HTTP marshal 开销、loopback 信任栅栏，并把所有原生 OS 特性困在 Tauri shell 内无路径回到 `ctx.*`。为此目的存在的 carrier 抽象未被使用。适合作 PoC 步骤（下文 WebView 兼容性 PoC 即此），不适合作交付 surface。

### 为什么不用 JSON-RPC SDK（`packages/examples/jsonrpc-demo`）上的 Tauri？

SDK 提供 stdio JSON-RPC 协议，Rust shell 可直接对话。但 stdio 是请求-响应，无事件流下行，因此整个 `client/ui-*` React 生态需另起 UI。`client/connection` 已经抽出的双流 carrier 抽象是更廉价的集成点。

### 为什么不用 Electron 替代 Tauri？

Electron 是 `webserver` README 点名的 shell。它与 Tauri 共享 WebView+IPC 形态，但 Tauri 的 Rust 核心更小，IPC 是 `invoke`/`event`（已被 carrier 契约匹配），且项目无既有 Electron 代码。carrier 抽象使 shell 可换：实现同一 `IApiClient` 的 `electron-carrier` 是未来同位项，而非重新设计。先选 Tauri 不排除 Electron。

### 为什么不通过 `node-addon-api` 把 Node 嵌入 Tauri Rust 进程？

`native/` 目录已交付 Node addon（`node-addon-landlock-run`），工具链并不陌生。但把整个 DSH 运行时嵌入进程会耦合 Rust↔JS 对象生命周期，复杂化 ESM 加载（DSH 需 `node --import tsx/esm`），相比已能讲 carrier 契约的 sidecar 收益甚微。sidecar 模型与 `packages/examples/jsonrpc-demo` 一致，保持 Rust shell 为薄 IPC 桥。

### 为什么不扩展 `directory-picker-native` 而新增 Tauri provider？

`directory-picker-native` 经 `koffi`/`osascript`/PowerShell 调 OS 对话框，仅能从 Node host 触达。Tauri shell 的原生对话框在 Rust 进程，不在 Node sidecar。强迫 sidecar 回调 Rust 打开它无法打开的对话框会反转进程边界。基于 Tauri `dialog` 的同位 provider 是对称解。

### 为什么不为 `directory-picker-tauri` 加自定义 mutex invariant？

现有 `host-directory-picker-native/invariant.ts` 是空的 `InvariantInstaller`（`() => {}`），运行时不断言任何东西。cordis 自身在第二次以同名注册 `Service` 时抛错（`super(ctx, 'directoryPicker')`），因此同时加载 `directory-picker-native` 和 `directory-picker-tauri` 已在 service 注册时以清晰的 cordis 错误失败。重走 fiber 树检测同位 provider 的自定义 invariant 会重复 cordis 的检查并增加第二个失败模式。方案让两者 invariant 都保持空，依赖 cordis。

### 为什么不在 Tauri profile 里保留 loopback-only `host-webserver`？

避免 `client-modules` 拆分，但交付一个 Tauri shell 永不使用的监听 HTTP server。该 server 仍会绑端口、仍走信任栅栏代码路径、仍诱使"从浏览器直接打"的漂移。拆分一次性付清重构成本并永久移除该死面；保留 server 永远付漂移成本。

## 验收标准

- `dsh --profile tauri --dump-config` 成功；输出含 `client-tauri-carrier`、`host-directory-picker-tauri`、`client-modules-tauri-wiring`，不含 `client-connection`、`frontend-static`、`host-webserver`、`client-hmr`、`client-modules-web-wiring`、`directory-picker-native`。
- 所有新包通过 `pnpm run test:coverage`（每文件 100%），包括：`client-modules` 测试断言 composer 在 `static inject` 无 `webServer` 时仍可构造；`client-modules-tauri-wiring` 测试断言 `onGraphChanged` 触发时 graph 被写入 sidecar 控制通道；`directory-picker-tauri` 测试断言与 `directory-picker-native` 同时加载时抛 cordis 重复 Service 错误。
- `packages/examples/tauri-replay/` 的 keyless snapshot 经 `MockTauriCarrier` 回放一轮对话，并注册进 `test:snapshot`。
- `pnpm run hygiene`、`pnpm run doc-sync`、`pnpm run website:build` 通过。同一 PR 内更新 `AGENTS.md` 的 Repository layout 段和 `packages/README.md` 包分组表，加入 `apps/tauri`、`client/modules-tauri-wiring`、`client/tauri-carrier`、`host/directory-picker-tauri`。
- `cargo tauri build` 在 Windows、macOS、Linux 上产出可运行安装器；窗口可打开、对话可完成、`pickDirectory` 打开 OS 对话框、dev 模式下 client bundle rebuild 触发的 reload 到达 WebView。
- `PROFILE_TEMPLATES` 的 `tauri` 行被 `app-boot/tests/profile.spec.ts` 的 `auto-initializes only shipped templates` 块中断言覆盖。
- 先落 PoC PR：Tauri shell + `dsh --profile web` 在 WebView2、WKWebView、WebKitGTK 上加载既有 React UI，无需 carrier 改动。PoC 在三平台通过前不开始 carrier 工作。

## 风险

- **`client-modules` 拆分回归。** 把 `webServer.register`/`tapIndex` 移出 `ClientModuleRegistry` 改变了自 web bundle 交付以来未变的构造函数。缓解：拆分是纯 effect 移动，web bundle 测试（`apps/web/tests/*.e2e.ts`）覆盖不变行为，`pnpm run test:coverage` 对新 wiring 包门控 100%。
- **Boot manifest 注入时序。** shell bundle 同步读取 `window.__DSH_BOOT__`；若 Tauri shell 在 boot graph 经控制通道到达前调 `set_html`，shell 会抛错。缓解：Rust 在 `set_html()` 前阻塞于 `boot_graph` 控制通道请求；`MockTauriCarrier` 测试端到端覆盖注入路径。
- **Sidecar stdout 跨平台帧解析。** 行缓冲和 JSON 成帧在 Windows 管道与 POSIX 间不同。缓解：Rust 侧强制换行分隔 JSON，丢弃 `JSON.parse` 失败的帧，与 `WebApiClient` 的 malformed-frame 处理一致；carrier 下行帧与控制响应在不同的逻辑通道上发出，单个 malformed 控制响应不会让下行失步。
- **CI 无法跑真实 Tauri GUI。** 组合与 snapshot 测试用 `MockTauriCarrier`；真实 Tauri 仅手动。这与项目现有的 keyless snapshot 与 real-API e2e 划分一致。
- **`PROFILE_TEMPLATES` 上游冲突。** 对 core 包的一行新增是合并面。缓解：pre-release 立场允许自由重构；行是隔离的，测试是窄的。
- **Tauri WebView React 兼容性。** Tauri 的 WebView 用 OS webview（WebView2/WebKitGTK/WKWebView），非 Chromium。缓解：PoC PR（见上文验收）在 carrier 工作开始前三平台验证 UI 加载。
- **Carrier 抽象漂移。** 若 `client/connection` 演进双流契约，`client-tauri-carrier` 须跟随。缓解：契约稳定（WebSocket 下行 Agent Note 拥有它）；Tauri carrier 是同一抽象类的第三个实现，任何契约变更会在三者上同时表现为编译错误。
- **CSP / 内联 `__DSH_BOOT__` 脚本。** WebView2 默认 `file://` CSP 阻断内联脚本；Tauri shell 经 `asset` 协议（非 `file://`）加载资源，`tauri.conf.json` 设 `csp` 允许 `'unsafe-inline'`。威胁面等同于 bundle 文件威胁面（graph 由磁盘上的包组合），但该决定在 `tauri.conf.json` 中显式声明，非静默继承。
- **Windows 上 WebView2 运行时可用性。** Tauri 的 bootstrapper 处理首次运行安装，但未预装 WebView2 的企业 kiosk 环境会看到一次性安装延迟。缓解：安装器捆绑 fixed-version WebView2 runtime 作为兜底；验收测试覆盖一次净装 Windows 运行。
