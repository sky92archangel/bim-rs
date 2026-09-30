//! 基准与定位元素（设计文档 4.6）—— **必须内置在 Core**。
//!
//! ## 为什么内置
//!
//! 轴网、标高、参考面、参考线是**全专业共用的定位地基**，不是某个专业的功能。
//! 判据一句话：**凡是"别的元素要靠它定位、视图要靠它生成"的，就是基础设施。**
//!
//! 四条理由（缺一个都成立）：① 坐标统一；② 定位宿主；③ 视图生成依赖
//! （平面图 = 某标高剖切 + 投影；立面 = 沿轴网方向；剖面 = 垂直某轴）；
//! ④ 标注基准（轴号标注、标高标注直接引用它们）。
//!
//! ## 核心原则：**基准 ≠ 几何**
//!
//! 一个基准元素横跨四层（设计文档 4.6.2）：
//!
//! | 层 | 内容 | 归属 |
//! |---|---|---|
//! | 语义层 | 轴号 / 名称、标高值、可见性、钉住 | **本模块（`core-model`）** |
//! | 几何层 | 轴网曲线、标高水平面、基准点 | `core-kernel`（经 [`DatumElement::geom_ref`]）|
//! | 显示层 | 轴号圈、标高符号 △、延长线 | `core-doc`（出图时渲染）|
//! | 关系层 | 谁挂在它上、哪些视图 / 标注引用它 | [`crate::relation`] |
//!
//! **最容易踩的坑**：把轴号文字、标高符号当几何画进模型。结果模型里全是文字图元，
//! 改一个轴号要在整图里找，视图联动全废。**轴号文字 / 标高符号不存几何**——
//! 这里只存 [`DatumElement::name`] 与 `extra["elevation"]`。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ids::{ElementId, RelationId, ShapeId};
use crate::property::Value;

/// `extra` 中的约定键：标高值（单位 mm）。
pub const KEY_ELEVATION: &str = "elevation";

/// `extra` 中的约定键：面层厚度（单位 mm）。
///
/// **建筑标高 vs 结构标高（真实坑）**：同一楼层有"建筑完成面（FFL）"与
/// "结构面（SSL）"两个标高，差一个面层厚度。不预留这个字段，后期改不动。
pub const KEY_SLAB_THICKNESS: &str = "slab_thickness";

/// `extra` 中的约定键：标高类型（`"architectural"` / `"structural"`）。
pub const KEY_LEVEL_KIND: &str = "level_kind";

/// 基准元素种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DatumKind {
    /// 轴网
    Grid,
    /// 标高
    Level,
    /// 参考面
    RefPlane,
    /// 参考线
    RefLine,
}

impl DatumKind {
    /// 建议的默认作用域（设计文档 4.6.3）。
    #[must_use]
    pub const fn default_scope(self) -> DatumScope {
        match self {
            Self::Grid | Self::Level | Self::RefPlane => DatumScope::Document,
            Self::RefLine => DatumScope::ViewLocal,
        }
    }

    /// 一阶段是否"内置且必须"（否则为"内置可选"）。
    ///
    /// 一阶段：Grid / Level **内置且必须**；RefPlane / RefLine **内置可选**。
    #[must_use]
    pub const fn required_in_phase_one(self) -> bool {
        matches!(self, Self::Grid | Self::Level)
    }
}

/// 作用域。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DatumScope {
    /// 文档级：跨会话持久化
    Document,
    /// 视图局部：只在某视图内有效
    ViewLocal,
    /// **仅会话**：如"设为当前工作平面"——**不写进 `.bimdoc`**（设计文档 4.6.3）
    Session,
}

/// 标高类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LevelKind {
    /// 建筑标高（建筑完成面，FFL）
    Architectural,
    /// 结构标高（结构面，SSL）
    Structural,
}

/// 视图可见性过滤。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewFilter {
    /// 平面视图可见
    pub plan: bool,
    /// 立面视图可见
    pub elevation: bool,
    /// 剖面视图可见
    pub section: bool,
    /// 三维视图可见
    pub three_d: bool,
}

impl ViewFilter {
    /// 全部可见。
    #[must_use]
    pub const fn all() -> Self {
        Self {
            plan: true,
            elevation: true,
            section: true,
            three_d: true,
        }
    }

    /// 全部不可见。
    #[must_use]
    pub const fn none() -> Self {
        Self {
            plan: false,
            elevation: false,
            section: false,
            three_d: false,
        }
    }
}

