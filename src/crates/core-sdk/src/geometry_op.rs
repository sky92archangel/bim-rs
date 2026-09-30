//! 几何算子（设计文档 9.2 / 3.5）—— **Recipe，不是内核调用**。
//!
//! ## 为什么是"配方"而不是"直接调内核"
//!
//! 模块**只允许依赖 `core-sdk`**（铁律 3），因此它**看不到 `core-kernel`**。
//! 就算看得到也不该直接调，因为那样会丢掉三样东西：
//!
//! | 直接调内核 | 用 Recipe |
//! |---|---|
//! | 无法撤销（不知道做了什么）| 步骤可序列化 → 进撤销栈、进日志 |
//! | 无法审计（几何来路不明）| 每步都有名字与参数，可回放 |
//! | 无法换内核（G3 目标）| 配方与内核无关，换后端不改模块 |
//! | 无法做增量更新 | 步骤带 `inputs` 引用，能建几何历史 DAG |
//!
//! ## 谁执行 Recipe
//!
//! `core-app` 的几何服务：把 [`BuildStep::op`] 映射到 `core-kernel` 的门面方法，
//! 按 [`BuildStep::inputs`] 把前一步的结果喂给下一步，最后把结果
//! 落成 `GeometryRecord`（BREP 文本）并回填 `Element::geometry_ref`。
//!
//! ```text
//! 模块                    core-app                     core-kernel
//! GeometryOp::apply() ──▶ 执行 BuildStep 序列 ──▶ make_box / fuse / cut …
//!   返回 OpOutput          （含 ShapeId ↔ ShapeHandle 映射）
//!
//! ## M7a：线框 / 面操作数
//!
//! M7a 起配方世界可产 **wire / face**（`MakeLineEdge` / `MakeArcEdge` /
//! `MakeWire` / `MakeFace` / `ExtractWire`），`Loft` / `Sweep` / `Pipe` /
//! `Thicken` 的操作数从此齐备——此前"从零造一个截面 wire 再喂给放样/扫掠"
//! 在 Recipe 世界造不出来。

use std::collections::BTreeMap;

use core_model::{PropertyMap, Value};
use serde::{Deserialize, Serialize};

use crate::error::{SdkError, SdkResult};

/// 一条可供执行的构造指令。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BuildOp {
    /// 长方体
    MakeBox,
    /// 圆柱
    MakeCylinder,
    /// 球
    MakeSphere,
    /// 圆锥（含圆台；`r2 = 0` 即尖锥）
    MakeCone,
    /// 圆环
    MakeTorus,
    /// 拉伸（轮廓 → 体）
    Extrude,
    /// 旋转（轮廓 → 体）
    Revolve,
    /// 布尔并
    Fuse,
    /// 布尔差
    Cut,
    /// 布尔交
    Common,
    /// 平移
    Translate,
    /// 旋转（刚体）
    Rotate,
    /// 缝合（成壳）—— 把面组缝成**壳**，**不成实体**
    Sew,
    /// 缝合并实体化 —— 缝合后再合成一个**实体**
    SewAndSolidify,
    /// 修复形状
    FixShape,
    /// 同域合并
    UnifySameDomain,
    /// 圆角：在实体选中的边上倒圆（M5-B6.2 八算子之一）
    Fillet,
    /// 倒角：在实体选中的边上切角（M5-B6.2 八算子之一）
    Chamfer,
    /// 抽壳：把实体按厚度挖空、在选中面上开口（M5-B6.2 八算子之一）
    Shell,
    /// 偏置：把形状按距离外扩 / 内缩（M5-B6.2 八算子之一）
    Offset,
    /// 加厚：把面 / 壳按厚度加厚成实体（M5-B6.2 八算子之一）
    Thicken,
    /// 放样：由**两个及以上**截面 wire 放样成体 / 面（**变长算子**；M5-B6.2 八算子之一）
    Loft,
    /// 扫掠：把 profile 沿 spine 扫掠（M5-B6.2 八算子之一）
    Sweep,
    /// 管道：把 profile 沿 spine 扫成管（M5-B6.2 八算子之一）
    Pipe,
    /// 直边：两端点定边
    MakeLineEdge,
    /// 三点弧：起点 / 中间点 / 终点（vendor make_arc_edge 为三点式，无角度参数）
    MakeArcEdge,
    /// 线框：把一组边按序连成 wire（变长）
    MakeWire,
    /// 由 wire 造面
    MakeFace,
    /// 提取面 / 实体的外环 wire
    ExtractWire,
}

