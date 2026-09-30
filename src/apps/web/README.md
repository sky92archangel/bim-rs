# 浏览器端 `apps/web`（占位）

**这不是一个 cargo 成员**，因此不参与 `cargo build --workspace`（见根 `Cargo.toml` 的说明与设计文档 2.2）。

## 为什么现在只有占位

设计文档 1.6 的桌面端选型结论是 **egui + wgpu（eframe）**，因为它与 Core 同源（Rust 原生单进程、内核 wasmtime 原生跑，没有 WebView / IPC / 浏览器内 WASM 的双重开销）。
**浏览器端只在"要与 Web 端复用同一套前端"时才值得做**，其代价是各系统 WebView 的 WebGPU 支持不均、需要 WebGL 回退。这是一阶段的**非目标**。

## 将来的形态（M-later）

若启动，按设计文档的落法：

```
apps/web/
├─ package.json          # 依赖 @tauri-apps/* 或纯 wasm-bindgen 打包
├─ vite.config.ts
├─ index.html
├─ src/
│  ├─ main.ts
│  ├─ viewport/          # WebGPU 视口（回退 WebGL2）
│  └─ bridge/            # 与 Core 的边界：仅经 core-app 暴露的用例接口
└─ wasm/                 # core-* 编译为 wasm 的产物（目标：wasm32-unknown-unknown）
```

**铁律不变**：前端只能经 `core-app` 暴露的用例接口访问 Core，不得绕过它直接触碰 `core-model` / `core-kernel`。
