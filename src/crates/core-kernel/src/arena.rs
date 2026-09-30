//! 槽位 arena —— **会话态句柄**的分配与回收（设计文档 3.3）。
//!
//! ## 为什么必须自己造一个
//!
//! `occt-wasm` 的 `ShapeHandle` 字段是 `pub(crate)`，**无法从裸 `u32` 重建**
//! （M0 源码级事实 ①）。所以适配层必须自己持有一张槽位表，把内核对外的
//! `ShapeHandle(idx)` 与内核真实句柄对应起来。
//!
//! 顺带得到三件内核**没有**提供的东西：
//!
//! | 能力 | 内核 | 本 arena |
//! |---|---|---|
//! | `is_alive` | ❌（只有 `is_null`） | ✅ |
//! | 存活计数 | 只有全局 `get_shape_count` | ✅ 按槽位统计 |
//! | 重复释放检测 | ❌ | ✅ 返回 [`KernelError::InvalidHandle`] |
//!
//! ## 两个后端共用同一份实现
//!
//! [`MockBackend`](crate::MockBackend) 与 [`OcctBackend`](crate::OcctBackend)
//! 的"槽位分配 / 复用 / 释放 / 存活判定"语义必须**完全一致**，否则上层的
//! 缓存（`core-app::GeometryService`）会在换后端时出现只在一边复现的怪 bug。
//! 因此把它抽成泛型 [`SlotArena<T>`]，两边都只提供 `T` 与"释放时怎么通知内核"。

use crate::error::{KernelError, KernelResult};
use crate::handle::ShapeHandle;

/// 槽位表：对外 `ShapeHandle.raw()` 就是这里的下标。
///
/// 泛型参数 `T` 是"这个后端内部表示形状的东西"——
/// 占位后端放一个简化结构体，真实后端放 `occt_wasm::ShapeHandle`。
#[derive(Debug)]
pub struct SlotArena<T> {
    /// 槽位内容；`None` 表示该槽位已释放（或从未使用）
    slots: Vec<Option<T>>,
    /// 空闲槽位（后进先出，复用以模拟 arena 语义）
    free: Vec<u32>,
}

// 手写 `Default`：派生会给 `T` 加上 `T: Default` 的额外约束，
// 而空 arena 与 `T` 无关（两个字段一个是空 `Vec<Option<T>>`、一个是空 `Vec<u32>`）。
impl<T> Default for SlotArena<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> SlotArena<T> {
    /// 空 arena。
    #[must_use]
    pub const fn new() -> Self {
        Self {
            slots: Vec::new(),
            free: Vec::new(),
        }
    }

    /// 取一个句柄给 `value`：优先复用空闲槽位，否则追加新槽位。
    pub fn alloc(&mut self, value: T) -> ShapeHandle {
        if let Some(idx) = self.free.pop() {
            if let Some(slot) = self.slots.get_mut(idx as usize) {
                *slot = Some(value);
            }
            ShapeHandle(idx)
        } else {
            let idx = self.slots.len() as u32;
            self.slots.push(Some(value));
            ShapeHandle(idx)
        }
    }

    /// 只读取出句柄对应的值。
    pub fn get(&self, handle: ShapeHandle) -> KernelResult<&T> {
        match self
            .slots
            .get(handle.raw() as usize)
            .and_then(|s| s.as_ref())
        {
            Some(v) => Ok(v),
            None => Err(KernelError::InvalidHandle(handle)),
        }
    }

    /// 可变取出句柄对应的值。
    pub fn get_mut(&mut self, handle: ShapeHandle) -> KernelResult<&mut T> {
        match self
            .slots
            .get_mut(handle.raw() as usize)
            .and_then(|s| s.as_mut())
        {
            Some(v) => Ok(v),
            None => Err(KernelError::InvalidHandle(handle)),
        }
    }

    /// **释放**：取出值并归还槽位。
    ///
    /// 这是所有释放路径的**唯一入口**——调用方拿到 `T` 之后要负责通知内核
    /// （真实后端调 `OcctKernel::release`，占位后端什么都不用做）。
    ///
    /// 重复释放 / 越界 → [`KernelError::InvalidHandle`]。内核自己**不做**这个检查，
    /// 所以这一层是"越界释放"的第一道（也是唯一一道）防线。
    pub fn take(&mut self, handle: ShapeHandle) -> KernelResult<T> {
        let idx = handle.raw() as usize;
        match self.slots.get_mut(idx).and_then(|s| s.take()) {
            Some(v) => {
                self.free.push(handle.raw());
                Ok(v)
            }
            None => Err(KernelError::InvalidHandle(handle)),
        }
    }