/// `Sew` / `SewAndSolidify` 的**缺省缝合容差**（mm，键名固定 `tolerance`）。
///
/// **与 `core_kernel::tolerance::POINT_COINCIDENT`（= `1e-7` mm）对齐**。
///
/// 之所以在 `core-sdk` 里另立常量而不是直接 `use core_kernel::tolerance`：
/// **`core-sdk` 不能依赖 `core-kernel`（铁律③：依赖方向单向向上，`core-sdk` 在
/// 内核层之上但模块侧只允许依赖 `core-sdk`）**。缺省值属于"配方词汇"的一部分
/// （与 `BuildOp` / `args` 键名同处一层），放这里最自然：`core-app` 执行配方时
/// 直接取它，模块侧若要声明缺省也看得到。
pub const DEFAULT_SEW_TOLERANCE_MM: f64 = 1e-7;

impl BuildOp {
    /// 该算子需要几个输入形状。
    ///
    /// 用途：`core-app` 在执行前做**前置校验**，避免跑到一半才发现参数不够
    /// （半途失败的事务回滚代价更高）。
    ///
    /// **语义（M5-B6.2 / M7a 扩展）**：
    /// - 对**非变长**算子：返回**精确**元数（沿用既有语义，16 个既有变体逐字不变）；
    /// - 对**变长**算子（[`BuildOp::Loft`] 截面 N ≥ 2、[`BuildOp::MakeWire`]
    ///   边 N ≥ 1）：返回**最小**元数。
    ///
    /// 变长判定与校验请统一走 [`BuildOp::is_variadic`] / [`BuildOp::arity_ok`]，
    /// **不要**再各处手写 `inputs.len() == arity()`（那会把变长算子误杀）。
    #[must_use]
    pub const fn arity(self) -> usize {
        match self {
            Self::MakeBox
            | Self::MakeCylinder
            | Self::MakeSphere
            | Self::MakeCone
            | Self::MakeTorus => 0,
            Self::Extrude | Self::Revolve | Self::Translate | Self::Rotate => 1,
            Self::Sew | Self::SewAndSolidify | Self::FixShape | Self::UnifySameDomain => 1,
            Self::Fuse | Self::Cut | Self::Common => 2,
            Self::Fillet | Self::Chamfer | Self::Shell | Self::Offset | Self::Thicken => 1,
            Self::Loft | Self::Sweep | Self::Pipe => 2,
            Self::MakeLineEdge | Self::MakeArcEdge => 0,
            Self::MakeWire => 1,
            Self::MakeFace | Self::ExtractWire => 1,
        }
    }

    /// 该算子是否**变长**（输入个数只有下限、无上限）。
    ///
    /// M7a 起为 [`BuildOp::Loft`]（截面 wire 数量 N ≥ 2）与 [`BuildOp::MakeWire`]
    /// （边数量 N ≥ 1）；其余一律**精确元数**。
    #[must_use]
    pub const fn is_variadic(self) -> bool {
        matches!(self, Self::Loft | Self::MakeWire)
    }

    /// 输入个数 `n` 对该算子是否**合法**（**单一事实源**）。
    ///
    /// - 变长算子：[`arity`](BuildOp::arity) 为**下限**，`n >= arity()` 即合法；
    /// - 非变长算子：`n == arity()` 才合法。
    ///
    /// [`validate_arity`](BuildStep::validate_arity) 与
    /// [`OpOutput::is_well_ordered`] 均改调本方法，避免两处口径漂移。
    #[must_use]
    pub const fn arity_ok(self, n: usize) -> bool {
        if self.is_variadic() {
            n >= self.arity()
        } else {
            n == self.arity()
        }
    }

    /// 中文名。
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::MakeBox => "长方体",
            Self::MakeCylinder => "圆柱",
            Self::MakeSphere => "球",
            Self::MakeCone => "圆锥",
            Self::MakeTorus => "圆环",
            Self::Extrude => "拉伸",
            Self::Revolve => "旋转",
            Self::Fuse => "布尔并",
            Self::Cut => "布尔差",
            Self::Common => "布尔交",
            Self::Translate => "平移",
            Self::Rotate => "旋转体",
            Self::Sew => "缝合（成壳）",
            Self::SewAndSolidify => "缝合并实体化",
            Self::FixShape => "修复形状",
            Self::UnifySameDomain => "同域合并",
            Self::Fillet => "圆角",
            Self::Chamfer => "倒角",
            Self::Shell => "抽壳",
            Self::Offset => "偏置",
            Self::Thicken => "加厚",
            Self::Loft => "放样",
            Self::Sweep => "扫掠",
            Self::Pipe => "管道",
            Self::MakeLineEdge => "直边",
            Self::MakeArcEdge => "三点弧",
            Self::MakeWire => "线框",
            Self::MakeFace => "由线框造面",
            Self::ExtractWire => "提取外环",
        }
    }
}

