//! 标注规则（设计文档 9.2 / 6.3）—— 模块侧"标什么"，Core 侧"怎么画"。
//!
//! ## 为什么这里只声明"需要什么"，不生成图元
//!
//! 标注的**实体生成**（尺寸界线、箭头、文字摆放）在 `core-doc`。
//! 而 `core-sdk` 只依赖 `core-model`（不许依赖 `core-doc`）。
//!
//! 所以模块能做的、也应该做的，是声明语义需求：
//!
//! ```text
//! 模块说：  "桥架需要标注宽度，引用轴网定位，保留 0 位小数"
//!            ↓  AnnotationContext
//! Core 做：  查 DraftingStandard 拿箭头大小/文字样式/图层
//!            → 生成尺寸界线与文字
//!            → 建 Annotation::bound_to(GeometryKey) 绑定模型拓扑
//!            → 登记进 DependencyGraph，模型改了自动更新
//! ```
//!
//! 这样"专业标注习惯"留在模块，"标注实现"留在 Core，**两边都不用改对方的代码**。

use serde::{Deserialize, Serialize};

/// 模块需要的标注类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AnnotationNeed {
    /// 线性尺寸（长度 / 间距）
    LinearDimension,
    /// 角度尺寸
    AngularDimension,
    /// 半径 / 直径尺寸
    RadialDimension,
    /// 标高标注（**依据基准元素**）
    ElevationTag,
    /// 轴号标注（**依据基准元素**）
    GridTag,
    /// 文字说明
    TextNote,
    /// 引线标注
    Leader,
}

impl AnnotationNeed {
    /// 中文名（界面展示用）。
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::LinearDimension => "线性标注",
            Self::AngularDimension => "角度标注",
            Self::RadialDimension => "半径/直径标注",
            Self::ElevationTag => "标高标注",
            Self::GridTag => "轴号标注",
            Self::TextNote => "文字说明",
            Self::Leader => "引线标注",
        }
    }

    /// 是否**必须依据基准元素**（标高 / 轴号）。
    ///
    /// 这类标注不随几何改，随基准元素改——与 [`AnnotationNeed`] 的几何类相对。
    #[must_use]
    pub const fn requires_datum(self) -> bool {
        matches!(self, Self::ElevationTag | Self::GridTag)
    }

    /// 是否随**模型几何**更新（决定要不要建 `BoundToGeometry` 关系）。
    #[must_use]
    pub const fn tracks_geometry(self) -> bool {
        matches!(
            self,
            Self::LinearDimension | Self::AngularDimension | Self::RadialDimension
        )
    }
}

/// 标注上下文：模块声明的"我需要这样一个标注"。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnnotationContext {
    /// 需要哪种标注
    pub need: AnnotationNeed,
    /// 是否依据基准元素
    pub requires_datum: bool,
    /// 小数位（`None` = 跟随 `DraftingStandard`）
    pub decimals: Option<u8>,
    /// 文字样式名（须是 `DraftingStandard` 里已定义的样式名）
    pub text_style: String,
    /// 说明（界面提示）
    pub description: String,
}

impl AnnotationContext {
    /// 以标注类型构造，文字样式取国标默认 `"标准"`。
    #[must_use]
    pub fn new(need: AnnotationNeed) -> Self {
        Self {
            need,
            requires_datum: need.requires_datum(),
            decimals: None,
            text_style: "标准".to_string(),
            description: String::new(),
        }
    }

    /// 指定小数位（链式）。
    #[must_use]
    pub const fn with_decimals(mut self, decimals: u8) -> Self {
        self.decimals = Some(decimals);
        self
    }

    /// 指定文字样式（链式）。
    #[must_use]
    pub fn with_text_style(mut self, style: impl Into<String>) -> Self {
        self.text_style = style.into();
        self
    }

    /// 说明（链式）。
    #[must_use]
    pub fn with_description(mut self, text: impl Into<String>) -> Self {
        self.description = text.into();
        self
    }

    /// 该标注需要绑定模型拓扑吗。
    #[must_use]
    pub const fn binds_geometry(&self) -> bool {
        self.need.tracks_geometry()
    }
}

/// 标注规则：模块对"这个专业构件该标什么"的声明。
pub trait AnnotationRule: Send + Sync {
    /// 规则 id（全局唯一，建议带模块前缀，如 `"ceec.tray.width"`）。
    fn id(&self) -> &str;

    /// 界面显示名。
    fn display_name(&self) -> &str;

    /// 需要哪种标注。
    fn need(&self) -> AnnotationNeed;

    /// 是否依据基准元素；默认由 [`AnnotationNeed::requires_datum`] 推出。
    fn requires_datum(&self) -> bool {
        self.need().requires_datum()
    }

    /// 组装上下文。
    fn context(&self) -> AnnotationContext {
        AnnotationContext {
            need: self.need(),
            requires_datum: self.requires_datum(),
            decimals: None,
            text_style: "标准".to_string(),
            description: String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TrayWidthRule;

    impl AnnotationRule for TrayWidthRule {
        fn id(&self) -> &'static str {
            "ceec.tray.width"
        }
        fn display_name(&self) -> &'static str {
            "桥架宽度标注"
        }
        fn need(&self) -> AnnotationNeed {
            AnnotationNeed::LinearDimension
        }
        fn context(&self) -> AnnotationContext {
            AnnotationContext::new(AnnotationNeed::LinearDimension).with_decimals(0)
        }
    }

    struct LevelRule;

    impl AnnotationRule for LevelRule {
        fn id(&self) -> &'static str {
            "ceec.level.elev"
        }
        fn display_name(&self) -> &'static str {
            "标高标注"
        }
        fn need(&self) -> AnnotationNeed {
            AnnotationNeed::ElevationTag
        }
    }

    #[test]
    fn dimension_needs_track_geometry() {
        assert!(AnnotationNeed::LinearDimension.tracks_geometry());
        assert!(!AnnotationNeed::LinearDimension.requires_datum());
        assert!(AnnotationContext::new(AnnotationNeed::LinearDimension).binds_geometry());
    }

    #[test]
    fn elevation_and_grid_tags_are_datum_based() {
        assert!(AnnotationNeed::ElevationTag.requires_datum());
        assert!(AnnotationNeed::GridTag.requires_datum());
        assert!(!AnnotationNeed::ElevationTag.tracks_geometry());
    }

    #[test]
    fn rule_defaults_follow_the_need() {
        let c = LevelRule.context();
        assert!(c.requires_datum, "标高标注应自动带上\"依据基准元素\"");
        assert!(!c.binds_geometry());
        assert_eq!(c.text_style, "标准");
    }

    #[test]
    fn rule_can_override_context_fields() {
        let c = TrayWidthRule.context();
        assert_eq!(c.decimals, Some(0));
        assert!(c.binds_geometry());
    }

    #[test]
    fn context_not_built_by_hand_still_derives_requires_datum() {
        let c = AnnotationContext::new(AnnotationNeed::GridTag);
        assert!(c.requires_datum);
    }

    #[test]
    fn labels_are_available_for_ui() {
        assert_eq!(AnnotationNeed::GridTag.label(), "轴号标注");
        assert_eq!(AnnotationNeed::RadialDimension.label(), "半径/直径标注");
    }

    #[test]
    fn context_is_serializable_for_module_declarations() {
        let c = AnnotationContext::new(AnnotationNeed::LinearDimension)
            .with_decimals(2)
            .with_text_style("小字")
            .with_description("桥架宽度");
        let json = serde_json::to_string(&c).unwrap();
        assert_eq!(serde_json::from_str::<AnnotationContext>(&json).unwrap(), c);
    }
}
