//! 修复类算子探针（M4 heal ops，occt 腿）：缝合 / 缝合并实体化 / 修复 / 同域合并。
//!
//! 运行：`cargo test -p core-app --features occt --release --test heal_ops -- --test-threads=1`
//!
//! ## 两条硬规矩（本文件刻意如此）
//!
//! 1. **每个用例开头必须** `assert!(kernel.is_real(), ...)`——内核不可用时本用例
//!    **不能判绿**。**禁止** `else { return }` 式提前退出，**禁止** `#[ignore]`：
//!    那会把"没验证"伪装成"通过"。
//! 2. 只依赖 `core-kernel` 门面的**公开 API**（`make_box` / `volume` /
//!    `sub_shape_count` / `sub_shapes` / `sew` / `sew_and_solidify` / `fix_shape` /
//!    `unify_same_domain`），不新造门面方法。
//!
//! ## 为什么**不用** `static` + `unsafe impl Send/Sync` 共享内核
//!
//! 每具 `Kernel` 内部是一份解压后的 ~21 MB WASM 加一份 wasmtime JIT 代码
//! （见 `core-kernel` 的 `pool.rs` / `occt.rs` 之实测），**多具并存必 OOM**。
//! 早先本文件用 `static Mutex<Kernel>` 共享一具，但 `Kernel` 内含
//! `Box<dyn KernelBackend>`（非 `Send`），迫使写 `unsafe impl Send/Sync`——
//! 那是**没有必要的强转**：本 crate 已提供**线程本地内核池**
//! [`core_kernel::pool::with_kernel`]，它**零 `unsafe`** 地保证"每线程一具、
//! 反复复用"。配合 `--test-threads=1`（全部用例跑在同一线程）即得**全进程唯一**
//! 的一具真实内核——正是我们要的，且不必触碰 `dyn` 的 `Send`。
//!
//! ## 语义钉（本文件的核心）
//!
//! - `sew` 产出**壳**（`solid_count == 0`）；
//! - `sew_and_solidify` 产出**单一实体**（`solid_count == 1`）。
//!
//! 这两个断言把"缝合不成实体"这条**实测语义**钉死：任何人把二者实现成等价的，
//! 这里立刻报红。

#![cfg(feature = "occt")]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::panic))]

use core_kernel::Kernel;
use core_kernel::pool::with_kernel;
use core_kernel::tolerance;
use core_kernel::types::SubShapeKind;

/// 在**线程本地内核池**里的那具真实内核上执行 `f`。
///
/// - **零 `unsafe`**：池用 `thread_local` 持有内核，配 `--test-threads=1`
///   即"每进程一具、全用例复用"；
/// - 池拿不到真实内核时**返回 `Err`**（`Unsupported`）——这里 `expect` **响亮失败**，
///   绝不静默退回 mock；用例内另有 `assert!(k.is_real())` 二次把关。
fn on_real_kernel<R>(f: impl FnOnce(&mut Kernel) -> R) -> R {
    with_kernel(f)
        .expect("启用 occt feature 后内核池必须给出真实内核；拿不到说明不是本用例要验的问题")
}

/// 相对误差判定（体积守恒类断言）。
fn close(a: f64, b: f64) -> bool {
    (a - b).abs() / b.abs().max(1.0) < 1e-6
}

/// **修复形状对合法盒应是保体积的**：`make_box` → `fix_shape` → 体积不变。
#[test]
fn fix_shape_on_valid_box_keeps_volume() {
    on_real_kernel(|k| {
        assert!(k.is_real(), "occt 内核不可用，本用例不能判绿");

        let b = k.make_box(10.0, 20.0, 30.0).unwrap();
        let v0 = k.volume(b).unwrap();
        assert!((v0 - 6000.0).abs() < 1e-6, "原始盒体积应约 6000，实得 {v0}");

        let fixed = k.fix_shape(b).unwrap();
        assert!(k.is_alive(fixed), "修复结果应是可用的新句柄");
        let v1 = k.volume(fixed).unwrap();
        assert!(
            close(v1, v0),
            "修复合法形状必须保体积：原始 {v0} vs 修复后 {v1}"
        );
    });
}

/// **语义钉 A**：`sew` 产出的是**壳 / 复合体**，不是实体。
///
/// 拆面（用现有门面 `sub_shapes(_, Face)` 枚举 box 的面）后缝合：
/// `sub_shape_count(Solid) == 0` 且 `sub_shape_count(Shell) >= 1`。
#[test]
fn sew_yields_shell_not_solid() {
    on_real_kernel(|k| {
        assert!(k.is_real(), "occt 内核不可用，本用例不能判绿");

        let b = k.make_box(10.0, 10.0, 10.0).unwrap();

        // 拆面：确认 box 可分解成面（缝合的语义输入就是这组面）。
        let faces = k.sub_shapes(b, SubShapeKind::Face).unwrap();
        assert!(!faces.is_empty(), "box 应能拆出面");

        let shell = k.sew(b, tolerance::POINT_COINCIDENT).unwrap();
        assert!(k.is_alive(shell), "缝合结果应是可用的新句柄");

        assert_eq!(
            k.sub_shape_count(shell, SubShapeKind::Solid).unwrap(),
            0,
            "sew 产出的是壳，**不含任何实体**（solid_count 必须为 0）"
        );
        assert!(
            k.sub_shape_count(shell, SubShapeKind::Shell).unwrap() >= 1,
            "sew 产出应至少含 1 个壳"
        );
    });
}

/// **语义钉 B**：`sew_and_solidify` 产出的是**单一实体**。
///
/// 同输入 → `sub_shape_count(Solid) == 1`。这条与上面的 `sew_yields_shell_not_solid`
/// 合起来把两者的**语义差**钉死。
#[test]
fn sew_and_solidify_yields_one_solid() {
    on_real_kernel(|k| {
        assert!(k.is_real(), "occt 内核不可用，本用例不能判绿");

        let b = k.make_box(10.0, 10.0, 10.0).unwrap();
        let faces = k.sub_shapes(b, SubShapeKind::Face).unwrap();
        assert!(!faces.is_empty(), "box 应能拆出面");

        let solid = k.sew_and_solidify(b, tolerance::POINT_COINCIDENT).unwrap();
        assert!(k.is_alive(solid), "缝合并实体化的结果应是可用的新句柄");
        assert_eq!(
            k.sub_shape_count(solid, SubShapeKind::Solid).unwrap(),
            1,
            "sew_and_solidify 应产出**恰好一个**实体"
        );
    });
}

/// **同域合并守恒体积**：`make_box` → `unify_same_domain` → 体积不变。
#[test]
fn unify_same_domain_keeps_volume() {
    on_real_kernel(|k| {
        assert!(k.is_real(), "occt 内核不可用，本用例不能判绿");

        let b = k.make_box(10.0, 20.0, 30.0).unwrap();
        let v0 = k.volume(b).unwrap();

        let unified = k.unify_same_domain(b).unwrap();
        assert!(k.is_alive(unified), "同域合并结果应是可用的新句柄");
        let v1 = k.volume(unified).unwrap();
        assert!(
            close(v1, v0),
            "同域合并必须守恒体积：原始 {v0} vs 合并后 {v1}"
        );
    });
}
