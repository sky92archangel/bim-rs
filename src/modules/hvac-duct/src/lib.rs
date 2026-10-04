//! # hvac-duct —— 建筑侧暖通专业模块（M7）
//!
//! 立项方案 §10 M7 行：「电力侧与建筑侧各至少一个可用模块」；§7.3 对策：
//! 「先电力后建筑」。电力侧标杆是 `cable-tunnel`；本 crate 是**建筑侧（暖通）
//! 第一个可用模块**，结构**逐项复刻 `cable-tunnel`**（其复刻自 `example-beam`，
//! 唯一合法模板）。
//!
//! ## 它刻意只依赖 `core-sdk` 一个 crate
//!
//! ```toml
//! [dependencies]
//! core-sdk.workspace = true      # ← 只有这一行（铁律 3）
//! ```
//!
//! 因此它**看不到内核、也看不到图纸层**。执行配方的是 `core-app`。
//!
//! ## 为什么是暖通、为什么是这两个算子
//!
//! - 电力侧两模块已覆盖 `MakeBox` / `MakeCylinder` / `Cut` / `Translate` /
//!   `MakeLineEdge` / `MakeWire` / `Pipe`；
//! - 本模块新增 **`Loft`（变长放样）**——此前没有任何模块用过它。变径管是
//!   暖通风管的标准构件，放样是配方世界唯一能做它的算子（两个截面 wire →
//!   实体）。这是「接口若贴合单一专业，二阶段无法低成本扩展」（立项 P0 风险）
//!   的持续体检：第三个专业模块仍**零改动挂接成功**。
//!
//! ## 它注册了什么
//!
//! | 注册项 | 具体内容 |
//! |---|---|
//! | 品类 | `HVAC_DUCT`（暖通风管） |
//! | 属性模式 | 风管宽 / 风管高 / 延米长度 / 材质 |
//! | 元素工厂 | [`DuctFactory`]（只造语义元素，不建几何） |
//! | 几何算子 | [`DuctRectOp`]（矩形风管段：`MakeBox` → `Translate` 配方）、[`DuctTransitionOp`]（变径管：两截面 wire → `Loft` 配方）、[`DuctElbowOp`]（90° 弯头：弧脊线 → `Pipe` 配方，G6-M12） |
//! | 标注规则 | [`DuctWidthRule`]（线性尺寸，0 位小数） |
//! | 专业图层 | `HVAC-风管` / `HVAC-附件` |
//!
//! ## 「可用」尺子（M7 验收口径）
//!
//! - [`DuctRectOp`]：mock 内核即可全链执行（`MakeBox`/`Translate` 是占位内核
//!   支持的算子），apps/desktop 的 `e2e_hvac.rs` 在**默认腿**走五环断言；
//! - [`DuctTransitionOp`]：`MakeLineEdge×4 → MakeWire(起端) → MakeLineEdge×4 →
//!   MakeWire(末端) → Loft`——`Loft` 为真实内核专属（占位后端响亮拒绝），
//!   `e2e_hvac_occt.rs` 在 occt 腿走**真实 execute + 棱台体积精确 oracle**；
//! - [`DuctElbowOp`]：`MakeArcEdge(弧脊线) → Pipe`——`Pipe` 同为真实内核专属
//!   （同 Loft 模式），单测走**配方静态断言**（G6-M12：三点弧坐标 / Pipe
//!   inputs 顺序 / 权威 key 对账）。
//!
//! 两者的配方坐标（截面角点 / 定位点）都在 [`GeometryOp::apply`] 时由参数计算，
//! **不新增任何 args key**（铁律 5：key 权威来源 = 设计文档 §5.1）。

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::panic))]

use core_sdk::{
    AnnotationContext, AnnotationNeed, AnnotationRule, BuildOp, BuildStep, CreateRequest, Element,
    ElementFactory, FieldDef, FieldType, GeometryOp, LayerSpec, Module, ModuleManifest, OpInput,
    OpOutput, ParamDef, ParamDefault, ParamKind, PropertyMap, PropertySchema, Registry, SdkError,
    SdkResult, Value, build_element,
};

/// 模块 id（反向域名风格）。
pub const MODULE_ID: &str = "hvac.duct";

/// 品类名（= 属性模式名，Core 用字符串作键）。
pub const CATEGORY: &str = "HVAC_DUCT";

/// 矩形风管段几何算子 id。
pub const GEOMETRY_OP_ID_RECT: &str = "hvac.duct.rect";

/// 风管变径管几何算子 id。
pub const GEOMETRY_OP_ID_TRANSITION: &str = "hvac.duct.transition";

/// 风管 90° 弯头几何算子 id（G6-M12：第三个算子）。
pub const GEOMETRY_OP_ID_ELBOW: &str = "hvac.duct.elbow";

/// 标注规则 id。
pub const ANNOTATION_RULE_ID: &str = "hvac.duct.width";

/// 默认风管宽（mm）。
pub const DEFAULT_WIDTH_MM: f64 = 500.0;
/// 默认风管高（mm）。
pub const DEFAULT_HEIGHT_MM: f64 = 320.0;
/// 默认延米长度（mm，标准风管节长）。
pub const DEFAULT_LENGTH_MM: f64 = 1200.0;
/// 默认材质。
pub const DEFAULT_MATERIAL: &str = "镀锌钢板";
/// 变径管默认起端宽（mm）。
pub const DEFAULT_START_WIDTH_MM: f64 = 400.0;
/// 变径管默认起端高（mm）。
pub const DEFAULT_START_HEIGHT_MM: f64 = 320.0;
/// 变径管默认末端宽（mm）。
pub const DEFAULT_END_WIDTH_MM: f64 = 800.0;
/// 变径管默认末端高（mm）。
pub const DEFAULT_END_HEIGHT_MM: f64 = 320.0;
/// 弯头默认转弯半径（mm）。
pub const DEFAULT_TURN_RADIUS_MM: f64 = 600.0;

/// 风管宽警告阈值（mm）：超过即提示确认加固与分段（承接 cable-tunnel 的
/// 可观测警告口径：超限**不阻断**，但用户要看得到）。
pub const WIDTH_WARN_LIMIT_MM: f64 = 2500.0;

// =============================================================================
// 属性模式
// =============================================================================

