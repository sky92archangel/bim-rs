//! # core-testkit —— 测试工具箱
//!
//! 各 crate 的单元测试只测自己的一亩三分地；**跨层的一致性**没人测，就会在
//! 集成时才炸。本 crate 提供两样东西：
//!
//! | 模块 | 作用 |
//! |---|---|
//! | [`fixtures`] | 固定夹具：文档、元素、属性模式。多处复用，避免每个测试各造一套 |
//! | [`invariants`] | 跨层不变式断言：配方有序、文档可 JSON 往返、id 不复用 |
//!
//! **只被 `[dev-dependencies]` 使用**，不进任何生产二进制的依赖图。
//!
//! ## 为什么夹具要放进 crate 而不是各测试文件
//!
//! 一旦两个测试对"一个典型文档"的假设不同，测试之间就会打架。
//! 把假设收敛到一处，改一次全场同步——这是黄金样本（golden sample）能立住的前提。

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::panic))]

pub mod fixtures;
pub mod invariants;

pub use fixtures::{DEMO_CATEGORY, demo_document, demo_element, demo_schema};
pub use invariants::{
    assert_document_roundtrips_json, assert_recipe_is_executable,
    assert_registry_summary_consistent,
};
