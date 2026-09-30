//! KNOWN UPSTREAM DEFECTS —— 本文件钉的是 occt-wasm（本 fork）的上游缺陷，不是本平台的 bug。为绿 = 缺陷仍在；变红 = 上游可能已修好，那是喜讯，请翻转本用例。
//!
//! ## 为什么这些钉子"为绿"反而是对的
//!
//! `occt-wasm` 本 fork 的 **STEP 出口**、**STEP 入口**、**XCAF → glTF 出口**在当前
//! 状态下**不可用**：调用它们会在 wasm 层触发 wasm trap（实测文本为
//! `WASM runtime error: error while executing at wasm backtrace: ...`；早先版本曾观察到
//! `wasm trap: uninitialized element`，偶有 `out of bounds memory access`），
//! 表现为**返回 `Err`**——而**不是** panic。
//!
//! 因此本文件用 `expect_err(...)` 断言"它们仍然失败"：
//! - **为绿** = 上游缺陷仍在（符合当前 capability 声明，别动）；
//! - **变红** = 上游可能已修好 → 那是喜讯，请更新 `core-interop` 的 capability 声明
//!   并翻转/删除本钉子。
//!
//! ## 为什么可以在这里 `use occt_wasm::...`
//!
//! 本文件是 crate 内 `#[cfg(test)]` 模块（在 `lib.rs` 里以
//! `#[cfg(all(test, feature = "occt"))] #[path = "known_upstream_defects.rs"]
//! mod zz_known_upstream_defects;` 声明，见下），**不在公开 API 面**内——
//! 而铁律①约束的正是**公开 API 面**。我们**不**为了测试去给内核门面新增
//! `export_step` / `import_step` / `xcaf_export_gltf` 方法：那会造出一批
//! **永远拿不到真实能力、却看起来像产品能力**的死 API。
//!
//! ## ⚠ 绝不使用 `#[should_panic]`
//!
//! 上游缺陷在本 crate 表现为**返回 `Err`**（`Err(WASM runtime error: ...)`），
//! **不是 panic**。用 `#[should_panic]` 必然失败。
//!
//! ## ⚠ 为什么本模块**共用** `occt.rs` 的那一具内核、且**排在最后**执行
//!
//! 两件事都为了绕开同一个坑（实测）：
//!
//! 1. **共用一具内核**：`OcctKernel::new()` 要 JIT 编译 ~21 MB WASM，且实测
//!    即便 `Drop` 之后其原生内存也**不立刻归还**。若本模块自建内核，就会和
//!    `occt.rs` 的探针内核（乃至探针里 `a_facade_new_returns_the_real_backend`
//!    再建的第二具）叠加到 OOM。故这里一律经由 [`crate::occt::shared_test_backend`]
//!    取**全 crate 唯一**的那一份，**零额外内核分配**。
//! 2. **排在最后执行**：wasm trap 会让被调用内核的线性内存**长大且不回收**。
//!    libtest **按测试全名字典序**执行；本模块名以 `zz_` 打头（见 `lib.rs`），
//!    字典序落在 `types` 之后，从而**排在 `occt::tests::a_facade_new_returns_the_real_backend`
//!    之后**。否则 trap 会先污染内存，让那条会**再建一具**内核的用例在分配数 MB 时
//!    `memory allocation ... failed`，进程以 `0xc0000409 (STATUS_STACK_BUFFER_OVERRUN)`
//!    中止（日志里**没有** FAILED 行，极易被误读成代码回归）。
//!
//! ⚠ 因为一具内核约 21 MB，本 crate 的 occt 腿**必须串行**执行
//! （`--test-threads=1`）——多具内核并发必然 OOM（见仓库铁律"同一时刻只跑一个 occt"）。

use std::sync::MutexGuard;

use crate::occt::OcctBackend;

/// 借用**全 crate 唯一**的 vendor 内核后端（与 `occt.rs` 探针共用一份）。
///
/// **刻意不跳过**：内核若连实例化都失败，那属于**另一类**问题（由 `occt.rs` 的
/// `occt_feature_must_actually_instantiate_the_kernel` 门禁负责），这里直接
/// `expect` 响亮失败，绝不用"跳过"把没验证伪装成通过。
fn shared() -> MutexGuard<'static, OcctBackend> {
    crate::occt::shared_test_backend()
        .expect("真实内核必须能实例化；连它都失败说明不是本钉子要钉的问题")
}

