//! 线程本地内核池（设计文档 3.2，性能关键）。
//!
//! 源码事实：`OcctKernel::new()` 会 brotli 解压并 JIT 编译约 21 MB WASM，
//! **耗时约 100–500 ms**；而单次调用很快。occt-wasm 官方建议：
//!
//! > Re-use one `OcctKernel` per worker thread; creation is the slow part,
//! > individual calls are cheap.
//!
//! 因此规矩是：**一个线程一个内核实例，绝不每次操作用完就丢**。
//!
//! ## 这不只是性能优化，是硬约束（M0 实测）
//!
//! M0 的探针测试曾让 N 个并行用例各建一个内核，直接 OOM
//! （`memory allocation of 1048576 bytes failed`，退出码 `0xc0000409`）。
//! 一个内核实例就是一份解压后的 21 MB WASM + 一份 JIT 代码。
//!
//! ## 两个入口的区别（**这层语义很重要**）
//!
//! | 入口 | 保证 | 拿不到时 |
//! |---|---|---|
//! | [`with_kernel`] | **真实**内核（`BackendKind::Real`） | 报错，**绝不**装作成功 |
//! | [`with_mock_kernel`] | 能用就行（真实优先，退而求其次用占位） | 造一个占位后端 |
//!
//! `with_kernel` 若发现槽位里是占位后端，会**把它替换掉**——否则
//! "先有人用占位内核跑过，之后所有人 `with_kernel` 都拿到占位内核"
//! 就是一个不报错的静默降级，正好违反本项目"不许假装成功"的原则。

use std::cell::RefCell;

use crate::backend::BackendKind;
use crate::error::{KernelError, KernelResult};
use crate::handle::ShapeHandle;
use crate::kernel::Kernel;

thread_local! {
    static KERNEL: RefCell<Option<Kernel>> = const { RefCell::new(None) };
}

/// 调用内核池的代码**又**调了一次内核池（`RefCell` 重入）时的错误消息。
const RECURSION_HINT: &str = "内核池重入：持有内核期间不得再次调用内核池（把内核用完再借下一次）";

/// 在**当前线程的真实内核**上执行 `f`；内核每线程只创建一次。
///
/// 槽位里若是占位后端（[`crate::MockBackend`]），会被**丢弃并替换**为真实内核——
/// 见模块文档"两个入口的区别"。真实内核不可用（未开 `--features occt`、
/// 或 WASM 加载失败）时返回 [`KernelError::Unsupported`]，**不会退回占位后端**。
///
/// # 注意
///
/// - 真实内核首次创建耗时 **100–500 ms**，且 debug 构建慢约 100 倍；
/// - `f` 执行期间内核被借出，同一线程内**不得**再调 [`with_kernel`] /
///   [`with_mock_kernel`] / [`live_count`] 等（会返回 [`KernelError::NotInitialized`]）。
///   需要嵌套请用 [`Kernel::scope`]，它在一把借用里完成。
pub fn with_kernel<R>(f: impl FnOnce(&mut Kernel) -> R) -> KernelResult<R> {
    // 第一步：槽位里已经有真实内核吗？（只读借用，用完立刻放掉）
    let has_real = KERNEL.with(|cell| {
        let slot = cell
            .try_borrow()
            .map_err(|_| KernelError::NotInitialized(RECURSION_HINT.to_string()))?;
        Ok(slot.as_ref().is_some_and(|k| k.is_real()))
    })?;

    if !has_real {
        // 丢弃占位内核（若有），然后在**不持有借用**的前提下创建真实内核。
        // 不持借用是刻意的：创建过程较长，持有借用会让"重入"变成 panic 而不是错误。
        KERNEL.with(|cell| {
            *cell.borrow_mut() = None;
        });
        let fresh = Kernel::new()?;
        KERNEL.with(|cell| {
            *cell.borrow_mut() = Some(fresh);
        });
    }

    // 第二步：借出使用。
    KERNEL.with(|cell| {
        let mut slot = cell
            .try_borrow_mut()
            .map_err(|_| KernelError::NotInitialized(RECURSION_HINT.to_string()))?;
        let kernel = slot
            .as_mut()
            .ok_or_else(|| KernelError::NotInitialized("线程本地内核槽位异常".to_string()))?;
        Ok(f(kernel))
    })
}

/// 在**当前线程的可用内核**上执行 `f`；没有就装一个占位后端。
///
/// **真实内核优先**：若该线程已经装入了真实内核，直接复用它——
/// 真实内核总比占位后端强，没有理由退回去。
///
/// 供无 WASM 环境（CLI、单元测试、文档构建）使用。需要真实几何时改用
/// [`with_kernel`]。
pub fn with_mock_kernel<R>(f: impl FnOnce(&mut Kernel) -> R) -> R {
    KERNEL.with(|cell| {
        let mut slot = cell.borrow_mut();
        let kernel = slot.get_or_insert_with(Kernel::mock);
        f(kernel)
    })
}

/// 丢弃当前线程的内核实例（测试收尾、或需要强制重建时用）。
pub fn reset_kernel() {
    KERNEL.with(|cell| {
        *cell.borrow_mut() = None;
    });
}

