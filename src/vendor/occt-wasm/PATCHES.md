# vendor/occt-wasm — 本地补丁说明（PATCHES）

## 基线

- 上游 crate：`occt-wasm`
- 版本：**4.0.0**（crates.io 最新版，2026-09-21 发布；截至修复时无 4.0.1）
- 来源：镜像 `mirrors.tuna.tsinghua.edu.cn` 的 registry 拷贝
  （`...\registry\src\mirrors.tuna.tsinghua.edu.cn-4dc01642fd091eda\occt-wasm-4.0.0`）
- 许可：`MIT OR Apache-2.0`（与本仓库 `deny.toml` 白名单一致）

本目录是**完整复制**后打了两处补丁的副本，**不是**本仓库 workspace 成员
（根 `Cargo.toml` 的 `[workspace].exclude` 已排除，`[patch.crates-io]` 指向本目录）。

## 改了什么

共两处，均在 Rust API 层，**未改 WASM blob**（`src/occt-wasm.wasm.br` 原样）。

### 补丁 1（2026-09-23）：linker 定义未知导入

`src/kernel.rs` 的 `OcctKernel::new()`（原第 160–161 行附近）。

```diff
     let mut store = Store::new(&engine, ());
-    let linker = Linker::new(&engine);
+    let mut linker = Linker::new(&engine);
+    linker.define_unknown_imports_as_traps(&module)?;
     let instance = linker.instantiate(&mut store, &module)?;
```

### 补丁 2（2026-09-24）：`ShapeHandle::from_raw`

`src/types.rs` 的 `impl ShapeHandle` 追加一个公开构造器。

```diff
 impl ShapeHandle {
     /// Returns the raw arena ID. Useful for debugging.
     #[must_use]
     pub const fn id(self) -> u32 {
         self.0
     }
+
+    /// 由内核 arena 原始 id 重建句柄（`EvolutionData.result_id` 等场景）。
+    #[must_use]
+    pub const fn from_raw(raw: u32) -> Self {
+        Self(raw)
+    }
 }
```

**为什么**：`EvolutionData.result_id` 是**裸 `u32`**（见 `src/types.rs` 的
`EvolutionData`），而 `ShapeHandle` 的字段 `pub(crate) u32` 是 crate 私有、原本
**无公开构造器**（只有 `id()` 取数）。于是 `core-kernel` 的适配层**无法**把带历史
布尔的结果 `result_id` 收进自己的句柄 arena。补上 `from_raw` 后，
`core-kernel/src/occt.rs` 的 `*_with_history` 接线（M5 B3 主路径）才能写出
`ShapeHandle::from_raw(evo.result_id)`。

> 这是 `core-kernel` **内部**使用的 vendor 能力；`ShapeHandle` 是 `occt_wasm::` 类型，
> 按铁律①**不得**经 `core-kernel` 公开 API 向上暴露（上层只见 `core_kernel::ShapeHandle`）。

## 补丁 1 为什么改

上游 4.0.0 的 `OcctKernel::new()` 建了一个**空 linker** 就 `instantiate`。但
`src/occt-wasm.wasm.br`（brotli 压缩，解压约 **24.3 MB**）是 Emscripten + WASI 产物，
声明了 18 个 host import，空 linker 一个都没定义，于是 wasmtime 48 在 `instantiate`
阶段直接失败：

```
Kernel(Geometry { op: "OcctKernel::new",
  msg: "WASM runtime error: unknown import:
        `env::emscripten_get_preloaded_image_data` has not been defined" })
```

## 实证：WASM 的真实 import 清单（18 个，全部为函数）

用 Node.js 的 `zlib.brotliDecompressSync` 解压后手工解析 import section 得到
（并用 `WebAssembly.Module.imports()` 交叉核对，计数一致）：