impl Default for ViewFilter {
    /// 默认：平 / 立 / 剖面可见，**三维不可见**（与常见制图习惯一致）。
    fn default() -> Self {
        Self {
            plan: true,
            elevation: true,
            section: true,
            three_d: false,
        }
    }
}

/// 一个基准 / 定位元素。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DatumElement {
    /// 持久 id（也是它在文档里的"身份证号"）
    pub id: ElementId,
    /// 种类（**模块禁止重定义**，设计文档 4.6.5）
    pub kind: DatumKind,
    /// 名称：轴号（`"3"` / `"A"`）或标高名（`"F2"`）
    ///
    /// ⚠ **文字不另存几何**，就靠这个字段；出图时由 `core-doc` 渲染符号。
    pub name: String,
    /// 作用域
    pub scope: DatumScope,
    /// 几何引用（轴网曲线 / 标高水平面 / 平面 / 直线）
    pub geom_ref: Option<ShapeId>,
    /// 是否钉住（防误拖）
    pub locked: bool,
    /// 在哪些视图可见
    pub visible_in: ViewFilter,
    /// 扩展语义（标高值、面层厚度、标高类型…）
    pub extra: BTreeMap<String, Value>,
    /// 关系 id
    pub relations: Vec<RelationId>,
}

impl DatumElement {
    fn base(kind: DatumKind, name: impl Into<String>) -> Self {
        Self {
            id: ElementId::new(),
            kind,
            name: name.into(),
            scope: kind.default_scope(),
            geom_ref: None,
            locked: false,
            visible_in: ViewFilter::default(),
            extra: BTreeMap::new(),
            relations: Vec::new(),
        }
    }

    /// 轴网。
    #[must_use]
    pub fn grid(name: impl Into<String>) -> Self {
        Self::base(DatumKind::Grid, name)
    }

    /// 标高（标高值单位 mm）。
    #[must_use]
    pub fn level(name: impl Into<String>, elevation_mm: f64) -> Self {
        let mut d = Self::base(DatumKind::Level, name);
        d.extra
            .insert(KEY_ELEVATION.to_string(), Value::Float(elevation_mm));
        d
    }

    /// 带面层厚度的**建筑标高**（建筑完成面 = 结构面 + 面层厚度）。
    #[must_use]
    pub fn level_with_slab(name: impl Into<String>, elevation_mm: f64, slab_mm: f64) -> Self {
        let mut d = Self::level(name, elevation_mm);
        d.extra
            .insert(KEY_SLAB_THICKNESS.to_string(), Value::Float(slab_mm));
        d.extra.insert(
            KEY_LEVEL_KIND.to_string(),
            Value::Text("architectural".to_string()),
        );
        d
    }

    /// 参考面。
    #[must_use]
    pub fn ref_plane(name: impl Into<String>) -> Self {
        Self::base(DatumKind::RefPlane, name)
    }

    /// 参考线。
    #[must_use]
    pub fn ref_line(name: impl Into<String>) -> Self {
        Self::base(DatumKind::RefLine, name)
    }

    /// 指定几何引用。
    #[must_use]
    pub fn with_geometry(mut self, shape: ShapeId) -> Self {
        self.geom_ref = Some(shape);
        self
    }

    /// 钉住。
    #[must_use]
    pub fn locked(mut self) -> Self {
        self.locked = true;
        self
    }

    /// 设置可见性。
    #[must_use]
    pub fn with_visibility(mut self, filter: ViewFilter) -> Self {
        self.visible_in = filter;
        self
    }

    /// 标高值（mm）。
    #[must_use]
    pub fn elevation(&self) -> Option<f64> {
        match self.extra.get(KEY_ELEVATION) {
            Some(Value::Float(v)) => Some(*v),
            _ => None,
        }
    }

    /// 面层厚度（mm）。
    #[must_use]
    pub fn slab_thickness(&self) -> Option<f64> {
        match self.extra.get(KEY_SLAB_THICKNESS) {
            Some(Value::Float(v)) => Some(*v),
            _ => None,
        }
    }

    /// 是否为"仅会话"作用域（此类**不写进 `.bimdoc`**）。
    #[must_use]
    pub const fn is_session_scoped(&self) -> bool {
        matches!(self.scope, DatumScope::Session)
    }