/// 配方里的一步。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BuildStep {
    /// 做什么
    pub op: BuildOp,
    /// 参数（长度单位一律 mm，角度一律弧度——与全局约定一致）
    pub args: BTreeMap<String, Value>,
    /// 依赖前面哪几步的结果（0 基下标，指 [`OpOutput::steps`] 里的位置）
    pub inputs: Vec<usize>,
    /// 说明（进日志、进历史面板）
    pub note: String,
}

impl BuildStep {
    /// 构造一步（无参数、无输入）。
    #[must_use]
    pub fn new(op: BuildOp) -> Self {
        Self {
            op,
            args: BTreeMap::new(),
            inputs: Vec::new(),
            note: String::new(),
        }
    }

    /// 指定它吃前面哪几步的结果。
    #[must_use]
    pub fn from_steps(mut self, inputs: Vec<usize>) -> Self {
        self.inputs = inputs;
        self
    }

    /// 说明（链式）。
    #[must_use]
    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.note = note.into();
        self
    }

    /// 加一个浮点参数（链式）。
    #[must_use]
    pub fn with_f64(mut self, key: impl Into<String>, value: f64) -> Self {
        self.args.insert(key.into(), Value::Float(value));
        self
    }

    /// 加一个整数参数（链式）。
    #[must_use]
    pub fn with_i64(mut self, key: impl Into<String>, value: i64) -> Self {
        self.args.insert(key.into(), Value::Int(value));
        self
    }

    /// 加一个文本参数（链式）。
    #[must_use]
    pub fn with_text(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.args.insert(key.into(), Value::Text(value.into()));
        self
    }

    /// 加一个布尔参数（链式）。
    #[must_use]
    pub fn with_bool(mut self, key: impl Into<String>, value: bool) -> Self {
        self.args.insert(key.into(), Value::Bool(value));
        self
    }

    /// 取浮点参数。
    #[must_use]
    pub fn arg_f64(&self, key: &str) -> Option<f64> {
        match self.args.get(key) {
            Some(Value::Float(v)) => Some(*v),
            _ => None,
        }
    }

    /// 取整数参数。
    #[must_use]
    pub fn arg_i64(&self, key: &str) -> Option<i64> {
        match self.args.get(key) {
            Some(Value::Int(v)) => Some(*v),
            _ => None,
        }
    }

    /// 取文本参数。
    #[must_use]
    pub fn arg_text(&self, key: &str) -> Option<&str> {
        match self.args.get(key) {
            Some(Value::Text(v)) => Some(v.as_str()),
            _ => None,
        }
    }

    /// 取布尔参数。
    #[must_use]
    pub fn arg_bool(&self, key: &str) -> Option<bool> {
        match self.args.get(key) {
            Some(Value::Bool(v)) => Some(*v),
            _ => None,
        }
    }

    /// 自检：输入个数对不对。
    ///
    /// 走 [`BuildOp::arity_ok`]（单一事实源）：非变长算子精确匹配，变长算子
    /// （放样 / 线框）只要求 ≥ 最小元数。对既有 16 变体与旧的 `== arity()` **逐位等价**。
    pub fn validate_arity(&self) -> SdkResult<()> {
        if !self.op.arity_ok(self.inputs.len()) {
            return Err(SdkError::GeometryOp(format!(
                "{} 需要 {} 个输入形状，实得 {}",
                self.op.label(),
                self.op.arity(),
                self.inputs.len()
            )));
        }
        Ok(())
    }
}

/// 算子输入。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpInput {
    /// 用户填的参数
    pub params: PropertyMap,
    /// 定位点（mm）
    pub anchor: [f64; 3],
    /// 可作为输入的现有形状描述（`"桥架-1:Face#3"` 这类，供算子挑选）
    pub available_refs: Vec<String>,
}

impl OpInput {
    /// 以参数集构造。
    #[must_use]
    pub fn new(params: PropertyMap) -> Self {
        Self {
            params,
            anchor: [0.0, 0.0, 0.0],
            available_refs: Vec::new(),
        }
    }

    /// 指定定位点（链式）。
    #[must_use]
    pub fn at(mut self, anchor: [f64; 3]) -> Self {
        self.anchor = anchor;
        self
    }

    /// 指定可用输入（链式）。
    #[must_use]
    pub fn with_refs(mut self, refs: Vec<String>) -> Self {
        self.available_refs = refs;
        self
    }

    /// 取浮点参数。
    #[must_use]
    pub fn f64_of(&self, key: &str) -> Option<f64> {
        self.params.get_float(key)
    }

    /// 取浮点参数，缺省时用 `fallback`。
    #[must_use]
    pub fn f64_or(&self, key: &str, fallback: f64) -> f64 {
        self.params.get_float(key).unwrap_or(fallback)
    }
}

/// 算子输出：一串**可执行**的构造步骤。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpOutput {
    /// 步骤序列（按序执行）
    pub steps: Vec<BuildStep>,
    /// 警告（不阻断，但用户要看得到）
    pub warnings: Vec<String>,
}