| # | module::name | 签名 |
|---|---|---|
| 1 | `env::emscripten_get_preloaded_image_data` | `(i32, i32, i32) -> (i32)` |
| 2 | `env::emscripten_get_callstack` | `(i32, i32, i32) -> (i32)` |
| 3 | `env::OSD_MemInfo_getModuleHeapLength` | `() -> (f64)` |
| 4 | `wasi_snapshot_preview1::proc_exit` | `(i32) -> ()` |
| 5 | `env::emscripten_notify_memory_growth` | `(i32) -> ()` |
| 6 | `wasi_snapshot_preview1::fd_close` | `(i32) -> (i32)` |
| 7 | `wasi_snapshot_preview1::fd_read` | `(i32, i32, i32, i32) -> (i32)` |
| 8 | `wasi_snapshot_preview1::fd_write` | `(i32, i32, i32, i32) -> (i32)` |
| 9 | `env::__syscall_faccessat` | `(i32, i32, i32, i32) -> (i32)` |
| 10 | `env::__syscall_chmod` | `(i32, i32) -> (i32)` |
| 11 | `wasi_snapshot_preview1::clock_time_get` | `(i32, i64, i32) -> (i32)` |
| 12 | `wasi_snapshot_preview1::environ_sizes_get` | `(i32, i32) -> (i32)` |
| 13 | `wasi_snapshot_preview1::environ_get` | `(i32, i32) -> (i32)` |
| 14 | `wasi_snapshot_preview1::fd_seek` | `(i32, i64, i32, i32) -> (i32)` |
| 15 | `env::__syscall_getdents64` | `(i32, i32, i32) -> (i32)` |
| 16 | `env::__syscall_unlinkat` | `(i32, i32, i32) -> (i32)` |
| 17 | `env::__syscall_rmdir` | `(i32) -> (i32)` |
| 18 | `env::_emscripten_lookup_name` | `(i32) -> (i32)` |

- 模块计数：`env` 10 个，`wasi_snapshot_preview1` 8 个。
- **没有一个是 `() -> ()`**（trivial 计数 = 0）→ 未知导入必须按真实签名逐一定义，
  不能只定义一个空函数；`define_unknown_imports_as_traps` 会按模块声明签名生成桩。

## 为什么用 traps 而不是 default values

- `define_unknown_imports_as_traps(&module)`：为每个未知导入生成"被调用即 trap"的桩。
  内核正常路径若**不调用**这些 host 函数，则一切正常；若**误调**，会以 trap 形式的
  **显式失败**暴露出来，而不会静默返回 0 值把几何算错。
- `define_unknown_imports_as_default_values(&mut store, &module)`：返回 0/0.0。
  风险是"能装起来但算错"，因此只有在 traps 真的在正常路径被触发时才退而用之，
  且必须同时保留端到端几何正确性用例。

### 实测结论：采用 traps（已 A/B 验证）

对 `cargo test -p core-kernel --features occt --release` 做了 A/B：

| 变体 | `occt_feature_must_actually_instantiate_the_kernel` | 其余 occt 用例 |
|---|---|---|
| **traps（采用）** | ✅ 通过 | 59 通过 / 3 失败（见下） |
| default values | ✅ 通过 | **完全相同的** 59 通过 / 3 失败 |

两者结果**逐条一致**——若某个 host import 真的被调用，default values 会返回 0 而改变行为，
但结果没有变化，说明**被验证的这些几何路径根本没有调用任何未实现的 host import**。
因此：

- **采用 traps**：一旦未来某条路径真的调到未实现的 host import，会**立即 trap（显式失败）**，
  而不会静默返回 0 把几何算错。这是更安全的默认。
- 那 3 条失败与本次 linker 修复**无关**（两个变体同因失败），属适配层与内核的语义口径问题，
  另案处理。

> ⚠ 注意：本 WASM 声明了 `OSD_MemInfo_getModuleHeapLength () -> f64`——**唯一返回非整数的**
> 导入，正是"default values 会给错值"的典型。用 traps 可保证它若被调用会立刻暴露。

## 上游若发布修复版，如何移除

1. 删除本目录（`vendor/occt-wasm`）。
2. 移除工作区根 `Cargo.toml` 的 `[patch.crates-io]` 段与 `[workspace].exclude` 里的
   `"vendor/occt-wasm"`。
3. 把根 `Cargo.toml` 的 `occt-wasm` 版本声明提升到修复版（或保持 `"4"`）。
4. 重跑 M0 门禁（`cargo test -p core-kernel --features occt --release`）确认守护用例
   `occt_feature_must_actually_instantiate_the_kernel` 仍通过。