    /// 是否属于给定种类。
    #[must_use]
    pub fn is_kind(&self, kind: DatumKind) -> bool {
        self.kind == kind
    }
}

/// 制图约定：**内置 ≠ 写死**（设计文档 4.6.4）。
///
/// 轴号命名规则、标高类型与精度、各视图可见性都应可配置，供国标 / 院标覆盖。
/// （与设计文档 7.7 的 `DraftingStandard` 同思路。）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatumConvention {
    /// 轴号编号方式
    pub grid_numbering: GridNumbering,
    /// 轴号是否跳过易混字母 I / O / Z（GB/T 50001 惯用；设计文档 4.6.4 明确要求可配置）。
    ///
    /// 默认 `true`（跳过），符合国标；旧 JSON 缺该键时按此默认反序列化。
    #[serde(default = "default_skip_ambiguous_grid_letters")]
    pub skip_ambiguous_grid_letters: bool,
    /// 标高显示小数位
    pub level_decimals: u8,
    /// 标高前缀（如 `"±"`、`"EL+"`）
    pub level_prefix: String,
    /// 三维视图是否显示轴网
    pub show_grid_in_three_d: bool,
}

/// [`DatumConvention::skip_ambiguous_grid_letters`] 的 serde 默认值：`true`（跳过 I/O/Z）。
///
/// 独立成函数是为了给 `#[serde(default = "…")]` 引用（serde 需要路径而非字面量）。
fn default_skip_ambiguous_grid_letters() -> bool {
    true
}

/// 轴网方向（决定 [`GridNumbering::Mixed`] 下取数字还是字母）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GridDirection {
    /// 纵向轴（Mixed 下取数字）
    Vertical,
    /// 横向轴（Mixed 下取字母）
    Horizontal,
}

/// 轴号编号方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GridNumbering {
    /// 纯数字（`1 2 3 …`）
    Numeric,
    /// 纯字母（`A B C …`）
    Alphabetic,
    /// 数字与字母交替（纵向数字、横向字母）
    Mixed,
}

impl Default for DatumConvention {
    fn default() -> Self {
        Self {
            grid_numbering: GridNumbering::Mixed,
            skip_ambiguous_grid_letters: default_skip_ambiguous_grid_letters(),
            level_decimals: 3,
            level_prefix: "±".to_string(),
            show_grid_in_three_d: false,
        }
    }
}

impl DatumConvention {
    /// 按精度格式化标高值（输入 mm，输出以米为单位的标注文字）。
    ///
    /// 正值用 [`DatumConvention::level_prefix`]（如 `±`），负值用 `-`。
    #[must_use]
    pub fn format_level(&self, elevation_mm: f64) -> String {
        let metres = elevation_mm / 1000.0;
        let prefix = if metres < 0.0 {
            "-"
        } else {
            self.level_prefix.as_str()
        };
        format!(
            "{}{:.*}",
            prefix,
            usize::from(self.level_decimals),
            metres.abs()
        )
    }

    /// 纯函数：序号（`index`，**从 1 开始**）→ 轴号文本。无 IO、无状态、可测。
    ///
    /// 规则：
    /// - [`GridNumbering::Numeric`] → 数字串；
    /// - [`GridNumbering::Alphabetic`] → 字母串；
    /// - [`GridNumbering::Mixed`] → [`GridDirection::Horizontal`] 取字母、
    ///   [`GridDirection::Vertical`] 取数字（与既有"纵向数字、横向字母"语义一致）。
    ///
    /// 字母编码用**双射 base-K**（`1→A … 26→Z, 27→AA`）；字母表在
    /// [`DatumConvention::skip_ambiguous_grid_letters`] 为 `true` 时过滤掉 `I/O/Z`（K=23），
    /// 否则 K=26。
    ///
    /// `index == 0` 返回空串（无 0 号轴）。
    #[must_use]
    pub fn grid_label(&self, index: usize, direction: GridDirection) -> String {
        let use_letters = match self.grid_numbering {
            GridNumbering::Numeric => false,
            GridNumbering::Alphabetic => true,
            GridNumbering::Mixed => matches!(direction, GridDirection::Horizontal),
        };
        if use_letters {
            alphabetic_label(index, self.skip_ambiguous_grid_letters)
        } else {
            index.to_string()
        }
    }
}