/// 暖通风管的属性模式。
///
/// 注意字段名全是**专业词汇**（风管宽/高/延米/材质）——Core 一个都不认识，
/// 这正是模块存在的意义（设计文档 9.3：Core 永远不为加专业而改）。
#[must_use]
pub fn duct_schema() -> PropertySchema {
    PropertySchema::new(CATEGORY)
        .with_field(
            FieldDef::new("width", FieldType::Float, "风管宽")
                .with_unit("mm")
                .required()
                .with_default(Value::Float(DEFAULT_WIDTH_MM)),
        )
        .with_field(
            FieldDef::new("height", FieldType::Float, "风管高")
                .with_unit("mm")
                .required()
                .with_default(Value::Float(DEFAULT_HEIGHT_MM)),
        )
        .with_field(
            FieldDef::new("length", FieldType::Float, "延米长度")
                .with_unit("mm")
                .required()
                .with_default(Value::Float(DEFAULT_LENGTH_MM)),
        )
        .with_field(
            FieldDef::new(
                "material",
                FieldType::Enum(vec!["镀锌钢板".into(), "不锈钢".into(), "铝板".into()]),
                "材质",
            )
            .with_default(Value::Text(DEFAULT_MATERIAL.to_string())),
        )
}

// =============================================================================
// 元素工厂
// =============================================================================

/// 暖通风管的元素工厂。
///
/// **只造语义元素**：返回的 [`Element`] 没有几何，`geometry_ref` 由
/// [`DuctRectOp`] / [`DuctTransitionOp`] 的配方执行完再回填（设计文档 9.2）。
pub struct DuctFactory;

impl ElementFactory for DuctFactory {
    fn category(&self) -> &str {
        CATEGORY
    }

    fn display_name(&self) -> &str {
        "矩形风管"
    }

    fn schema(&self) -> Option<PropertySchema> {
        Some(duct_schema())
    }

    fn create(&self, request: &CreateRequest) -> SdkResult<Element> {
        let mut e = build_element(self.category(), request, "矩形风管");
        e.properties.set(
            "width",
            Value::Float(request.param_f64("width").unwrap_or(DEFAULT_WIDTH_MM)),
        );
        e.properties.set(
            "height",
            Value::Float(request.param_f64("height").unwrap_or(DEFAULT_HEIGHT_MM)),
        );
        e.properties.set(
            "length",
            Value::Float(request.param_f64("length").unwrap_or(DEFAULT_LENGTH_MM)),
        );
        e.properties.set(
            "material",
            Value::Text(
                request
                    .param_text("material")
                    .unwrap_or(DEFAULT_MATERIAL)
                    .to_string(),
            ),
        );
        Ok(e)
    }

    fn default_params(&self) -> PropertyMap {
        PropertyMap::new(CATEGORY)
            .with("width", Value::Float(DEFAULT_WIDTH_MM))
            .with("height", Value::Float(DEFAULT_HEIGHT_MM))
            .with("length", Value::Float(DEFAULT_LENGTH_MM))
            .with("material", Value::Text(DEFAULT_MATERIAL.to_string()))
    }
}

// =============================================================================
// 几何算子（返回配方，不直接建几何）
// =============================================================================

/// 矩形风管段的几何算子（hvac.duct.rect）。
///
/// 它返回的是**两步行之有效的配方**（`MakeBox` → `Translate`），不是内核调用。
/// 执行配方的是 `core-app`（模块看不到内核）。风管轴沿 Z（与 cable-tunnel 的
/// 隧道段同向：dx=宽、dy=高、dz=延米）。宽度超过 [`WIDTH_WARN_LIMIT_MM`] 时落
/// **配方警告**（不阻断，用户要看得到）。
pub struct DuctRectOp;

impl GeometryOp for DuctRectOp {
    fn id(&self) -> &str {
        GEOMETRY_OP_ID_RECT
    }

    fn display_name(&self) -> &str {
        "矩形风管段"
    }

    fn description(&self) -> &str {
        "由 宽×高×延米 生成长方体风管段，按定位点放置"
    }

    /// 专业参数定义（G6-M13：GUI 配方表单，键与 `apply` 的 `f64_or` 逐字对账）。
    fn param_defs(&self) -> Vec<ParamDef> {
        vec![
            ParamDef::optional(
                "width",
                "风管宽",
                ParamKind::Length,
                ParamDefault::Float(DEFAULT_WIDTH_MM),
            ),
            ParamDef::optional(
                "height",
                "风管高",
                ParamKind::Length,
                ParamDefault::Float(DEFAULT_HEIGHT_MM),
            ),
            ParamDef::optional(
                "length",
                "延米长度",
                ParamKind::Length,
                ParamDefault::Float(DEFAULT_LENGTH_MM),
            ),
        ]
    }

    fn apply(&self, input: &OpInput) -> SdkResult<OpOutput> {
        let width = input.f64_or("width", DEFAULT_WIDTH_MM);
        let height = input.f64_or("height", DEFAULT_HEIGHT_MM);
        let length = input.f64_or("length", DEFAULT_LENGTH_MM);

        if width <= 0.0 || height <= 0.0 || length <= 0.0 {
            return Err(SdkError::BadParameter(
                "风管的宽/高/延米必须为正数（单位 mm）".to_string(),
            ));
        }

        let [x, y, z] = input.anchor;
        let body = BuildStep::new(BuildOp::MakeBox)
            .with_f64("dx", width)
            .with_f64("dy", height)
            .with_f64("dz", length)
            .with_note("风管段毛坯");
        let placed = BuildStep::new(BuildOp::Translate)
            .from_steps(vec![0])
            .with_f64("x", x)
            .with_f64("y", y)
            .with_f64("z", z)
            .with_note("按定位点放置");

        let mut out = OpOutput::of(vec![body, placed]);
        if width > WIDTH_WARN_LIMIT_MM {
            out = out.with_warning("风管宽度超过 2500mm，请确认加固方式或采用分仓风道");
        }
        Ok(out)
    }
}