    /// 句柄是否仍然有效。
    #[must_use]
    pub fn is_alive(&self, handle: ShapeHandle) -> bool {
        self.slots
            .get(handle.raw() as usize)
            .is_some_and(Option::is_some)
    }

    /// 当前存活数量（泄漏自查用）。
    #[must_use]
    pub fn live_count(&self) -> usize {
        self.slots.iter().filter(|s| s.is_some()).count()
    }

    /// 当前所有存活句柄（升序）。用于"临时物回收"与泄漏自查：
    /// 对比进入/离开某段代码前后的两个集合，差集就是这段代码新分配的句柄。
    #[must_use]
    pub fn live_handles(&self) -> Vec<ShapeHandle> {
        self.slots
            .iter()
            .enumerate()
            .filter(|(_, s)| s.is_some())
            .map(|(i, _)| ShapeHandle(i as u32))
            .collect()
    }

    /// 槽位总容量（含已释放的坑位）——诊断用。
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.slots.len()
    }

    /// 空闲坑位数——诊断用。
    #[must_use]
    pub fn free_count(&self) -> usize {
        self.free.len()
    }

    /// **清空**，返回被丢弃的全部值（顺序 = 槽位顺序）。
    ///
    /// 调用方负责通知内核（真实后端要在这里补一次 `release_all`，否则内核侧
    /// 的 arena 会继续胀下去）。测试里"每个用例从干净状态出发"也走这里。
    pub fn clear(&mut self) -> Vec<T> {
        self.free.clear();
        self.slots.drain(..).flatten().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alloc_grows_then_reuses_freed_slots() {
        let mut a: SlotArena<i32> = SlotArena::new();
        assert_eq!(a.live_count(), 0);
        assert_eq!(a.capacity(), 0);

        let h0 = a.alloc(10);
        let h1 = a.alloc(20);
        assert_eq!((h0.raw(), h1.raw()), (0, 1));
        assert_eq!((a.live_count(), a.capacity()), (2, 2));

        // 释放 h0 → 下一个分配应复用它（后进先出）
        assert_eq!(a.take(h0).unwrap(), 10);
        assert_eq!(a.free_count(), 1);
        let h2 = a.alloc(30);
        assert_eq!(h2, h0, "释放的槽位应被复用");
        assert_eq!(a.capacity(), 2, "复用不应让容量增长");
        assert_eq!(*a.get(h2).unwrap(), 30);
    }

    #[test]
    fn double_release_and_out_of_range_are_rejected() {
        let mut a: SlotArena<i32> = SlotArena::new();
        let h = a.alloc(1);
        a.take(h).unwrap();
        assert_eq!(a.take(h), Err(KernelError::InvalidHandle(h)));
        // 从未使用过的下标同样无效
        assert_eq!(
            a.take(ShapeHandle(99)),
            Err(KernelError::InvalidHandle(ShapeHandle(99)))
        );
        assert!(!a.is_alive(h));
        assert_eq!(a.get(h), Err(KernelError::InvalidHandle(h)));
        assert_eq!(a.get_mut(h), Err(KernelError::InvalidHandle(h)));
    }

    #[test]
    fn stale_handle_that_was_reused_is_alive_but_points_elsewhere() {
        // 这一条钉住"句柄可复用"的**危险面**：释放后再分配，
        // 旧句柄会"复活"并指向新形状。所以上层绝不允许长期持有裸句柄
        // ——这正是持久态 ShapeId 存在的理由（设计文档 3.3）。
        let mut a: SlotArena<&'static str> = SlotArena::new();
        let old = a.alloc("旧形状");
        a.take(old).unwrap();
        let new = a.alloc("新形状");
        assert_eq!(old, new);
        assert!(a.is_alive(old), "旧句柄'复活'了——这不是 bug，是 arena 语义");
        assert_eq!(*a.get(old).unwrap(), "新形状");
    }

    #[test]
    fn live_handles_are_sorted_and_exclude_freed() {
        let mut a: SlotArena<i32> = SlotArena::new();
        let h0 = a.alloc(0);
        let h1 = a.alloc(1);
        let h2 = a.alloc(2);
        a.take(h1).unwrap();
        assert_eq!(a.live_handles(), vec![h0, h2]);
        assert_eq!(a.live_count(), 2);
    }

    #[test]
    fn clear_returns_all_values_and_resets_capacity() {
        let mut a: SlotArena<i32> = SlotArena::new();
        a.alloc(1);
        a.alloc(2);
        let dropped = a.clear();
        assert_eq!(dropped, vec![1, 2]);
        assert_eq!((a.live_count(), a.capacity(), a.free_count()), (0, 0, 0));
        // 清空后从 0 号槽重新开始
        assert_eq!(a.alloc(9).raw(), 0);
    }
}
