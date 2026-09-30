//! 图纸层的持久标识符。
//!
//! **为什么这里另起一套 id，而不复用 `core-model` 的 `ElementId`**：
//! `Entity2D` / `Annotation` 是"画在图纸上的东西"，与"模型里的构件"是两类对象；
//! 一个构件可以在三张视图里各有一条投影线，这三条线必须有各自的 id。混用会导致
//! "删构件把图纸线也删了"这类事故。
//!
//! 但仍遵守同一条铁律：**持久 id 永不复用、一律 UUID v7**（设计文档 3.3）。

use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// 图纸内的图元 / 标注标识（持久态，随 `.bimdoc` 的 `views/*.json` 存盘）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EntityId(pub Uuid);

impl EntityId {
    /// 生成新 id（UUID v7，时间有序）。
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    /// 由已有 UUID 构造。
    #[must_use]
    pub const fn from_uuid(u: Uuid) -> Self {
        Self(u)
    }

    /// 取出底层 UUID。
    #[must_use]
    pub const fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl Default for EntityId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for EntityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entity_ids_are_unique_and_serializable() {
        let a = EntityId::new();
        let b = EntityId::new();
        assert_ne!(a, b);
        let s = serde_json::to_string(&a).unwrap();
        assert_eq!(serde_json::from_str::<EntityId>(&s).unwrap(), a);
    }
}