/// 风管变径管的几何算子（hvac.duct.transition）。
///
/// 它返回的是**十一步行之有效的配方**：
///
/// ```text
/// MakeLineEdge×4 → MakeWire(起端截面) → MakeLineEdge×4 → MakeWire(末端截面) → Loft
/// ```
///
/// 起端 / 末端截面都是**矩形 wire**（`MakeLineEdge×4 + MakeWire`），绕定位点
/// **居中**、分别位于 `z = anchor.z` 与 `z = anchor.z + length` 两个水平面；
/// `Loft`（变长放样，`is_solid=true`、`ruled=true`）把两个截面放样成**渐缩实体**
/// ——两个相似矩形（如同为正方形）时是精确棱台，体积有解析 oracle。
///
/// **不新增任何 args key**：`MakeLineEdge` 用 `x1/y1/z1/x2/y2/z2`，`Loft` 用
/// `is_solid` / `ruled`——全部是设计文档 §5.1 权威 key（铁律 5）。
pub struct DuctTransitionOp;

impl GeometryOp for DuctTransitionOp {
    fn id(&self) -> &str {
        GEOMETRY_OP_ID_TRANSITION
    }

    fn display_name(&self) -> &str {
        "风管变径管"
    }

    fn description(&self) -> &str {
        "由 起端/末端矩形截面沿轴线放样成渐缩段（Loft），定位点为起端截面中心"
    }

    /// 专业参数定义（G6-M13：变径四参数 start_width / start_height /
    /// end_width / end_height——schema 未覆盖的专业参数，配方表单是它们的
    /// GUI 入口）。
    fn param_defs(&self) -> Vec<ParamDef> {
        vec![
            ParamDef::optional(
                "start_width",
                "起端宽",
                ParamKind::Length,
                ParamDefault::Float(DEFAULT_START_WIDTH_MM),
            ),
            ParamDef::optional(
                "start_height",
                "起端高",
                ParamKind::Length,
                ParamDefault::Float(DEFAULT_START_HEIGHT_MM),
            ),
            ParamDef::optional(
                "end_width",
                "末端宽",
                ParamKind::Length,
                ParamDefault::Float(DEFAULT_END_WIDTH_MM),
            ),
            ParamDef::optional(
                "end_height",
                "末端高",
                ParamKind::Length,
                ParamDefault::Float(DEFAULT_END_HEIGHT_MM),
            ),
            ParamDef::optional(
                "length",
                "延米长度",
                ParamKind::Length,
                ParamDefault::Float(DEFAULT_LENGTH_MM),
            ),
        ]
    }

    fn apply(&self, input: &OpInput) -> SdkResult<OpOutput> {
        let start_width = input.f64_or("start_width", DEFAULT_START_WIDTH_MM);
        let start_height = input.f64_or("start_height", DEFAULT_START_HEIGHT_MM);
        let end_width = input.f64_or("end_width", DEFAULT_END_WIDTH_MM);
        let end_height = input.f64_or("end_height", DEFAULT_END_HEIGHT_MM);
        let length = input.f64_or("length", DEFAULT_LENGTH_MM);

        if start_width <= 0.0
            || start_height <= 0.0
            || end_width <= 0.0
            || end_height <= 0.0
            || length <= 0.0
        {
            return Err(SdkError::BadParameter(
                "变径管的起端/末端截面与长度必须为正数（单位 mm）".to_string(),
            ));
        }

        let [cx, cy, z0] = input.anchor;
        let z1 = z0 + length;
        let section_edge = |(ax, ay, az): (f64, f64, f64), (bx, by, bz): (f64, f64, f64)| {
            BuildStep::new(BuildOp::MakeLineEdge)
                .with_f64("x1", ax)
                .with_f64("y1", ay)
                .with_f64("z1", az)
                .with_f64("x2", bx)
                .with_f64("y2", by)
                .with_f64("z2", bz)
        };

        // 起端截面（z0 平面，绕定位点居中，逆时针闭合）。
        let (hw1, hh1) = (start_width / 2.0, start_height / 2.0);
        let s0 =
            section_edge((cx - hw1, cy - hh1, z0), (cx + hw1, cy - hh1, z0)).with_note("起端底边");
        let s1 =
            section_edge((cx + hw1, cy - hh1, z0), (cx + hw1, cy + hh1, z0)).with_note("起端右边");
        let s2 =
            section_edge((cx + hw1, cy + hh1, z0), (cx - hw1, cy + hh1, z0)).with_note("起端顶边");
        let s3 = section_edge((cx - hw1, cy + hh1, z0), (cx - hw1, cy - hh1, z0))
            .with_note("起端闭合边");
        let start_wire = BuildStep::new(BuildOp::MakeWire)
            .from_steps(vec![0, 1, 2, 3])
            .with_note("起端截面 wire");

        // 末端截面（z1 平面，同一中心）。
        let (hw2, hh2) = (end_width / 2.0, end_height / 2.0);
        let e0 =
            section_edge((cx - hw2, cy - hh2, z1), (cx + hw2, cy - hh2, z1)).with_note("末端底边");
        let e1 =
            section_edge((cx + hw2, cy - hh2, z1), (cx + hw2, cy + hh2, z1)).with_note("末端右边");
        let e2 =
            section_edge((cx + hw2, cy + hh2, z1), (cx - hw2, cy + hh2, z1)).with_note("末端顶边");
        let e3 = section_edge((cx - hw2, cy + hh2, z1), (cx - hw2, cy - hh2, z1))
            .with_note("末端闭合边");
        let end_wire = BuildStep::new(BuildOp::MakeWire)
            .from_steps(vec![5, 6, 7, 8])
            .with_note("末端截面 wire");

        // Loft 的 inputs 顺序即放样顺序 [起端, 末端]；实体化 + 直纹（相似截面
        // 时为精确棱台）。
        let loft = BuildStep::new(BuildOp::Loft)
            .from_steps(vec![4, 9])
            .with_bool("is_solid", true)
            .with_bool("ruled", true)
            .with_note("放样成变径管实体");

        Ok(OpOutput::of(vec![
            s0, s1, s2, s3, start_wire, e0, e1, e2, e3, end_wire, loft,
        ]))
    }
}