/// **上游缺陷钉子 ①：STEP 出口不可用**（`export_step` 触发 wasm trap）。
#[test]
fn export_step_traps_upstream() {
    let mut b = shared();
    let k = b.kernel_mut();
    let h = k.make_box(10.0, 20.0, 30.0).expect("make_box 应可用");
    let err = k
        .export_step(h)
        .expect_err("上游缺陷已修复？请更新 capability 声明并删除本钉子（这是喜讯，不是回归）");
    assert_trap_text(&err, "uninitialized element");
}

/// **上游缺陷钉子 ②：STEP 入口不可用**（`import_step` 触发 wasm trap）。
#[test]
fn import_step_traps_upstream() {
    let mut b = shared();
    let k = b.kernel_mut();
    // 一段结构上合法的 STEP 头（内容正确与否无关紧要：trap 发生在进入解析之前）。
    let step = "ISO-10303-21;\n\
                HEADER;\n\
                FILE_DESCRIPTION((''),'2;1');\n\
                FILE_NAME('a.step','',(''),(''),'','','');\n\
                FILE_SCHEMA(('AUTOMOTIVE_DESIGN'));\n\
                ENDSEC;\n\
                DATA;\n\
                ENDSEC;\n\
                END-ISO-10303-21;\n";
    let err = k
        .import_step(step)
        .expect_err("上游缺陷已修复？请更新 capability 声明并删除本钉子（这是喜讯，不是回归）");
    assert_trap_text(&err, "uninitialized element");
}

/// **上游缺陷钉子 ③：XCAF → glTF 出口不可用**（`xcaf_export_gltf` 触发 wasm trap）。
///
/// ⚠ 实测：本钉子触发的 **trap 文本与上面两条不同**——是
/// `wasm trap: out of bounds memory access`（其 `Debug` 为 `MemoryOutOfBounds`），
/// **不是** `uninitialized element`。该差异**与本路径固有相关**，**非执行顺序所致**：
/// 单独运行本用例（全新内核）复现同样的 `out of bounds`（多次运行一致）。故本钉子
/// 钉的是它自己的那条文字，强度同样严格（不许放宽成只判 `wasm`）。
#[test]
fn xcaf_export_gltf_traps_upstream() {
    let mut b = shared();
    let k = b.kernel_mut();
    let doc = k
        .xcaf_new_document()
        .expect("xcaf_new_document 应可用（若它也 trap，则属另一条上游缺陷，请单列）");
    let err = k
        .xcaf_export_gltf(doc, 0.1, 0.5)
        .expect_err("上游缺陷已修复？请更新 capability 声明并删除本钉子（这是喜讯，不是回归）");
    assert_trap_text(&err, "out of bounds memory access");
}

/// 把错误的**完整 `source()` 链**（逐层的 `Display` 与 `Debug`）拼成一段文本。
///
/// # 为什么必须走 `source()` 链，而不是只看顶层的 `Display`
///
/// `OcctError::Runtime(wasmtime::Error)` 的 `Display` 来自
/// `thiserror` 的 `#[error("WASM runtime error: {0}")]`，而 `wasmtime::Error` 是
/// `anyhow` 风格：它的 `Display` **只打印顶层上下文**——
/// `WASM runtime error: error while executing at wasm backtrace: ...`——
/// **不含**真正的 wasm trap 文本。trap 文本位于其 `source()` 链的更深一层
/// （STEP 两钉在 `[1]`，glTF 钉在 `[2]`，见各用例实测）。
///
/// 曾因**只看顶层 `Display`** 而误判"错误文本已漂移"，实为**观察位置不对**：
/// 走 `source()` 链即可看到稳定出现的 trap 文本，无需任何放宽。
fn error_chain_text(err: &(dyn std::error::Error + 'static)) -> String {
    let mut out = format!("{err}\n{err:?}");
    let mut cur = err.source();
    while let Some(e) = cur {
        out.push_str(&format!("\n{e}\n{e:?}"));
        cur = e.source();
    }
    out
}

/// 断言错误链里含**指定的 wasm trap 文本**（严格匹配，**不允许**放宽成只判 `wasm`）。
///
/// 期望文本来自**实测的确定事实**（同一机器多次运行一致）：
/// - STEP 出口 / STEP 入口 → `uninitialized element`；
/// - XCAF→glTF 出口 → `out of bounds memory access`（与本路径固有相关，非顺序所致）。
fn assert_trap_text(err: &(dyn std::error::Error + 'static), expected: &str) {
    let chain = error_chain_text(err);
    assert!(
        chain.contains(expected),
        "上游缺陷钉子失配：期望错误链含 `{expected}`，但完整错误链里没有。\n\
         若上游确已修复，请更新 capability 声明并删除本钉子（这是喜讯，不是回归）。\n\
         实得完整错误链（Display + Debug，逐层）：\n{chain}"
    );
}