/// 双射 base-K 字母编码（`1→A … 26→Z, 27→AA`；跳过 I/O/Z 时 K=23）。
///
/// 这是"序号 → 字母轴号"的唯一实现，务必**双射**（不是 Excel 式的进位）：
/// 逐位用 `(n-1) % k`，保证每个序号都有唯一且连续的一个字母串。
fn alphabetic_label(index: usize, skip_ambiguous: bool) -> String {
    const FULL: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let alphabet: Vec<u8> = FULL
        .iter()
        .copied()
        .filter(|&c| !(skip_ambiguous && matches!(c, b'I' | b'O' | b'Z')))
        .collect();
    let k = alphabet.len(); // 26 或 23
    if index == 0 || k == 0 {
        return String::new();
    }
    let mut n = index;
    let mut out: Vec<u8> = Vec::new();
    while n > 0 {
        let rem = (n - 1) % k;
        out.push(alphabet[rem]);
        n = (n - 1) / k;
    }
    out.reverse();
    // 字母表恒为 ASCII，故逐字节转 char 安全、无 panic 分支。
    out.iter().map(|&b| b as char).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_has_no_geometry_until_attached() {
        // 轴号只是 name，不是几何
        let g = DatumElement::grid("3");
        assert_eq!(g.name, "3");
        assert!(g.geom_ref.is_none());
        assert_eq!(g.kind, DatumKind::Grid);
        assert_eq!(g.scope, DatumScope::Document);
    }

    #[test]
    fn level_stores_elevation_in_extra() {
        let l = DatumElement::level("F2", 3600.0);
        assert_eq!(l.elevation(), Some(3600.0));
        assert!(l.slab_thickness().is_none());
    }

    #[test]
    fn architectural_level_reserves_slab_thickness() {
        // 建筑标高 = 结构面 + 面层厚度（真实坑，必须预留）
        let l = DatumElement::level_with_slab("F2-建筑", 3650.0, 50.0);
        assert_eq!(l.slab_thickness(), Some(50.0));
        assert_eq!(
            l.extra.get(KEY_LEVEL_KIND),
            Some(&Value::Text("architectural".to_string()))
        );
    }

    #[test]
    fn phase_one_required_kinds_are_grid_and_level() {
        assert!(DatumKind::Grid.required_in_phase_one());
        assert!(DatumKind::Level.required_in_phase_one());
        assert!(!DatumKind::RefPlane.required_in_phase_one());
        assert!(!DatumKind::RefLine.required_in_phase_one());
    }

    #[test]
    fn session_scope_is_flagged_for_persistence_skip() {
        let mut p = DatumElement::ref_plane("工作平面-1");
        assert!(!p.is_session_scoped(), "参考面默认是文档级");
        p.scope = DatumScope::Session;
        assert!(p.is_session_scoped(), "被设为当前工作平面后属于会话态");
    }

    #[test]
    fn convention_formats_levels_by_precision() {
        let c = DatumConvention::default();
        assert_eq!(c.format_level(3600.0), "±3.600");
        assert_eq!(c.format_level(-1500.0), "-1.500");
        assert_eq!(c.format_level(0.0), "±0.000");
    }

    #[test]
    fn datum_roundtrips_through_json() {
        let d = DatumElement::level_with_slab("F1", 0.0, 30.0);
        let s = serde_json::to_string(&d).unwrap();
        assert_eq!(serde_json::from_str::<DatumElement>(&s).unwrap(), d);
    }

    #[test]
    fn grid_label_matches_the_gb_table() {
        // 不跳过 I/O/Z（K=26）：1→A, 8→H, 9→I, 21→U, 26→Z
        let plain = DatumConvention {
            grid_numbering: GridNumbering::Alphabetic,
            skip_ambiguous_grid_letters: false,
            ..DatumConvention::default()
        };
        assert_eq!(plain.grid_label(1, GridDirection::Horizontal), "A");
        assert_eq!(plain.grid_label(8, GridDirection::Horizontal), "H");
        assert_eq!(plain.grid_label(9, GridDirection::Horizontal), "I");
        assert_eq!(plain.grid_label(21, GridDirection::Horizontal), "U");
        assert_eq!(plain.grid_label(26, GridDirection::Horizontal), "Z");

        // 跳过 I/O/Z（K=23）：9→J（跳过 I）、21→W（累计跳过 I,O）、26→AC（双射 base-23）
        let gb = DatumConvention {
            grid_numbering: GridNumbering::Alphabetic,
            skip_ambiguous_grid_letters: true,
            ..DatumConvention::default()
        };
        assert_eq!(gb.grid_label(1, GridDirection::Horizontal), "A");
        assert_eq!(gb.grid_label(8, GridDirection::Horizontal), "H");
        assert_eq!(gb.grid_label(9, GridDirection::Horizontal), "J");
        assert_eq!(gb.grid_label(21, GridDirection::Horizontal), "W");
        assert_eq!(gb.grid_label(26, GridDirection::Horizontal), "AC");
    }

    #[test]
    fn mixed_numbering_is_direction_dependent() {
        // Mixed：纵向（Vertical）取数字、横向（Horizontal）取字母。
        let mixed = DatumConvention::default();
        assert_eq!(mixed.grid_numbering, GridNumbering::Mixed);
        assert_eq!(mixed.grid_label(12, GridDirection::Vertical), "12");
        // 默认跳过 I/O/Z：第 12 个字母轴号 = M（I 被跳过）
        assert_eq!(mixed.grid_label(12, GridDirection::Horizontal), "M");
        // Numeric 恒数字，忽略方向
        let numeric = DatumConvention {
            grid_numbering: GridNumbering::Numeric,
            ..DatumConvention::default()
        };
        assert_eq!(numeric.grid_label(3, GridDirection::Horizontal), "3");
        assert_eq!(numeric.grid_label(3, GridDirection::Vertical), "3");
    }

    #[test]
    fn convention_defaults_to_skipping_ambiguous_letters() {
        let c = DatumConvention::default();
        assert!(
            c.skip_ambiguous_grid_letters,
            "默认应跳过易混字母 I/O/Z（符合国标）"
        );
        assert_eq!(c.grid_label(9, GridDirection::Horizontal), "J");
    }

    #[test]
    fn datum_convention_json_without_the_new_field_still_loads() {
        // 旧 JSON 缺 `skip_ambiguous_grid_letters` 键：靠 #[serde(default)] 兜底，
        // 反序列化必须成功且默认跳过（去掉 default 即解析报错 → 红）。
        let old = r#"{
            "grid_numbering": "Alphabetic",
            "level_decimals": 3,
            "level_prefix": "±",
            "show_grid_in_three_d": false
        }"#;
        let c: DatumConvention = serde_json::from_str(old).unwrap();
        assert!(c.skip_ambiguous_grid_letters, "缺省键时应按国标默认跳过");
        assert_eq!(c.grid_label(9, GridDirection::Horizontal), "J");
    }

    #[test]
    fn alphabetic_labels_are_bijective_over_a_range() {
        // 双射性：1..=200 的字母轴号在两个字母表状态下都**互不重复**（否则会撞号）。
        for skip in [false, true] {
            let c = DatumConvention {
                grid_numbering: GridNumbering::Alphabetic,
                skip_ambiguous_grid_letters: skip,
                ..DatumConvention::default()
            };
            let labels: std::collections::BTreeSet<String> = (1..=200)
                .map(|i| c.grid_label(i, GridDirection::Horizontal))
                .collect();
            assert_eq!(
                labels.len(),
                200,
                "skip={skip} 时 1..=200 的字母轴号出现重复"
            );

            // index == 0 → 空串（无 0 号轴）
            assert_eq!(c.grid_label(0, GridDirection::Horizontal), "");
        }

        // K=23（跳过 I/O/Z）时的进位锚点：23→Y、24→AA、46→AY。
        let k23 = DatumConvention {
            grid_numbering: GridNumbering::Alphabetic,
            skip_ambiguous_grid_letters: true,
            ..DatumConvention::default()
        };
        assert_eq!(k23.grid_label(23, GridDirection::Horizontal), "Y");
        assert_eq!(k23.grid_label(24, GridDirection::Horizontal), "AA");
        assert_eq!(k23.grid_label(46, GridDirection::Horizontal), "AY");
    }

    #[test]
    fn mixed_direction_labels_differ_for_the_same_index() {
        // 同一 index 下，Vertical 出数字、Horizontal 出字母，二者必然不相等。
        let mixed = DatumConvention::default();
        for index in 1..=26 {
            let v = mixed.grid_label(index, GridDirection::Vertical);
            let h = mixed.grid_label(index, GridDirection::Horizontal);
            assert_ne!(
                v, h,
                "Mixed 下同一 index={index} 的方向标签不应相同（Vertical={v}, Horizontal={h}）"
            );
        }
    }
}