impl OpOutput {
    /// 由步骤序列构造。
    #[must_use]
    pub fn of(steps: Vec<BuildStep>) -> Self {
        Self {
            steps,
            warnings: Vec::new(),
        }
    }

    /// 空输出（算子判断"这个输入不该建几何"时返回它，而不是报错）。
    #[must_use]
    pub fn empty() -> Self {
        Self {
            steps: Vec::new(),
            warnings: Vec::new(),
        }
    }

    /// 记一条警告（链式）。
    #[must_use]
    pub fn with_warning(mut self, text: impl Into<String>) -> Self {
        self.warnings.push(text.into());
        self
    }

    /// 步骤数。
    #[must_use]
    pub fn step_count(&self) -> usize {
        self.steps.len()
    }

    /// 是否没有任何步骤。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// 最终结果对应第几步（`None` 表示空配方）。
    #[must_use]
    pub fn final_step(&self) -> Option<usize> {
        self.steps.len().checked_sub(1)
    }

    /// 全部步骤的**依赖下标**是否都指向更早的步骤。
    ///
    /// 这是配方可执行的必要条件（有向无环）；不满足就不要送去执行。
    ///
    /// 元数校验走 [`BuildOp::arity_ok`]（与 [`BuildStep::validate_arity`] 同一口径，
    /// 支持变长算子；对既有 16 变体与旧 `== arity()` 逐位等价）。
    #[must_use]
    pub fn is_well_ordered(&self) -> bool {
        self.steps.iter().enumerate().all(|(i, step)| {
            step.op.arity_ok(step.inputs.len()) && step.inputs.iter().all(|&idx| idx < i)
        })
    }
}

/// 几何算子。
pub trait GeometryOp: Send + Sync {
    /// 算子 id（全局唯一，建议带模块前缀）。
    fn id(&self) -> &str;

    /// 界面显示名。
    fn display_name(&self) -> &str;

    /// 生成配方。
    fn apply(&self, input: &OpInput) -> SdkResult<OpOutput>;

    /// 说明（界面提示"这个算子做什么"）。
    fn description(&self) -> &str {
        ""
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一个真实的示例：由"宽 × 高 × 长"造一段矩形桥架。
    struct RectTrayOp;

    impl GeometryOp for RectTrayOp {
        fn id(&self) -> &'static str {
            "ceec.tray.rect"
        }

        fn display_name(&self) -> &'static str {
            "矩形电缆桥架"
        }