/// 风管 90° 弯头的几何算子（hvac.duct.elbow，G6-M12：第三个算子）。
///
/// 它返回的是**八步行之有效的配方**：
///
/// ```text
/// MakeLineEdge×4 → MakeWire(矩形断面) → MakeArcEdge(弧脊线) → MakeWire → Pipe
/// ```
///
/// 断面是**矩形 wire**（`MakeLineEdge×4 + MakeWire`），在 x=0 平面**绕定位点
/// 居中**（即脊线起端切向 +X 的法平面）；脊线是**三点弧**（`MakeArcEdge`，
/// 起 (0,0,0) / 中 (R(1−1/√2), 0, R·1/√2) / 终 (R, 0, R)）——四分之一圆弧把
/// 风管从 +X 方向转到 +Z 方向；`Pipe` 把断面沿弧脊线扫成弯头实体。
///
/// 转弯半径小于风管宽时落**配方警告**（不阻断，用户要看得到——弯头局部
/// 阻力大，宜加大半径或加导流片）。
///
/// **不新增任何 args key**：`MakeLineEdge` 用 `x1/y1/z1/x2/y2/z2`，
/// `MakeArcEdge` 用 `x1..z3` 九坐标键，`MakeWire` / `Pipe` 无 args——全部是
/// 设计文档 §5.1 权威 key（铁律 5）。
pub struct DuctElbowOp;

impl GeometryOp for DuctElbowOp {
    fn id(&self) -> &str {
        GEOMETRY_OP_ID_ELBOW
    }

    fn display_name(&self) -> &str {
        "风管 90° 弯头"
    }

    fn description(&self) -> &str {
        "由 矩形断面沿四分之一圆弧脊线扫成 90° 弯头（Pipe），定位点为断面中心"
    }

    /// 专业参数定义（G6-M13：含转弯半径专业参数 turn_radius——schema 未
    /// 覆盖的专业参数，配方表单是它的 GUI 入口）。
    fn param_defs(&self) -> Vec<ParamDef> {
        vec![
            ParamDef::optional(
                "width",
                "风管宽",
                ParamKind::Length,
                ParamDefault::Float(DEFAULT_WIDTH_MM),
            ),
            ParamDef::optional(
                "height",
                "风管高",
                ParamKind::Length,
                ParamDefault::Float(DEFAULT_HEIGHT_MM),
            ),
            ParamDef::optional(
                "turn_radius",
                "转弯半径",
                ParamKind::Length,
                ParamDefault::Float(DEFAULT_TURN_RADIUS_MM),
            ),
        ]
    }

    fn apply(&self, input: &OpInput) -> SdkResult<OpOutput> {
        let width = input.f64_or("width", DEFAULT_WIDTH_MM);
        let height = input.f64_or("height", DEFAULT_HEIGHT_MM);
        let turn_radius = input.f64_or("turn_radius", DEFAULT_TURN_RADIUS_MM);

        if width <= 0.0 || height <= 0.0 || turn_radius <= 0.0 {
            return Err(SdkError::BadParameter(
                "弯头的宽/高/转弯半径必须为正数（单位 mm）".to_string(),
            ));
        }

        // 断面（x=0 平面，绕定位点居中，逆时针闭合）。
        let (hw, hh) = (width / 2.0, height / 2.0);
        let section_edge = |(ay, az): (f64, f64), (by, bz): (f64, f64)| {
            BuildStep::new(BuildOp::MakeLineEdge)
                .with_f64("x1", 0.0)
                .with_f64("y1", ay)
                .with_f64("z1", az)
                .with_f64("x2", 0.0)
                .with_f64("y2", by)
                .with_f64("z2", bz)
        };
        let s0 = section_edge((-hw, -hh), (hw, -hh)).with_note("断面底边");
        let s1 = section_edge((hw, -hh), (hw, hh)).with_note("断面右边");
        let s2 = section_edge((hw, hh), (-hw, hh)).with_note("断面顶边");
        let s3 = section_edge((-hw, hh), (-hw, -hh)).with_note("断面闭合边");
        let section_wire = BuildStep::new(BuildOp::MakeWire)
            .from_steps(vec![0, 1, 2, 3])
            .with_note("断面 wire");

        // 弧脊线（三点式四分之一圆弧：起 → 45° 中点 → 终，从 +X 转到 +Z）。
        let r = turn_radius;
        let s45 = std::f64::consts::FRAC_1_SQRT_2;
        let arc = BuildStep::new(BuildOp::MakeArcEdge)
            .with_f64("x1", 0.0)
            .with_f64("y1", 0.0)
            .with_f64("z1", 0.0)
            .with_f64("x2", r * (1.0 - s45))
            .with_f64("y2", 0.0)
            .with_f64("z2", r * s45)
            .with_f64("x3", r)
            .with_f64("y3", 0.0)
            .with_f64("z3", r)
            .with_note("四分之一圆弧脊线");
        let spine_wire = BuildStep::new(BuildOp::MakeWire)
            .from_steps(vec![5])
            .with_note("脊线 wire");

        // Pipe 的 inputs 顺序恒为 [profile, spine]（几何服务 Pipe 派发臂口径）。
        let elbow = BuildStep::new(BuildOp::Pipe)
            .from_steps(vec![4, 6])
            .with_note("扫成 90° 弯头");

        let mut out = OpOutput::of(vec![s0, s1, s2, s3, section_wire, arc, spine_wire, elbow]);
        if turn_radius < width {
            out =
                out.with_warning("弯头转弯半径小于风管宽度，局部阻力较大，建议加大半径或加导流片");
        }
        Ok(out)
    }
}

// =============================================================================
// 标注规则
// =============================================================================

/// 风管宽度标注规则。
///
/// 模块只说"要一个线性尺寸、0 位小数"；**怎么画**（尺寸界线、箭头、文字样式）
/// 由 `core-doc` 依 `DraftingStandard` 决定。
pub struct DuctWidthRule;

impl AnnotationRule for DuctWidthRule {
    fn id(&self) -> &str {
        ANNOTATION_RULE_ID
    }

    fn display_name(&self) -> &str {
        "风管宽度标注"
    }

    fn need(&self) -> AnnotationNeed {
        AnnotationNeed::LinearDimension
    }

    fn context(&self) -> AnnotationContext {
        AnnotationContext::new(AnnotationNeed::LinearDimension)
            .with_decimals(0)
            .with_description("风管的宽度（mm）")
    }
}

// =============================================================================
// 专业图层
// =============================================================================

/// 本模块声明的专业图层（`LayerId` 由 Core 在装载时分配）。
#[must_use]
pub fn duct_layers() -> Vec<LayerSpec> {
    vec![
        LayerSpec::new("HVAC-风管", [0, 160, 80], 0.50).with_description("风管轮廓"),
        LayerSpec::new("HVAC-附件", [255, 140, 0], 0.25)
            .with_line_type("Dashed")
            .with_description("风管附件（阀门/风口示意）"),
    ]
}