/// 当前线程是否已装入内核实例（无论真实还是占位）。
#[must_use]
pub fn is_loaded() -> bool {
    KERNEL.with(|cell| cell.borrow().is_some())
}

/// 当前线程装入的**后端种类**；未装入时为 `None`。
///
/// 诊断与断言用：想知道"这个线程到底在用什么内核"时看它，
/// 不要去看后端名字符串。
#[must_use]
pub fn current_kind() -> Option<BackendKind> {
    KERNEL.with(|cell| cell.borrow().as_ref().map(Kernel::kind))
}

/// 当前线程装入的后端名（`"occt-wasm"` / `"mock"`）；未装入时为 `None`。
#[must_use]
pub fn current_backend() -> Option<&'static str> {
    KERNEL.with(|cell| cell.borrow().as_ref().map(Kernel::backend_name))
}

/// 当前线程的存活形状数与句柄清单（泄漏自查）；未装入时为 `None`。
///
/// 借不出内核时（正在 `with_kernel` 的 `f` 里调用）返回 `None`——
/// **不 panic**，便于把它放进日志与断言里。
#[must_use]
pub fn live_handles() -> Option<Vec<ShapeHandle>> {
    KERNEL.with(|cell| {
        cell.try_borrow()
            .ok()
            .and_then(|slot| slot.as_ref().map(Kernel::live_handles))
    })
}

/// 当前线程的存活形状数；未装入或借不出时为 `None`。
#[must_use]
pub fn live_count() -> Option<usize> {
    live_handles().map(|v| v.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(not(feature = "occt"))]
    fn real_backend_is_reported_as_unsupported_for_now() {
        reset_kernel();
        let r = with_kernel(|k| k.backend_name());
        assert!(matches!(r, Err(KernelError::Unsupported(_))));
    }

    #[test]
    fn mock_kernel_is_reused_within_thread() {
        reset_kernel();
        let first = with_mock_kernel(|k| k.make_box(1.0, 1.0, 1.0).unwrap());
        // 第二次调用应复用同一个内核实例：第一次分配的形状仍然存活
        let still_alive = with_mock_kernel(|k| k.is_alive(first));
        assert!(still_alive, "同一线程内应复用内核实例");
        reset_kernel();
    }

    #[test]
    fn is_loaded_tracks_state() {
        reset_kernel();
        assert!(!is_loaded());
        with_mock_kernel(|_| ());
        assert!(is_loaded());
        reset_kernel();
        assert!(!is_loaded());
    }

    #[test]
    fn current_kind_exposes_what_is_actually_installed() {
        reset_kernel();
        assert_eq!(current_kind(), None);
        assert_eq!(current_backend(), None);

        with_mock_kernel(|_| ());
        assert_eq!(current_kind(), Some(BackendKind::Mock));
        assert_eq!(current_backend(), Some("mock"));
        reset_kernel();
    }

    /// **核心回归测试**：占位内核不得冒充真实内核。
    ///
    /// 旧实现里 `with_kernel` 只检查"槽位是不是空的"，于是先跑过
    /// `with_mock_kernel` 的线程，之后每一次 `with_kernel` 都会**成功返回一个占位内核**。
    /// 那是最坏的一种 bug：不报错、结果不可信、只在换后端时炸。
    #[test]
    #[cfg(not(feature = "occt"))]
    fn with_kernel_must_not_accept_a_leftover_mock_kernel() {
        reset_kernel();
        with_mock_kernel(|k| k.make_box(1.0, 1.0, 1.0).unwrap());
        assert_eq!(current_kind(), Some(BackendKind::Mock));

        // 未启用 occt 时真实内核不可用 → 必须**报错**，而不是把占位内核交出去
        assert!(matches!(
            with_kernel(|k| k.backend_name()),
            Err(KernelError::Unsupported(_))
        ));
        reset_kernel();
    }

    #[test]
    fn live_handles_reports_none_when_nothing_is_loaded() {
        reset_kernel();
        assert_eq!(live_handles(), None);
        assert_eq!(live_count(), None);

        with_mock_kernel(|k| k.make_box(1.0, 1.0, 1.0).unwrap());
        assert_eq!(live_count(), Some(1));
        reset_kernel();
        assert_eq!(live_count(), None);
    }

    /// 在 `with_*` 的闭包里再调内核池 → 必须返回错误，**不得 panic**。
    #[test]
    fn reentrant_pool_access_returns_an_error_instead_of_panicking() {
        reset_kernel();
        let inner = with_mock_kernel(|_| live_count());
        assert_eq!(inner, None, "持有内核时查询存活数应得到 None，而不是 panic");
        // 真实入口同样要给出可读错误
        #[cfg(not(feature = "occt"))]
        {
            reset_kernel();
            let nested = with_mock_kernel(|_| with_kernel(|k| k.backend_name()));
            assert!(
                matches!(&nested, Err(KernelError::NotInitialized(msg)) if msg.contains("重入")),
                "重入应报 NotInitialized 并说明原因，实得 {nested:?}"
            );
        }
        reset_kernel();
    }
}