        fn description(&self) -> &'static str {
            "宽×高×长 的矩形体，按定位点放置"
        }

        fn apply(&self, input: &OpInput) -> SdkResult<OpOutput> {
            let width = input.f64_or("width", 600.0);
            let height = input.f64_or("height", 150.0);
            let length = input.f64_or("length", 6000.0);
            if width <= 0.0 || height <= 0.0 || length <= 0.0 {
                return Err(SdkError::BadParameter(
                    "桥架尺寸必须为正数（单位 mm）".to_string(),
                ));
            }

            let [x, y, z] = input.anchor;
            let body = BuildStep::new(BuildOp::MakeBox)
                .with_f64("dx", width)
                .with_f64("dy", height)
                .with_f64("dz", length)
                .with_note("桥架毛坯");
            let placed = BuildStep::new(BuildOp::Translate)
                .from_steps(vec![0])
                .with_f64("x", x)
                .with_f64("y", y)
                .with_f64("z", z)
                .with_note("按定位点放置");

            let mut out = OpOutput::of(vec![body, placed]);
            if width > 1200.0 {
                out = out.with_warning("桥架宽度超过 1200mm，请确认是否需要双层桥架");
            }
            Ok(out)
        }
    }

    #[test]
    fn build_op_arity_matches_the_operation() {
        assert_eq!(BuildOp::MakeBox.arity(), 0);
        assert_eq!(BuildOp::Extrude.arity(), 1);
        assert_eq!(BuildOp::Fuse.arity(), 2);
        assert_eq!(BuildOp::Cut.arity(), 2);
    }

    /// M3 新增：锥 / 环都是**无输入**的原始体，中文名照设计文档。
    #[test]
    fn m3_cone_and_torus_are_nullary_primitives() {
        assert_eq!(BuildOp::MakeCone.arity(), 0);
        assert_eq!(BuildOp::MakeTorus.arity(), 0);
        assert_eq!(BuildOp::MakeCone.label(), "圆锥");
        assert_eq!(BuildOp::MakeTorus.label(), "圆环");
    }

    /// **M4 修复类算子钉子（mock 腿）**：`Sew` / `SewAndSolidify` / `FixShape` /
    /// `UnifySameDomain` 四者都是一元算子，`validate_arity()` 对 1 个输入通过、
    /// 对 0 / 2 个输入报错，中文名与能力表**逐字一致**。
    #[test]
    fn sew_fix_unify_are_unary_and_labeled() {
        let ops = [
            BuildOp::Sew,
            BuildOp::SewAndSolidify,
            BuildOp::FixShape,
            BuildOp::UnifySameDomain,
        ];
        for op in ops {
            assert_eq!(op.arity(), 1, "{op:?} 应为一元算子");
        }

        // validate_arity：1 个输入通过；0 / 2 个输入报错
        assert!(
            BuildStep::new(BuildOp::Sew)
                .from_steps(vec![0])
                .validate_arity()
                .is_ok(),
            "1 个输入应通过"
        );
        assert!(
            BuildStep::new(BuildOp::Sew).validate_arity().is_err(),
            "0 个输入应报错"
        );
        assert!(
            BuildStep::new(BuildOp::Sew)
                .from_steps(vec![0, 1])
                .validate_arity()
                .is_err(),
            "2 个输入应报错"
        );
        // 其余三者同样一元
        for op in [
            BuildOp::SewAndSolidify,
            BuildOp::FixShape,
            BuildOp::UnifySameDomain,
        ] {
            assert!(BuildStep::new(op).validate_arity().is_err());
            assert!(
                BuildStep::new(op)
                    .from_steps(vec![0])
                    .validate_arity()
                    .is_ok()
            );
        }

        // label 与需求表逐字一致（含全角括号）
        assert_eq!(BuildOp::Sew.label(), "缝合（成壳）");
        assert_eq!(BuildOp::SewAndSolidify.label(), "缝合并实体化");
        assert_eq!(BuildOp::FixShape.label(), "修复形状");
        assert_eq!(BuildOp::UnifySameDomain.label(), "同域合并");
    }

    /// 缺省缝合容差与 `core_kernel::tolerance::POINT_COINCIDENT` 对齐（= `1e-7` mm）。
    #[test]
    fn default_sew_tolerance_matches_the_kernel_tolerance_alignment() {
        assert_eq!(DEFAULT_SEW_TOLERANCE_MM, 1e-7);
    }

    #[test]
    fn build_step_args_roundtrip_by_type() {
        let s = BuildStep::new(BuildOp::MakeBox)
            .with_f64("dx", 600.0)
            .with_i64("segments", 8)
            .with_text("material", "钢")
            .with_bool("hollow", true);

        assert_eq!(s.arg_f64("dx"), Some(600.0));
        assert_eq!(s.arg_i64("segments"), Some(8));
        assert_eq!(s.arg_text("material"), Some("钢"));
        assert_eq!(s.arg_bool("hollow"), Some(true));
        assert_eq!(s.arg_f64("segments"), None, "类型不符应返回 None");
    }

    #[test]
    fn rect_tray_op_produces_a_two_step_recipe() {
        let input = OpInput::new(
            PropertyMap::new("POWER_CABLE_TRAY")
                .with("width", Value::Float(600.0))
                .with("length", Value::Float(6000.0)),
        )
        .at([1000.0, 0.0, 3000.0]);

        let out = RectTrayOp.apply(&input).unwrap();
        assert_eq!(out.step_count(), 2);
        assert_eq!(out.steps[0].op, BuildOp::MakeBox);
        assert_eq!(out.steps[1].op, BuildOp::Translate);
        assert_eq!(out.final_step(), Some(1));
        assert!(out.is_well_ordered());
    }

    #[test]
    fn recipe_is_dependency_ordered_and_arity_correct() {
        let out = RectTrayOp
            .apply(&OpInput::new(PropertyMap::new("X")))
            .unwrap();
        assert!(out.is_well_ordered());
        for step in &out.steps {
            assert!(step.validate_arity().is_ok());
        }
    }

    #[test]
    fn bad_dimensions_are_rejected() {
        let input = OpInput::new(PropertyMap::new("X").with("width", Value::Float(0.0)));
        assert!(matches!(
            RectTrayOp.apply(&input),
            Err(SdkError::BadParameter(_))
        ));
    }

    #[test]
    fn wide_trays_produce_a_warning_but_still_build() {
        let input = OpInput::new(PropertyMap::new("X").with("width", Value::Float(1500.0)));
        let out = RectTrayOp.apply(&input).unwrap();
        assert_eq!(out.step_count(), 2);
        assert_eq!(out.warnings.len(), 1);
    }

    #[test]
    fn defaults_are_used_when_parameters_are_missing() {
        let out = RectTrayOp
            .apply(&OpInput::new(PropertyMap::new("X")))
            .unwrap();
        assert_eq!(out.steps[0].arg_f64("dx"), Some(600.0));
        assert_eq!(out.steps[0].arg_f64("dz"), Some(6000.0));
    }

    #[test]
    fn arity_validation_catches_a_step_missing_its_input() {
        let bad = BuildStep::new(BuildOp::Fuse).with_note("忘了接输入");
        assert!(matches!(bad.validate_arity(), Err(SdkError::GeometryOp(_))));
    }

    #[test]
    fn well_ordered_detects_forward_references() {
        let bad = OpOutput::of(vec![BuildStep::new(BuildOp::Translate).from_steps(vec![1])]);
        assert!(!bad.is_well_ordered(), "指向未来的下标不成环也是错的");
    }

    #[test]
    fn empty_output_is_empty_and_selectable() {
        let out = OpOutput::empty();
        assert!(out.is_empty());
        assert_eq!(out.final_step(), None);
        assert!(out.is_well_ordered(), "空配方平凡有序");
    }

    #[test]
    fn op_input_helpers_work() {
        let input = OpInput::new(PropertyMap::new("X").with("width", Value::Float(500.0)))
            .at([1.0, 2.0, 3.0])
            .with_refs(vec!["a".to_string()]);
        assert_eq!(input.f64_of("width"), Some(500.0));
        assert_eq!(input.f64_or("missing", 42.0), 42.0);
        assert_eq!(input.anchor, [1.0, 2.0, 3.0]);
        assert_eq!(input.available_refs.len(), 1);
    }

    #[test]
    fn recipe_roundtrips_through_json_for_the_undo_stack() {
        let out = RectTrayOp
            .apply(&OpInput::new(PropertyMap::new("X")))
            .unwrap();
        let json = serde_json::to_string(&out).unwrap();
        assert_eq!(serde_json::from_str::<OpOutput>(&json).unwrap(), out);
    }

    #[test]
    fn op_labels_are_available_for_ui() {
        assert_eq!(BuildOp::Fuse.label(), "布尔并");
        assert_eq!(BuildOp::MakeCylinder.label(), "圆柱");
    }

    /// **B6.2a 钉子 9**：8 个新算子的 `arity()` / `label()` 逐字一致；`Loft` 当时是
    /// 唯一变长算子（`arity() == 2` 为**最小值**，`arity_ok(2)/(3)` 真、`arity_ok(1)` 假；
    /// M7a 起 `MakeWire` 亦变长，见下方 M7a 钉子）；其余 7 个为精确元数。
    #[test]
    fn new_ops_arity_labels_and_variadic() {
        // (算子, 元数, 中文名, 是否变长)
        let cases = [
            (BuildOp::Fillet, 1usize, "圆角", false),
            (BuildOp::Chamfer, 1, "倒角", false),
            (BuildOp::Shell, 1, "抽壳", false),
            (BuildOp::Offset, 1, "偏置", false),
            (BuildOp::Thicken, 1, "加厚", false),
            (BuildOp::Loft, 2, "放样", true),
            (BuildOp::Sweep, 2, "扫掠", false),
            (BuildOp::Pipe, 2, "管道", false),
        ];
        for (op, arity, label, variadic) in cases {
            assert_eq!(op.arity(), arity, "{op:?} 元数应为 {arity}");
            assert_eq!(op.label(), label, "{op:?} 中文名应为 {label}");
            assert_eq!(
                op.is_variadic(),
                variadic,
                "{op:?} is_variadic 应为 {variadic}"
            );
        }

        // Loft：变长语义（arity 是最小值）。
        assert!(BuildOp::Loft.is_variadic());
        assert_eq!(BuildOp::Loft.arity(), 2);
        assert!(BuildOp::Loft.arity_ok(2), "Loft 恰好 2 个输入应合法");
        assert!(BuildOp::Loft.arity_ok(3), "Loft 3 个输入应合法（变长）");
        assert!(BuildOp::Loft.arity_ok(10), "Loft N>2 个输入应合法（变长）");
        assert!(
            !BuildOp::Loft.arity_ok(1),
            "Loft 只 1 个输入应非法（<最小值）"
        );
        assert!(!BuildOp::Loft.arity_ok(0), "Loft 0 个输入应非法");

        // 非变长：精确匹配。
        assert!(BuildOp::Fillet.arity_ok(1));
        assert!(!BuildOp::Fillet.arity_ok(0));
        assert!(!BuildOp::Fillet.arity_ok(2));
        assert!(BuildOp::Sweep.arity_ok(2));
        assert!(!BuildOp::Sweep.arity_ok(3));

        // 经 validate_arity 的可观测行为：
        // Loft 2 / 3 个输入通过（变长），1 / 0 个报错；
        assert!(
            BuildStep::new(BuildOp::Loft)
                .from_steps(vec![0, 1])
                .validate_arity()
                .is_ok(),
            "Loft 2 个输入应通过"
        );
        assert!(
            BuildStep::new(BuildOp::Loft)
                .from_steps(vec![0, 1, 2])
                .validate_arity()
                .is_ok(),
            "Loft 3 个输入应通过（变长）"
        );
        assert!(
            BuildStep::new(BuildOp::Loft)
                .from_steps(vec![0])
                .validate_arity()
                .is_err(),
            "Loft 1 个输入应报错"
        );
        // Fillet 精确一元。
        assert!(
            BuildStep::new(BuildOp::Fillet)
                .from_steps(vec![0])
                .validate_arity()
                .is_ok()
        );
        assert!(BuildStep::new(BuildOp::Fillet).validate_arity().is_err());

        // is_well_ordered 也接受变长 step。
        let ok = OpOutput::of(vec![
            BuildStep::new(BuildOp::MakeBox),
            BuildStep::new(BuildOp::Loft).from_steps(vec![0, 0, 0]),
        ]);
        assert!(ok.is_well_ordered(), "变长 step 的有序判定应通过");
    }

    /// **B6.2a 钉子 10（回归红线）**：既有 16 变体的 `arity()` / `label()` /
    /// `validate_arity()` **逐字不变**——防新增 8 变体时误伤既有语义。
    #[test]
    fn existing_sixteen_ops_keep_their_arity_and_labels() {
        let cases = [
            (BuildOp::MakeBox, 0usize, "长方体"),
            (BuildOp::MakeCylinder, 0, "圆柱"),
            (BuildOp::MakeSphere, 0, "球"),
            (BuildOp::MakeCone, 0, "圆锥"),
            (BuildOp::MakeTorus, 0, "圆环"),
            (BuildOp::Extrude, 1, "拉伸"),
            (BuildOp::Revolve, 1, "旋转"),
            (BuildOp::Fuse, 2, "布尔并"),
            (BuildOp::Cut, 2, "布尔差"),
            (BuildOp::Common, 2, "布尔交"),
            (BuildOp::Translate, 1, "平移"),
            (BuildOp::Rotate, 1, "旋转体"),
            (BuildOp::Sew, 1, "缝合（成壳）"),
            (BuildOp::SewAndSolidify, 1, "缝合并实体化"),
            (BuildOp::FixShape, 1, "修复形状"),
            (BuildOp::UnifySameDomain, 1, "同域合并"),
        ];
        assert_eq!(cases.len(), 16, "既有算子恰为 16 个");
        for (op, arity, label) in cases {
            assert_eq!(op.arity(), arity, "{op:?} 元数应保持 {arity}");
            assert_eq!(op.label(), label, "{op:?} 中文名应保持 {label}");
            assert!(!op.is_variadic(), "{op:?} 既有算子不得变长");
            assert!(op.arity_ok(arity), "{op:?} 应接受 {arity} 个输入");
            assert!(!op.arity_ok(arity + 1), "{op:?} 不得多收输入");
        }

        // validate_arity 既有行为：Fuse 缺输入报错、齐备通过。
        assert!(BuildStep::new(BuildOp::Fuse).validate_arity().is_err());
        assert!(
            BuildStep::new(BuildOp::Fuse)
                .from_steps(vec![0, 1])
                .validate_arity()
                .is_ok()
        );
    }

    /// **M7a 钉子 1**：5 个线框/面算子的 `arity()` / `label()` / `is_variadic()`
    /// 逐字一致；`MakeWire` 为变长（`arity() == 1` 为**最小值**，`arity_ok(1)/(3)`
    /// 真、`arity_ok(0)` 假）；其余 4 个为精确元数。
    #[test]
    fn m7a_wire_face_ops_arity_labels_and_variadic() {
        // (算子, 元数, 中文名, 是否变长)
        let cases = [
            (BuildOp::MakeLineEdge, 0usize, "直边", false),
            (BuildOp::MakeArcEdge, 0, "三点弧", false),
            (BuildOp::MakeWire, 1, "线框", true),
            (BuildOp::MakeFace, 1, "由线框造面", false),
            (BuildOp::ExtractWire, 1, "提取外环", false),
        ];
        for (op, arity, label, variadic) in cases {
            assert_eq!(op.arity(), arity, "{op:?} 元数应为 {arity}");
            assert_eq!(op.label(), label, "{op:?} 中文名应为 {label}");
            assert_eq!(
                op.is_variadic(),
                variadic,
                "{op:?} is_variadic 应为 {variadic}"
            );
        }

        // MakeWire：变长语义（arity 是最小值）。
        assert!(BuildOp::MakeWire.is_variadic());
        assert_eq!(BuildOp::MakeWire.arity(), 1);
        assert!(BuildOp::MakeWire.arity_ok(1), "MakeWire 恰好 1 条边应合法");
        assert!(
            BuildOp::MakeWire.arity_ok(3),
            "MakeWire 3 条边应合法（变长）"
        );
        assert!(
            BuildOp::MakeWire.arity_ok(10),
            "MakeWire N>1 条边应合法（变长）"
        );
        assert!(
            !BuildOp::MakeWire.arity_ok(0),
            "MakeWire 0 条边应非法（<最小值）"
        );

        // 非变长：精确匹配。
        assert!(BuildOp::MakeLineEdge.arity_ok(0));
        assert!(!BuildOp::MakeLineEdge.arity_ok(1));
        assert!(BuildOp::MakeFace.arity_ok(1));
        assert!(!BuildOp::MakeFace.arity_ok(2));
        assert!(BuildOp::ExtractWire.arity_ok(1));
        assert!(!BuildOp::ExtractWire.arity_ok(0));
    }

    /// **M7a 钉子 2**：MakeWire 0 输入被配方前置门（`validate_arity` /
    /// `is_well_ordered`）拒绝——变长下限 1 的可观测行为；1 条边输入齐备通过。
    #[test]
    fn m7a_make_wire_zero_inputs_is_refused_by_recipe_gates() {
        // 0 输入：validate_arity 报错、is_well_ordered 拒绝。
        let zero = BuildStep::new(BuildOp::MakeWire).with_note("忘了接边");
        assert!(
            matches!(zero.validate_arity(), Err(SdkError::GeometryOp(_))),
            "MakeWire 0 输入应被 validate_arity 拒绝"
        );
        let bad = OpOutput::of(vec![BuildStep::new(BuildOp::MakeWire)]);
        assert!(!bad.is_well_ordered(), "MakeWire 0 输入不应通过有序判定");

        // 1 输入（下限）：validate_arity 通过、is_well_ordered 接受。
        let one = BuildStep::new(BuildOp::MakeWire).from_steps(vec![0]);
        assert!(one.validate_arity().is_ok(), "MakeWire 1 条边应通过");
        let ok = OpOutput::of(vec![
            BuildStep::new(BuildOp::MakeLineEdge),
            BuildStep::new(BuildOp::MakeWire).from_steps(vec![0]),
        ]);
        assert!(ok.is_well_ordered(), "MakeWire 变长 step 的有序判定应通过");
    }

    /// **M7a 钉子 3（清点钉）**：`BuildOp` 恰为 29 个变体（M6 的 24 + M7a 的 5）；
    /// exhaustive match 保证清单与枚举同步——M7b+ 新增变体时同步更新此钉。
    #[test]
    fn m7a_build_op_variant_count_is_twenty_nine() {
        let all: [BuildOp; 29] = [
            BuildOp::MakeBox,
            BuildOp::MakeCylinder,
            BuildOp::MakeSphere,
            BuildOp::MakeCone,
            BuildOp::MakeTorus,
            BuildOp::Extrude,
            BuildOp::Revolve,
            BuildOp::Fuse,
            BuildOp::Cut,
            BuildOp::Common,
            BuildOp::Translate,
            BuildOp::Rotate,
            BuildOp::Sew,
            BuildOp::SewAndSolidify,
            BuildOp::FixShape,
            BuildOp::UnifySameDomain,
            BuildOp::Fillet,
            BuildOp::Chamfer,
            BuildOp::Shell,
            BuildOp::Offset,
            BuildOp::Thicken,
            BuildOp::Loft,
            BuildOp::Sweep,
            BuildOp::Pipe,
            BuildOp::MakeLineEdge,
            BuildOp::MakeArcEdge,
            BuildOp::MakeWire,
            BuildOp::MakeFace,
            BuildOp::ExtractWire,
        ];
        // exhaustive match：枚举与清单不同步（漏列/多列）都会编译失败。
        for op in all {
            match op {
                BuildOp::MakeBox
                | BuildOp::MakeCylinder
                | BuildOp::MakeSphere
                | BuildOp::MakeCone
                | BuildOp::MakeTorus
                | BuildOp::Extrude
                | BuildOp::Revolve
                | BuildOp::Fuse
                | BuildOp::Cut
                | BuildOp::Common
                | BuildOp::Translate
                | BuildOp::Rotate
                | BuildOp::Sew
                | BuildOp::SewAndSolidify
                | BuildOp::FixShape
                | BuildOp::UnifySameDomain
                | BuildOp::Fillet
                | BuildOp::Chamfer
                | BuildOp::Shell
                | BuildOp::Offset
                | BuildOp::Thicken
                | BuildOp::Loft
                | BuildOp::Sweep
                | BuildOp::Pipe
                | BuildOp::MakeLineEdge
                | BuildOp::MakeArcEdge
                | BuildOp::MakeWire
                | BuildOp::MakeFace
                | BuildOp::ExtractWire => {}
            }
        }
    }
}