// =============================================================================
// 模块本体
// =============================================================================

/// 暖通风管模块（建筑侧首个可用模块）。
///
/// `register` 是**纯粹的注册动作**（设计文档 9.2：不 I/O、不读数、不启线程）。
pub struct HvacDuct;

impl Module for HvacDuct {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest::new(MODULE_ID, "暖通风管模块", env!("CARGO_PKG_VERSION"))
            .with_description("建筑侧暖通专业模块：注册品类 / 属性模式 / 工厂 / 算子 / 标注 / 图层")
            .with_vendor("轻量化 BIM 内核平台")
    }

    fn register(&self, registry: &mut Registry) -> SdkResult<()> {
        registry.register_module(&self.manifest())?;
        registry.register_category(CATEGORY, "暖通风管", MODULE_ID)?;
        registry.register_schema(duct_schema())?;
        registry.register_factory(Box::new(DuctFactory))?;
        registry.register_geometry_op(Box::new(DuctRectOp))?;
        registry.register_geometry_op(Box::new(DuctTransitionOp))?;
        registry.register_geometry_op(Box::new(DuctElbowOp))?;
        registry.register_annotation_rule(Box::new(DuctWidthRule))?;
        for layer in duct_layers() {
            registry.register_layer(layer)?;
        }
        Ok(())
    }

    /// 把刚注册的东西逐项回读一遍。
    ///
    /// 这是"体检"的核心：注册时报了错名字、或某一项根本没进注册表，
    /// 都会在这里暴露，而不是等到用户点"新建风管"才发现。
    fn self_check(&self, registry: &Registry) -> SdkResult<()> {
        let mut missing: Vec<String> = Vec::new();

        if !registry.has_category(CATEGORY) {
            missing.push(format!("品类 {CATEGORY}"));
        }
        if registry.schema(CATEGORY).is_none() {
            missing.push(format!("属性模式 {CATEGORY}"));
        }
        if registry.factory_for(CATEGORY).is_none() {
            missing.push(format!("元素工厂 {CATEGORY}"));
        }
        if registry.geometry_op(GEOMETRY_OP_ID_RECT).is_none() {
            missing.push(format!("几何算子 {GEOMETRY_OP_ID_RECT}"));
        }
        if registry.geometry_op(GEOMETRY_OP_ID_TRANSITION).is_none() {
            missing.push(format!("几何算子 {GEOMETRY_OP_ID_TRANSITION}"));
        }
        if registry.geometry_op(GEOMETRY_OP_ID_ELBOW).is_none() {
            missing.push(format!("几何算子 {GEOMETRY_OP_ID_ELBOW}"));
        }
        if !registry.annotation_rule_ids().contains(&ANNOTATION_RULE_ID) {
            missing.push(format!("标注规则 {ANNOTATION_RULE_ID}"));
        }
        let registered = registry.layers();
        for layer in duct_layers() {
            if !registered.iter().any(|l| l.name == layer.name) {
                missing.push(format!("图层 {}", layer.name));
            }
        }

        if missing.is_empty() {
            Ok(())
        } else {
            Err(SdkError::Factory(format!(
                "暖通风管模块自检失败，注册表缺少：{}",
                missing.join("、")
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_sdk::ModuleId;

    /// 设计文档 §5.1 权威 args key 全集（配方参数 key 的唯一合法来源）。
    const AUTHORITATIVE_KEYS: &[&str] = &[
        "dx",
        "dy",
        "dz",
        "radius",
        "height",
        "r1",
        "r2",
        "major_radius",
        "minor_radius",
        "x",
        "y",
        "z",
        "px",
        "py",
        "pz",
        "angle_rad",
        "thickness",
        "tolerance",
        "distance",
        "edge_selectors",
        "face_selectors",
        "is_solid",
        "ruled",
        "transition_mode",
        "x1",
        "y1",
        "z1",
        "x2",
        "y2",
        "z2",
        "x3",
        "y3",
        "z3",
    ];

    fn installed() -> Registry {
        let mut reg = Registry::new();
        reg.install(&HvacDuct).expect("暖通风管模块应当能装载");
        reg
    }

    #[test]
    fn manifest_is_valid_and_namespaced() {
        let m = HvacDuct.manifest();
        assert_eq!(m.id, ModuleId::new(MODULE_ID));
        assert!(m.id.looks_namespaced(), "MODULE_ID 应为反向域名风格");
        assert!(m.validate().is_ok());
    }

    #[test]
    fn installing_registers_every_bucket() {
        let s = installed().summary();
        assert_eq!(s.modules, 1);
        assert_eq!(s.categories, 1);
        assert_eq!(s.schemas, 1);
        assert_eq!(s.factories, 1);
        assert_eq!(s.geometry_ops, 3, "rect + transition + elbow 三个几何算子");
        assert_eq!(s.annotation_rules, 1);
        assert_eq!(s.layers, 2);
    }

    #[test]
    fn self_check_passes_after_a_clean_install() {
        let reg = installed();
        assert!(HvacDuct.self_check(&reg).is_ok());
    }

    #[test]
    fn self_check_reports_a_missing_category() {
        // 往注册表里"少装一样东西"来验证自检真的会红。
        let mut reg = Registry::new();
        reg.register_module(&HvacDuct.manifest()).unwrap();
        let err = HvacDuct.self_check(&reg).unwrap_err();
        assert!(matches!(err, SdkError::Factory(_)));
    }

    #[test]
    fn factory_produces_a_geometryless_element_with_defaults() {
        let e = DuctFactory.create(&CreateRequest::new("")).unwrap();
        assert_eq!(e.category.as_str(), CATEGORY);
        assert_eq!(e.name, "矩形风管", "空名应回落到显示名");
        assert!(!e.has_geometry(), "工厂不建几何");
        assert_eq!(e.properties.schema, CATEGORY);
        assert_eq!(e.properties.get_float("width"), Some(DEFAULT_WIDTH_MM));
        assert_eq!(e.properties.get_float("height"), Some(DEFAULT_HEIGHT_MM));
        assert_eq!(e.properties.get_float("length"), Some(DEFAULT_LENGTH_MM));
        assert_eq!(
            e.properties.get_text("material"),
            Some(DEFAULT_MATERIAL),
            "材质应回落到默认值"
        );
    }

    #[test]
    fn factory_honours_user_parameters() {
        let e = DuctFactory
            .create(
                &CreateRequest::new("FG-1")
                    .with_param("width", Value::Float(1000.0))
                    .with_param("material", Value::Text("不锈钢".to_string())),
            )
            .unwrap();
        assert_eq!(e.name, "FG-1");
        assert_eq!(e.properties.get_float("width"), Some(1000.0));
        assert_eq!(e.properties.get_text("material"), Some("不锈钢"));
    }

    // ---------- duct.rect：配方有序性 / 参数拒绝 / warnings 落配方 ----------

    #[test]
    fn rect_op_emits_a_well_ordered_recipe() {
        let input = OpInput::new(PropertyMap::new(CATEGORY).with("width", Value::Float(1000.0)))
            .at([1000.0, 0.0, 3000.0]);
        let out = DuctRectOp.apply(&input).unwrap();

        // 2 步：MakeBox → Translate
        assert_eq!(out.step_count(), 2);
        assert_eq!(out.steps[0].op, BuildOp::MakeBox);
        assert_eq!(out.steps[1].op, BuildOp::Translate);

        assert_eq!(out.steps[0].arg_f64("dx"), Some(1000.0));
        assert_eq!(out.steps[0].arg_f64("dy"), Some(DEFAULT_HEIGHT_MM));
        assert_eq!(out.steps[0].arg_f64("dz"), Some(DEFAULT_LENGTH_MM));
        // Translate 吃毛坯
        assert_eq!(out.steps[1].inputs, vec![0]);
        assert_eq!(out.steps[1].arg_f64("x"), Some(1000.0));
        assert_eq!(out.steps[1].arg_f64("y"), Some(0.0));
        assert_eq!(out.steps[1].arg_f64("z"), Some(3000.0));

        assert!(out.is_well_ordered());
        for step in &out.steps {
            assert!(step.validate_arity().is_ok());
        }
    }

    #[test]
    fn rect_op_rejects_non_positive_parameters() {
        for (key, value) in [("width", 0.0), ("height", -320.0), ("length", 0.0)] {
            let input = OpInput::new(PropertyMap::new(CATEGORY).with(key, Value::Float(value)));
            assert!(
                matches!(DuctRectOp.apply(&input), Err(SdkError::BadParameter(_))),
                "参数 {key}={value} 必须被响亮拒绝（BadParameter）"
            );
        }
    }

    #[test]
    fn rect_op_warns_when_width_exceeds_2500mm() {
        // 超宽（3000 > 2500）：配方仍合法可建，但 warnings 落配方。
        let wide = OpInput::new(PropertyMap::new(CATEGORY).with("width", Value::Float(3000.0)));
        let out = DuctRectOp.apply(&wide).unwrap();
        assert_eq!(out.step_count(), 2);
        assert_eq!(out.warnings.len(), 1, "宽度超过 2500mm 必须落 1 条警告");
        assert!(
            out.warnings[0].contains("2500"),
            "警告应提示宽度超过 2500mm"
        );
        assert!(out.is_well_ordered(), "警告不阻断，配方仍应合法");

        // 常规宽（默认 500）：无警告。
        let normal = OpInput::new(PropertyMap::new(CATEGORY));
        let out = DuctRectOp.apply(&normal).unwrap();
        assert!(out.warnings.is_empty(), "常规宽度不应产生警告");
    }

    #[test]
    fn rect_op_recipe_roundtrips_through_json() {
        let input = OpInput::new(
            PropertyMap::new(CATEGORY)
                .with("width", Value::Float(1000.0))
                .with("height", Value::Float(400.0)),
        )
        .at([10.0, 20.0, 30.0]);
        let out = DuctRectOp.apply(&input).unwrap();

        let text = serde_json::to_string(&out).expect("配方应可序列化");
        let back: OpOutput = serde_json::from_str(&text).expect("配方应可反序列化");
        assert_eq!(back, out, "JSON 往返后配方应逐字段相等");
    }

    // ---------- duct.transition：Loft 配方（首个用变长放样的模块）----------

    #[test]
    fn transition_op_emits_an_eleven_step_recipe() {
        let input = OpInput::new(
            PropertyMap::new(CATEGORY)
                .with("start_width", Value::Float(400.0))
                .with("end_width", Value::Float(800.0))
                .with("length", Value::Float(1000.0)),
        )
        .at([1000.0, 0.0, 3000.0]);
        let out = DuctTransitionOp.apply(&input).unwrap();

        // 11 步：MakeLineEdge×4 → MakeWire(起端) → MakeLineEdge×4 → MakeWire(末端) → Loft
        assert_eq!(out.step_count(), 11);
        for i in [0usize, 1, 2, 3, 5, 6, 7, 8] {
            assert_eq!(out.steps[i].op, BuildOp::MakeLineEdge, "第 {i} 步应为直边");
        }
        assert_eq!(out.steps[4].op, BuildOp::MakeWire, "起端截面 wire");
        assert_eq!(out.steps[9].op, BuildOp::MakeWire, "末端截面 wire");
        assert_eq!(out.steps[10].op, BuildOp::Loft);

        // Loft：inputs 顺序即放样顺序 [起端, 末端]；is_solid / ruled 落位。
        assert_eq!(out.steps[10].inputs, vec![4, 9]);
        assert_eq!(out.steps[10].arg_bool("is_solid"), Some(true));
        assert_eq!(out.steps[10].arg_bool("ruled"), Some(true));

        // 「可用」尺子（静态侧）：每步的 inputs 只引用前序步骤、args 只用
        // §5.1 权威 key——即配方每步操作数均可被 Recipe 表达。
        assert!(out.is_well_ordered());
        for (i, step) in out.steps.iter().enumerate() {
            assert!(step.validate_arity().is_ok(), "第 {i} 步元数应合法");
            for key in step.args.keys() {
                assert!(
                    AUTHORITATIVE_KEYS.contains(&key.as_str()),
                    "第 {i} 步参数 key `{key}` 不在设计文档 §5.1 权威集内"
                );
            }
        }
    }

    #[test]
    fn transition_op_rejects_non_positive_parameters() {
        for (key, value) in [
            ("start_width", 0.0),
            ("start_height", -320.0),
            ("end_width", 0.0),
            ("end_height", -320.0),
            ("length", 0.0),
        ] {
            let input = OpInput::new(PropertyMap::new(CATEGORY).with(key, Value::Float(value)));
            assert!(
                matches!(
                    DuctTransitionOp.apply(&input),
                    Err(SdkError::BadParameter(_))
                ),
                "参数 {key}={value} 必须被响亮拒绝（BadParameter）"
            );
        }
    }

    #[test]
    fn transition_op_centers_sections_on_the_anchor() {
        // 截面绕定位点**居中**（不是角点对齐）：起端底边从 (cx-w/2, cy-h/2, z0)
        // 到 (cx+w/2, cy-h/2, z0)；末端截面在 z0+length。
        let input = OpInput::new(
            PropertyMap::new(CATEGORY)
                .with("start_width", Value::Float(400.0))
                .with("start_height", Value::Float(200.0))
                .with("end_width", Value::Float(800.0))
                .with("end_height", Value::Float(400.0))
                .with("length", Value::Float(1000.0)),
        )
        .at([100.0, 200.0, 300.0]);
        let out = DuctTransitionOp.apply(&input).unwrap();

        // 起端底边（step 0）：y = cy - h1/2 = 200 - 100 = 100
        assert_eq!(out.steps[0].arg_f64("x1"), Some(-100.0));
        assert_eq!(out.steps[0].arg_f64("y1"), Some(100.0));
        assert_eq!(out.steps[0].arg_f64("z1"), Some(300.0));
        assert_eq!(out.steps[0].arg_f64("x2"), Some(300.0));
        assert_eq!(out.steps[0].arg_f64("y2"), Some(100.0));

        // 起端顶边（step 2）：y = cy + h1/2 = 300
        assert_eq!(out.steps[2].arg_f64("y1"), Some(300.0));

        // 末端顶边（step 7）：y = cy + h2/2 = 400，z = z0 + length = 1300
        assert_eq!(out.steps[7].arg_f64("y1"), Some(400.0));
        assert_eq!(out.steps[7].arg_f64("z1"), Some(1300.0));
        assert_eq!(out.steps[7].arg_f64("z2"), Some(1300.0));
    }

    #[test]
    fn transition_op_recipe_roundtrips_through_json() {
        let input = OpInput::new(
            PropertyMap::new(CATEGORY)
                .with("start_width", Value::Float(400.0))
                .with("end_height", Value::Float(400.0)),
        )
        .at([10.0, 20.0, 30.0]);
        let out = DuctTransitionOp.apply(&input).unwrap();

        let text = serde_json::to_string(&out).expect("配方应可序列化");
        let back: OpOutput = serde_json::from_str(&text).expect("配方应可反序列化");
        assert_eq!(back, out, "JSON 往返后配方应逐字段相等");
    }

    // ---------- duct.elbow：三点弧脊线 Pipe 配方（G6-M12）----------

    #[test]
    fn elbow_op_emits_an_eight_step_recipe() {
        let input = OpInput::new(
            PropertyMap::new(CATEGORY)
                .with("width", Value::Float(1000.0))
                .with("turn_radius", Value::Float(1200.0)),
        )
        .at([1000.0, 0.0, 3000.0]);
        let out = DuctElbowOp.apply(&input).unwrap();

        // 8 步：MakeLineEdge×4 → MakeWire(断面) → MakeArcEdge → MakeWire(脊线) → Pipe
        assert_eq!(out.step_count(), 8);
        for i in [0usize, 1, 2, 3] {
            assert_eq!(out.steps[i].op, BuildOp::MakeLineEdge, "第 {i} 步应为直边");
        }
        assert_eq!(out.steps[4].op, BuildOp::MakeWire, "断面 wire");
        assert_eq!(out.steps[5].op, BuildOp::MakeArcEdge, "弧脊线");
        assert_eq!(out.steps[6].op, BuildOp::MakeWire, "脊线 wire");
        assert_eq!(out.steps[7].op, BuildOp::Pipe);

        // Pipe 的 inputs 顺序恒为 [profile, spine]；断面 wire 吃 4 条边；脊线 wire 吃弧。
        assert_eq!(out.steps[7].inputs, vec![4, 6]);
        assert_eq!(out.steps[6].inputs, vec![5]);
        assert_eq!(out.steps[4].inputs, vec![0, 1, 2, 3]);

        // 三点弧：起 (0,0,0) → 中 45° → 终 (R,0,R)（R = 1200）。
        let s45 = std::f64::consts::FRAC_1_SQRT_2;
        assert_eq!(out.steps[5].arg_f64("x1"), Some(0.0));
        assert_eq!(out.steps[5].arg_f64("x2"), Some(1200.0 * (1.0 - s45)));
        assert_eq!(out.steps[5].arg_f64("z2"), Some(1200.0 * s45));
        assert_eq!(out.steps[5].arg_f64("x3"), Some(1200.0));
        assert_eq!(out.steps[5].arg_f64("z3"), Some(1200.0));

        // 断面绕定位点居中（宽 1000 / 高默认 320）。
        assert_eq!(out.steps[0].arg_f64("y1"), Some(-500.0));
        assert_eq!(out.steps[0].arg_f64("z1"), Some(-160.0));

        // 「可用」尺子（静态侧）：inputs 只引用前序步骤、args 只用 §5.1 权威 key。
        assert!(out.is_well_ordered());
        for (i, step) in out.steps.iter().enumerate() {
            assert!(step.validate_arity().is_ok(), "第 {i} 步元数应合法");
            for key in step.args.keys() {
                assert!(
                    AUTHORITATIVE_KEYS.contains(&key.as_str()),
                    "第 {i} 步参数 key `{key}` 不在设计文档 §5.1 权威集内"
                );
            }
        }
    }

    #[test]
    fn elbow_op_rejects_non_positive_parameters() {
        for (key, value) in [("width", 0.0), ("height", -320.0), ("turn_radius", 0.0)] {
            let input = OpInput::new(PropertyMap::new(CATEGORY).with(key, Value::Float(value)));
            assert!(
                matches!(DuctElbowOp.apply(&input), Err(SdkError::BadParameter(_))),
                "参数 {key}={value} 必须被响亮拒绝（BadParameter）"
            );
        }
    }

    #[test]
    fn elbow_op_warns_when_radius_below_width() {
        // 转弯半径 < 风管宽：配方仍合法可建，但 warnings 落配方（不阻断）。
        let tight = OpInput::new(
            PropertyMap::new(CATEGORY)
                .with("width", Value::Float(1000.0))
                .with("turn_radius", Value::Float(600.0)),
        );
        let out = DuctElbowOp.apply(&tight).unwrap();
        assert_eq!(out.step_count(), 8);
        assert_eq!(out.warnings.len(), 1, "转弯半径小于宽度必须落 1 条警告");
        assert!(out.is_well_ordered(), "警告不阻断，配方仍应合法");

        // 常规（默认宽 500 ≤ 默认半径 600）：无警告。
        let roomy = OpInput::new(PropertyMap::new(CATEGORY));
        let out = DuctElbowOp.apply(&roomy).unwrap();
        assert!(
            out.warnings.is_empty(),
            "默认半径（600 ≥ 宽 500）不应产生警告"
        );
    }

    #[test]
    fn elbow_op_recipe_roundtrips_through_json() {
        let input =
            OpInput::new(PropertyMap::new(CATEGORY).with("turn_radius", Value::Float(900.0)))
                .at([10.0, 20.0, 30.0]);
        let out = DuctElbowOp.apply(&input).unwrap();

        let text = serde_json::to_string(&out).expect("配方应可序列化");
        let back: OpOutput = serde_json::from_str(&text).expect("配方应可反序列化");
        assert_eq!(back, out, "JSON 往返后配方应逐字段相等");
    }

    /// **G6-M13 对账钉（静态侧）**：三算子 `param_defs` 的键序 / 键名 /
    /// 缺省值与 `apply` 内 `f64_or` 逐字对账；参数全为可选（f64_or 自带
    /// 缺省，用户不给值也能建几何）——防 param_defs 侧手误。
    #[test]
    fn param_defs_match_the_apply_fallbacks() {
        // (算子, [(键, 缺省)])——与 apply 的 f64_or 键序一致。
        type Case<'a> = (&'a dyn GeometryOp, &'a [(&'static str, f64)]);
        let cases: [Case<'_>; 3] = [
            (
                &DuctRectOp,
                &[
                    ("width", DEFAULT_WIDTH_MM),
                    ("height", DEFAULT_HEIGHT_MM),
                    ("length", DEFAULT_LENGTH_MM),
                ],
            ),
            (
                &DuctTransitionOp,
                &[
                    ("start_width", DEFAULT_START_WIDTH_MM),
                    ("start_height", DEFAULT_START_HEIGHT_MM),
                    ("end_width", DEFAULT_END_WIDTH_MM),
                    ("end_height", DEFAULT_END_HEIGHT_MM),
                    ("length", DEFAULT_LENGTH_MM),
                ],
            ),
            (
                &DuctElbowOp,
                &[
                    ("width", DEFAULT_WIDTH_MM),
                    ("height", DEFAULT_HEIGHT_MM),
                    ("turn_radius", DEFAULT_TURN_RADIUS_MM),
                ],
            ),
        ];
        for (op, expected) in cases {
            let defs = op.param_defs();
            assert_eq!(
                defs.len(),
                expected.len(),
                "{} 的参数数应与 apply 对账",
                op.id()
            );
            for (d, (key, default)) in defs.iter().zip(expected.iter()) {
                assert_eq!(d.key, *key, "{} 的参数键应对账", op.id());
                assert_eq!(
                    d.default,
                    Some(ParamDefault::Float(*default)),
                    "{} 的键 `{}` 缺省应与 f64_or 一致",
                    op.id(),
                    d.key
                );
                assert!(!d.required, "{} 的键 `{}` 应为可选", op.id(), d.key);
            }
        }
    }

    /// **G6-M13 对账钉（行为侧）**：`param_defs` 的每个键填**非缺省哨兵值**
    /// 后，配方必须与全缺省版不同——键名对不上时哨兵被 `f64_or` 忽略、
    /// 配方不变，此钉即红（防"表单填了但算子读不到"的键名漂移）。
    #[test]
    fn param_defs_keys_feed_the_apply_recipe() {
        const SENTINEL: f64 = 777.0;
        let ops: [&dyn GeometryOp; 3] = [&DuctRectOp, &DuctTransitionOp, &DuctElbowOp];
        for op in ops {
            let baseline = op
                .apply(&OpInput::new(PropertyMap::new(CATEGORY)))
                .expect("全缺省输入应合法");
            for def in op.param_defs() {
                let mut input = OpInput::new(PropertyMap::new(CATEGORY));
                input.params.set(def.key, Value::Float(SENTINEL));
                match op.apply(&input) {
                    Ok(out) => assert_ne!(
                        out,
                        baseline,
                        "{} 的键 `{}` 填哨兵后配方应变化（键名对不上则被 f64_or 忽略）",
                        op.id(),
                        def.key
                    ),
                    Err(SdkError::BadParameter(_)) => {
                        // 键被读到但触发退化拒绝（若未来本模块算子加退化
                        // 校验）——键名对账成立，容忍。
                    }
                    Err(e) => panic!(
                        "{} 的键 `{}` 填哨兵后应只成功或 BadParameter，实得 {e}",
                        op.id(),
                        def.key
                    ),
                }
            }
        }
    }

    // ---------- 标注 / 图层 / 边界 ----------

    #[test]
    fn annotation_rule_binds_geometry_with_zero_decimals() {
        let c = DuctWidthRule.context();
        assert_eq!(c.need, AnnotationNeed::LinearDimension);
        assert!(c.binds_geometry());
        assert!(!c.requires_datum);
        assert_eq!(c.decimals, Some(0));
    }

    #[test]
    fn declared_layers_use_known_line_types() {
        // "Continuous" / "Dashed" 是 core-doc 的契约线型名——
        // 模块侧认错名字会在装载时被 core-app 的图层套用逻辑拒绝。
        for layer in duct_layers() {
            assert!(
                layer.line_type == "Continuous" || layer.line_type == "Dashed",
                "线型名必须是 core-doc 已知的契约字符串，实得 {}",
                layer.line_type
            );
            assert!(layer.width_mm > 0.0);
        }
    }

    #[test]
    fn the_module_cannot_redefine_datum_kinds() {
        // 顺手验证边界：模块想给 Core 加一种基准元素，必须被拒。
        let mut reg = Registry::new();
        assert!(matches!(
            reg.try_define_datum_kind("DuctAxis"),
            Err(SdkError::DatumKindReserved(_))
        ));
    }
}
