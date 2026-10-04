//! # cable-tunnel —— 电力专业标杆模块（M7c）
//!
//! 立项方案 §6.3：「二阶段先用 SDK 交付电力专业模块形成标杆，再横向复制到
//! 建筑专业」；§10 M7 行：「电力侧至少一个可用模块」。本 crate 就是那个
//! 电力侧标杆——结构**逐项复刻 `example-beam`**（唯一合法模板）。
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
//! ## 它注册了什么
//!
//! | 注册项 | 具体内容 |
//! |---|---|
//! | 品类 | `CABLE_TUNNEL`（电缆隧道） |
//! | 属性模式 | 净宽 / 净高 / 延米长度 / 覆土深度 / 衬砌厚度 |
//! | 元素工厂 | [`TunnelFactory`]（只造语义元素，不建几何） |
//! | 几何算子 | [`TunnelRectOp`]（毛坯 → 圆柱 → Cut → 平移配方）、[`TunnelDuctbankOp`]（矩形断面沿脊线 Pipe 配方）、[`TunnelTwinOp`]（双仓：两毛坯 → Fuse → 平移配方，G6-M12） |
//! | 标注规则 | [`TunnelClearWidthRule`]（线性尺寸，0 位小数） |
//! | 专业图层 | `TUNNEL-隧道` / `TUNNEL-电缆支架` |
//!
//! ## 「可用」尺子（M7 验收口径）
//!
//! - [`TunnelRectOp`]：全部操作数 Recipe 可表达（`MakeBox` → `MakeCylinder` →
//!   `Cut` → `Translate`），apps/desktop 的 e2e 走**真实 execute + 落盘断言**；
//! - [`TunnelDuctbankOp`]：`MakeLineEdge×4 → MakeWire(断面) → MakeLineEdge(脊线)
//!   → MakeWire → Pipe`——M7a 线框算子解锁的**线状构件**全链。
//!
//! 两者的配方坐标（断面尺寸 / 脊线长 / 定位点）都在 [`GeometryOp::apply`]
//! 时由参数计算，**不新增任何 args key**（铁律 5：key 权威来源 = 设计文档 §5.1）。

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::panic))]

use core_sdk::{
    AnnotationContext, AnnotationNeed, AnnotationRule, BuildOp, BuildStep, CreateRequest, Element,
    ElementFactory, FieldDef, FieldType, GeometryOp, LayerSpec, Module, ModuleManifest, OpInput,
    OpOutput, PropertyMap, PropertySchema, Registry, SdkError, SdkResult, Value, build_element,
};

/// 模块 id（反向域名风格）。
pub const MODULE_ID: &str = "power.tunnel";

/// 品类名（= 属性模式名，Core 用字符串作键）。
pub const CATEGORY: &str = "CABLE_TUNNEL";

/// 矩形电缆隧道段几何算子 id。
pub const GEOMETRY_OP_ID_RECT: &str = "power.tunnel.rect";

/// 电缆排管段几何算子 id。
pub const GEOMETRY_OP_ID_DUCTBANK: &str = "power.tunnel.ductbank";

/// 双仓隧道段几何算子 id（G6-M12：第三个算子）。
pub const GEOMETRY_OP_ID_TWIN: &str = "power.tunnel.twin";

/// 标注规则 id。
pub const ANNOTATION_RULE_ID: &str = "power.tunnel.clear_width";

/// 默认净宽（mm）。
pub const DEFAULT_CLEAR_WIDTH_MM: f64 = 2000.0;
/// 默认净高（mm）。
pub const DEFAULT_CLEAR_HEIGHT_MM: f64 = 2100.0;
/// 默认延米长度（mm）。
pub const DEFAULT_LENGTH_MM: f64 = 6000.0;
/// 默认覆土深度（mm）。
pub const DEFAULT_COVER_DEPTH_MM: f64 = 1000.0;
/// 默认衬砌厚度（mm）。
pub const DEFAULT_LINING_THICKNESS_MM: f64 = 250.0;
/// 默认排管孔半径（mm）。
pub const DEFAULT_DUCT_RADIUS_MM: f64 = 75.0;
/// 默认排管孔深（mm，圆柱刀具的高）。
pub const DEFAULT_DUCT_LENGTH_MM: f64 = 300.0;
/// 默认中隔墙厚（mm）。
pub const DEFAULT_WALL_THICKNESS_MM: f64 = 300.0;

/// 净宽警告阈值（mm）：超过即提示确认断面分孔布置（承接遗留 #6 的可观测警告）。
pub const CLEAR_WIDTH_WARN_LIMIT_MM: f64 = 3000.0;

/// 排管断面尺寸换算系数：断面宽 = 高 = `本系数 × 孔半径`（孔径 `2r` + 四周壁厚各 `r`）。
pub const DUCT_SECTION_PER_RADIUS: f64 = 4.0;

// =============================================================================
// 属性模式
// =============================================================================

/// 电缆隧道的属性模式。
///
/// 注意字段名全是**专业词汇**（净宽/净高/延米/覆土/衬砌）——Core 一个都不认识，
/// 这正是模块存在的意义（设计文档 9.3：Core 永远不为加专业而改）。
#[must_use]
pub fn tunnel_schema() -> PropertySchema {
    PropertySchema::new(CATEGORY)
        .with_field(
            FieldDef::new("clear_width", FieldType::Float, "净宽")
                .with_unit("mm")
                .required()
                .with_default(Value::Float(DEFAULT_CLEAR_WIDTH_MM)),
        )
        .with_field(
            FieldDef::new("clear_height", FieldType::Float, "净高")
                .with_unit("mm")
                .required()
                .with_default(Value::Float(DEFAULT_CLEAR_HEIGHT_MM)),
        )
        .with_field(
            FieldDef::new("length", FieldType::Float, "延米长度")
                .with_unit("mm")
                .required()
                .with_default(Value::Float(DEFAULT_LENGTH_MM)),
        )
        .with_field(
            FieldDef::new("cover_depth", FieldType::Float, "覆土深度")
                .with_unit("mm")
                .required()
                .with_default(Value::Float(DEFAULT_COVER_DEPTH_MM)),
        )
        .with_field(
            FieldDef::new("lining_thickness", FieldType::Float, "衬砌厚度")
                .with_unit("mm")
                .required()
                .with_default(Value::Float(DEFAULT_LINING_THICKNESS_MM)),
        )
}

// =============================================================================
// 元素工厂
// =============================================================================

/// 电缆隧道的元素工厂。
///
/// **只造语义元素**：返回的 [`Element`] 没有几何，`geometry_ref` 由
/// [`TunnelRectOp`] / [`TunnelDuctbankOp`] 的配方执行完再回填（设计文档 9.2）。
pub struct TunnelFactory;

impl ElementFactory for TunnelFactory {
    fn category(&self) -> &str {
        CATEGORY
    }

    fn display_name(&self) -> &str {
        "电缆隧道"
    }

    fn schema(&self) -> Option<PropertySchema> {
        Some(tunnel_schema())
    }

    fn create(&self, request: &CreateRequest) -> SdkResult<Element> {
        let mut e = build_element(self.category(), request, "电缆隧道");
        e.properties.set(
            "clear_width",
            Value::Float(
                request
                    .param_f64("clear_width")
                    .unwrap_or(DEFAULT_CLEAR_WIDTH_MM),
            ),
        );
        e.properties.set(
            "clear_height",
            Value::Float(
                request
                    .param_f64("clear_height")
                    .unwrap_or(DEFAULT_CLEAR_HEIGHT_MM),
            ),
        );
        e.properties.set(
            "length",
            Value::Float(request.param_f64("length").unwrap_or(DEFAULT_LENGTH_MM)),
        );
        e.properties.set(
            "cover_depth",
            Value::Float(
                request
                    .param_f64("cover_depth")
                    .unwrap_or(DEFAULT_COVER_DEPTH_MM),
            ),
        );
        e.properties.set(
            "lining_thickness",
            Value::Float(
                request
                    .param_f64("lining_thickness")
                    .unwrap_or(DEFAULT_LINING_THICKNESS_MM),
            ),
        );
        Ok(e)
    }

    fn default_params(&self) -> PropertyMap {
        PropertyMap::new(CATEGORY)
            .with("clear_width", Value::Float(DEFAULT_CLEAR_WIDTH_MM))
            .with("clear_height", Value::Float(DEFAULT_CLEAR_HEIGHT_MM))
            .with("length", Value::Float(DEFAULT_LENGTH_MM))
            .with("cover_depth", Value::Float(DEFAULT_COVER_DEPTH_MM))
            .with(
                "lining_thickness",
                Value::Float(DEFAULT_LINING_THICKNESS_MM),
            )
    }
}

// =============================================================================
// 几何算子（返回配方，不直接建几何）
// =============================================================================

/// 矩形电缆隧道段的几何算子（power.tunnel.rect）。
///
/// 它返回的是**四步行之有效的配方**（`MakeBox` → `MakeCylinder` → `Cut` →
/// `Translate`），不是内核调用。执行配方的是 `core-app`（模块看不到内核）。
///
/// 与 `example-beam` 的开孔梁算子（`example.beam.hole`）同型（毛坯 → 圆柱刀具 →
/// Cut → 平移）：排管孔是**圆柱刀具**（半径 `duct_radius`、深 `duct_length`），
/// 沿毛坯原点处钻入。净宽超过 [`CLEAR_WIDTH_WARN_LIMIT_MM`] 时落**配方警告**
/// （不阻断，用户要看得到）。
pub struct TunnelRectOp;

impl GeometryOp for TunnelRectOp {
    fn id(&self) -> &str {
        GEOMETRY_OP_ID_RECT
    }

    fn display_name(&self) -> &str {
        "矩形电缆隧道段"
    }

    fn description(&self) -> &str {
        "由 净宽×净高×延米 生成隧道段毛坯并按孔径/孔深开排管孔，再按定位点放置"
    }

    fn apply(&self, input: &OpInput) -> SdkResult<OpOutput> {
        let width = input.f64_or("clear_width", DEFAULT_CLEAR_WIDTH_MM);
        let height = input.f64_or("clear_height", DEFAULT_CLEAR_HEIGHT_MM);
        let length = input.f64_or("length", DEFAULT_LENGTH_MM);
        let duct_radius = input.f64_or("duct_radius", DEFAULT_DUCT_RADIUS_MM);
        let duct_length = input.f64_or("duct_length", DEFAULT_DUCT_LENGTH_MM);

        if width <= 0.0
            || height <= 0.0
            || length <= 0.0
            || duct_radius <= 0.0
            || duct_length <= 0.0
        {
            return Err(SdkError::BadParameter(
                "隧道的净宽/净高/延米与排管孔径/孔深必须为正数（单位 mm）".to_string(),
            ));
        }

        let [x, y, z] = input.anchor;
        let body = BuildStep::new(BuildOp::MakeBox)
            .with_f64("dx", width)
            .with_f64("dy", height)
            .with_f64("dz", length)
            .with_note("隧道段毛坯");
        let hole = BuildStep::new(BuildOp::MakeCylinder)
            .with_f64("radius", duct_radius)
            .with_f64("height", duct_length)
            .with_note("排管孔刀具");
        let opened = BuildStep::new(BuildOp::Cut)
            .from_steps(vec![0, 1])
            .with_note("开排管孔");
        let placed = BuildStep::new(BuildOp::Translate)
            .from_steps(vec![2])
            .with_f64("x", x)
            .with_f64("y", y)
            .with_f64("z", z)
            .with_note("按定位点放置");

        let mut out = OpOutput::of(vec![body, hole, opened, placed]);
        if width > CLEAR_WIDTH_WARN_LIMIT_MM {
            out = out.with_warning("隧道净宽超过 3m，请确认断面分孔布置或采用双仓断面");
        }
        Ok(out)
    }
}

/// 电缆排管段的几何算子（power.tunnel.ductbank，M7a 线框算子解锁的线状构件）。
///
/// 它返回的是**八步行之有效的配方**：
///
/// ```text
/// MakeLineEdge×4 → MakeWire(矩形断面) → MakeLineEdge(脊线) → MakeWire → Pipe
/// ```
///
/// 断面是**矩形 wire**（`MakeLineEdge×4 + MakeWire`，M7a 变体）；脊线沿延米方向；
/// `Pipe` 把断面沿脊线扫成排管通道。配方坐标（断面尺寸 / 脊线长 / 定位点）在
/// [`GeometryOp::apply`] 时由参数计算：
///
/// - 断面宽 = 高 = [`DUCT_SECTION_PER_RADIUS`] × `duct_radius`（孔径 2r + 壁厚 r）；
/// - 脊线长 = `length`（延米）；
/// - 定位点 = `anchor`（断面基点与脊线起点重合）。
///
/// **不新增任何 args key**：`MakeLineEdge` 用 `x1/y1/z1/x2/y2/z2`，`MakeWire` /
/// `Pipe` 无 args——全部是设计文档 §5.1 权威 key（铁律 5）。
pub struct TunnelDuctbankOp;

impl GeometryOp for TunnelDuctbankOp {
    fn id(&self) -> &str {
        GEOMETRY_OP_ID_DUCTBANK
    }

    fn display_name(&self) -> &str {
        "电缆排管段"
    }

    fn description(&self) -> &str {
        "由 孔径换算的矩形断面沿延米方向扫成排管通道，定位点为断面基点"
    }

    fn apply(&self, input: &OpInput) -> SdkResult<OpOutput> {
        let duct_radius = input.f64_or("duct_radius", DEFAULT_DUCT_RADIUS_MM);
        let length = input.f64_or("length", DEFAULT_LENGTH_MM);

        if duct_radius <= 0.0 || length <= 0.0 {
            return Err(SdkError::BadParameter(
                "排管的孔径与延米长度必须为正数（单位 mm）".to_string(),
            ));
        }

        let [x, y, z] = input.anchor;
        // 断面尺寸（mm）：孔径 2r + 四周壁厚各 r = 4r（换算规则见常量文档）。
        let s = DUCT_SECTION_PER_RADIUS * duct_radius;
        let section_edge = |(ax, ay, az): (f64, f64, f64), (bx, by, bz): (f64, f64, f64)| {
            BuildStep::new(BuildOp::MakeLineEdge)
                .with_f64("x1", ax)
                .with_f64("y1", ay)
                .with_f64("z1", az)
                .with_f64("x2", bx)
                .with_f64("y2", by)
                .with_f64("z2", bz)
        };
        // 矩形断面（位于脊线起点的法平面，逆时针闭合）。
        let e0 = section_edge((x, y, z), (x, y + s, z)).with_note("断面底边");
        let e1 = section_edge((x, y + s, z), (x, y + s, z + s)).with_note("断面侧边");
        let e2 = section_edge((x, y + s, z + s), (x, y, z + s)).with_note("断面顶边");
        let e3 = section_edge((x, y, z + s), (x, y, z)).with_note("断面闭合边");
        let profile = BuildStep::new(BuildOp::MakeWire)
            .from_steps(vec![0, 1, 2, 3])
            .with_note("排管断面 wire");
        // 脊线（延米方向）。
        let e4 = section_edge((x, y, z), (x + length, y, z)).with_note("排管脊线");
        let spine = BuildStep::new(BuildOp::MakeWire)
            .from_steps(vec![5])
            .with_note("脊线 wire");
        // Pipe 的 inputs 顺序恒为 [profile, spine]（几何服务 Pipe 派发臂口径）。
        let piped = BuildStep::new(BuildOp::Pipe)
            .from_steps(vec![4, 6])
            .with_note("扫成排管通道");

        Ok(OpOutput::of(vec![
            e0, e1, e2, e3, profile, e4, spine, piped,
        ]))
    }
}

/// 双仓隧道段的几何算子（power.tunnel.twin，G6-M12：第三个算子）。
///
/// 它返回的是**五步行之有效的配方**：
///
/// ```text
/// MakeBox(左仓) → MakeBox(右仓) → Translate(右仓让位) → Fuse → Translate(定位)
/// ```
///
/// 两仓都是净断面毛坯（`MakeBox`，dx=净宽、dy=净高、dz=延米）；右仓沿 +X
/// 让位 `净宽 + 中隔墙厚`（`Translate`）；`Fuse`（布尔并）把两仓并成**双仓
/// 断面**——这是配方世界首个用 **`Fuse`** 的模块算子，也正是 [`TunnelRectOp`]
/// 超宽警告（"请确认断面分孔布置或采用双仓断面"）给出的对策。两仓不相接
/// （中隔墙厚 > 0），并集是两个不相连实体的复合体。
///
/// **不新增任何 args key**：`MakeBox` 用 `dx/dy/dz`，`Translate` 用 `x/y/z`，
/// `Fuse` 无 args——全部是设计文档 §5.1 权威 key（铁律 5）。
pub struct TunnelTwinOp;

impl GeometryOp for TunnelTwinOp {
    fn id(&self) -> &str {
        GEOMETRY_OP_ID_TWIN
    }

    fn display_name(&self) -> &str {
        "双仓隧道段"
    }

    fn description(&self) -> &str {
        "由 两个单仓毛坯沿 +X 并排（中隔墙厚让位）布尔并成双仓断面，再按定位点放置"
    }

    fn apply(&self, input: &OpInput) -> SdkResult<OpOutput> {
        let width = input.f64_or("clear_width", DEFAULT_CLEAR_WIDTH_MM);
        let height = input.f64_or("clear_height", DEFAULT_CLEAR_HEIGHT_MM);
        let length = input.f64_or("length", DEFAULT_LENGTH_MM);
        let wall = input.f64_or("wall_thickness", DEFAULT_WALL_THICKNESS_MM);

        if width <= 0.0 || height <= 0.0 || length <= 0.0 || wall <= 0.0 {
            return Err(SdkError::BadParameter(
                "隧道的净宽/净高/延米与中隔墙厚必须为正数（单位 mm）".to_string(),
            ));
        }

        let [x, y, z] = input.anchor;
        let left = BuildStep::new(BuildOp::MakeBox)
            .with_f64("dx", width)
            .with_f64("dy", height)
            .with_f64("dz", length)
            .with_note("左仓毛坯");
        let right = BuildStep::new(BuildOp::MakeBox)
            .with_f64("dx", width)
            .with_f64("dy", height)
            .with_f64("dz", length)
            .with_note("右仓毛坯");
        let shifted = BuildStep::new(BuildOp::Translate)
            .from_steps(vec![1])
            .with_f64("x", width + wall)
            .with_f64("y", 0.0)
            .with_f64("z", 0.0)
            .with_note("右仓沿 +X 让位（中隔墙厚）");
        let fused = BuildStep::new(BuildOp::Fuse)
            .from_steps(vec![0, 2])
            .with_note("并成双仓断面");
        let placed = BuildStep::new(BuildOp::Translate)
            .from_steps(vec![3])
            .with_f64("x", x)
            .with_f64("y", y)
            .with_f64("z", z)
            .with_note("按定位点放置");

        let mut out = OpOutput::of(vec![left, right, shifted, fused, placed]);
        if width > CLEAR_WIDTH_WARN_LIMIT_MM {
            out = out.with_warning("单仓净宽超过 3m，双仓断面的单仓宜控制在 3m 以内");
        }
        Ok(out)
    }
}

// =============================================================================
// 标注规则
// =============================================================================

/// 隧道净宽标注规则。
///
/// 模块只说"要一个线性尺寸、0 位小数"；**怎么画**（尺寸界线、箭头、文字样式）
/// 由 `core-doc` 依 `DraftingStandard` 决定。
pub struct TunnelClearWidthRule;

impl AnnotationRule for TunnelClearWidthRule {
    fn id(&self) -> &str {
        ANNOTATION_RULE_ID
    }

    fn display_name(&self) -> &str {
        "隧道净宽标注"
    }

    fn need(&self) -> AnnotationNeed {
        AnnotationNeed::LinearDimension
    }

    fn context(&self) -> AnnotationContext {
        AnnotationContext::new(AnnotationNeed::LinearDimension)
            .with_decimals(0)
            .with_description("隧道的净宽（mm）")
    }
}

// =============================================================================
// 专业图层
// =============================================================================

/// 本模块声明的专业图层（`LayerId` 由 Core 在装载时分配）。
#[must_use]
pub fn tunnel_layers() -> Vec<LayerSpec> {
    vec![
        LayerSpec::new("TUNNEL-隧道", [255, 0, 0], 0.50).with_description("电缆隧道轮廓"),
        LayerSpec::new("TUNNEL-电缆支架", [0, 0, 255], 0.25)
            .with_line_type("Dashed")
            .with_description("隧道内电缆支架（示意）"),
    ]
}

// =============================================================================
// 模块本体
// =============================================================================

/// 电缆隧道模块（电力侧标杆）。
///
/// `register` 是**纯粹的注册动作**（设计文档 9.2：不 I/O、不读数、不启线程）。
pub struct CableTunnel;

impl Module for CableTunnel {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest::new(MODULE_ID, "电缆隧道模块", env!("CARGO_PKG_VERSION"))
            .with_description("电力专业标杆模块：注册品类 / 属性模式 / 工厂 / 算子 / 标注 / 图层")
            .with_vendor("轻量化 BIM 内核平台")
    }

    fn register(&self, registry: &mut Registry) -> SdkResult<()> {
        registry.register_module(&self.manifest())?;
        registry.register_category(CATEGORY, "电缆隧道", MODULE_ID)?;
        registry.register_schema(tunnel_schema())?;
        registry.register_factory(Box::new(TunnelFactory))?;
        registry.register_geometry_op(Box::new(TunnelRectOp))?;
        registry.register_geometry_op(Box::new(TunnelDuctbankOp))?;
        registry.register_geometry_op(Box::new(TunnelTwinOp))?;
        registry.register_annotation_rule(Box::new(TunnelClearWidthRule))?;
        for layer in tunnel_layers() {
            registry.register_layer(layer)?;
        }
        Ok(())
    }

    /// 把刚注册的东西逐项回读一遍。
    ///
    /// 这是"体检"的核心：注册时报了错名字、或某一项根本没进注册表，
    /// 都会在这里暴露，而不是等到用户点"新建隧道"才发现。
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
        if registry.geometry_op(GEOMETRY_OP_ID_DUCTBANK).is_none() {
            missing.push(format!("几何算子 {GEOMETRY_OP_ID_DUCTBANK}"));
        }
        if registry.geometry_op(GEOMETRY_OP_ID_TWIN).is_none() {
            missing.push(format!("几何算子 {GEOMETRY_OP_ID_TWIN}"));
        }
        if !registry.annotation_rule_ids().contains(&ANNOTATION_RULE_ID) {
            missing.push(format!("标注规则 {ANNOTATION_RULE_ID}"));
        }
        let registered = registry.layers();
        for layer in tunnel_layers() {
            if !registered.iter().any(|l| l.name == layer.name) {
                missing.push(format!("图层 {}", layer.name));
            }
        }

        if missing.is_empty() {
            Ok(())
        } else {
            Err(SdkError::Factory(format!(
                "电缆隧道模块自检失败，注册表缺少：{}",
                missing.join("、")
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_sdk::ModuleId;

    /// 设计文档 §5.1 权威 args key 全集（配方参数 key 的唯一合法来源；
    /// M7a 线框算子补 x1..z3 九坐标键——本配方只用 x1..z2）。
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
        reg.install(&CableTunnel).expect("电缆隧道模块应当能装载");
        reg
    }

    #[test]
    fn manifest_is_valid_and_namespaced() {
        let m = CableTunnel.manifest();
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
        assert_eq!(s.geometry_ops, 3, "rect + ductbank + twin 三个几何算子");
        assert_eq!(s.annotation_rules, 1);
        assert_eq!(s.layers, 2);
    }

    #[test]
    fn self_check_passes_after_a_clean_install() {
        let reg = installed();
        assert!(CableTunnel.self_check(&reg).is_ok());
    }

    #[test]
    fn self_check_reports_a_missing_category() {
        // 往注册表里"少装一样东西"来验证自检真的会红。
        let mut reg = Registry::new();
        reg.register_module(&CableTunnel.manifest()).unwrap();
        let err = CableTunnel.self_check(&reg).unwrap_err();
        assert!(matches!(err, SdkError::Factory(_)));
    }

    #[test]
    fn factory_produces_a_geometryless_element_with_defaults() {
        let e = TunnelFactory.create(&CreateRequest::new("")).unwrap();
        assert_eq!(e.category.as_str(), CATEGORY);
        assert_eq!(e.name, "电缆隧道", "空名应回落到显示名");
        assert!(!e.has_geometry(), "工厂不建几何");
        assert_eq!(e.properties.schema, CATEGORY);
        assert_eq!(
            e.properties.get_float("clear_width"),
            Some(DEFAULT_CLEAR_WIDTH_MM)
        );
        assert_eq!(
            e.properties.get_float("cover_depth"),
            Some(DEFAULT_COVER_DEPTH_MM)
        );
        assert_eq!(
            e.properties.get_float("lining_thickness"),
            Some(DEFAULT_LINING_THICKNESS_MM)
        );
    }

    #[test]
    fn factory_honours_user_parameters() {
        let e = TunnelFactory
            .create(&CreateRequest::new("TD-1").with_param("clear_width", Value::Float(3600.0)))
            .unwrap();
        assert_eq!(e.name, "TD-1");
        assert_eq!(e.properties.get_float("clear_width"), Some(3600.0));
    }

    // ---------- tunnel.rect：配方有序性 / 参数拒绝 / warnings 落配方 ----------

    #[test]
    fn rect_op_emits_a_well_ordered_recipe() {
        let input = OpInput::new(PropertyMap::new(CATEGORY).with("length", Value::Float(9000.0)))
            .at([1000.0, 0.0, 3000.0]);
        let out = TunnelRectOp.apply(&input).unwrap();

        // 4 步：MakeBox → MakeCylinder → Cut → Translate
        assert_eq!(out.step_count(), 4);
        assert_eq!(out.steps[0].op, BuildOp::MakeBox);
        assert_eq!(out.steps[1].op, BuildOp::MakeCylinder);
        assert_eq!(out.steps[2].op, BuildOp::Cut);
        assert_eq!(out.steps[3].op, BuildOp::Translate);

        assert_eq!(out.steps[0].arg_f64("dx"), Some(DEFAULT_CLEAR_WIDTH_MM));
        assert_eq!(out.steps[0].arg_f64("dy"), Some(DEFAULT_CLEAR_HEIGHT_MM));
        assert_eq!(out.steps[0].arg_f64("dz"), Some(9000.0));
        assert_eq!(out.steps[1].arg_f64("radius"), Some(DEFAULT_DUCT_RADIUS_MM));
        assert_eq!(out.steps[1].arg_f64("height"), Some(DEFAULT_DUCT_LENGTH_MM));

        // Cut 步元数 2 的 inputs 正确性：吃毛坯 + 孔刀具
        assert_eq!(out.steps[2].inputs, vec![0, 1]);
        assert!(out.steps[2].validate_arity().is_ok());
        // Translate 吃 Cut 的结果
        assert_eq!(out.steps[3].inputs, vec![2]);

        assert!(out.is_well_ordered());
        for step in &out.steps {
            assert!(step.validate_arity().is_ok());
        }
    }

    #[test]
    fn rect_op_rejects_non_positive_parameters() {
        for (key, value) in [
            ("clear_width", 0.0),
            ("clear_height", -2100.0),
            ("length", 0.0),
            ("duct_radius", 0.0),
            ("duct_length", -300.0),
        ] {
            let input = OpInput::new(PropertyMap::new(CATEGORY).with(key, Value::Float(value)));
            assert!(
                matches!(TunnelRectOp.apply(&input), Err(SdkError::BadParameter(_))),
                "参数 {key}={value} 必须被响亮拒绝（BadParameter）"
            );
        }
    }

    #[test]
    fn rect_op_warns_when_clear_width_exceeds_3m() {
        // 超宽（3600 > 3000）：配方仍合法可建，但 warnings 落配方（遗留 #6）。
        let wide =
            OpInput::new(PropertyMap::new(CATEGORY).with("clear_width", Value::Float(3600.0)));
        let out = TunnelRectOp.apply(&wide).unwrap();
        assert_eq!(out.step_count(), 4);
        assert_eq!(out.warnings.len(), 1, "净宽超过 3m 必须落 1 条警告");
        assert!(out.warnings[0].contains("3m"), "警告应提示净宽超过 3m");
        assert!(out.is_well_ordered(), "警告不阻断，配方仍应合法");

        // 常规宽（默认 2000）：无警告。
        let normal = OpInput::new(PropertyMap::new(CATEGORY));
        let out = TunnelRectOp.apply(&normal).unwrap();
        assert!(out.warnings.is_empty(), "常规净宽不应产生警告");
    }

    #[test]
    fn rect_op_recipe_roundtrips_through_json() {
        let input = OpInput::new(
            PropertyMap::new(CATEGORY)
                .with("clear_width", Value::Float(3600.0))
                .with("duct_radius", Value::Float(60.0)),
        )
        .at([10.0, 20.0, 30.0]);
        let out = TunnelRectOp.apply(&input).unwrap();

        let text = serde_json::to_string(&out).expect("配方应可序列化");
        let back: OpOutput = serde_json::from_str(&text).expect("配方应可反序列化");
        assert_eq!(back, out, "JSON 往返后配方应逐字段相等");
    }

    // ---------- tunnel.ductbank：操作数 Recipe 可表达（「可用」尺子静态侧）----------

    #[test]
    fn ductbank_op_recipe_uses_only_recipe_expressible_operands() {
        let input = OpInput::new(PropertyMap::new(CATEGORY).with("length", Value::Float(9000.0)))
            .at([1000.0, 0.0, 3000.0]);
        let out = TunnelDuctbankOp.apply(&input).unwrap();

        // 8 步：MakeLineEdge×4 → MakeWire(断面) → MakeLineEdge(脊线) → MakeWire → Pipe
        assert_eq!(out.step_count(), 8);
        assert_eq!(out.steps[0].op, BuildOp::MakeLineEdge);
        assert_eq!(out.steps[1].op, BuildOp::MakeLineEdge);
        assert_eq!(out.steps[2].op, BuildOp::MakeLineEdge);
        assert_eq!(out.steps[3].op, BuildOp::MakeLineEdge);
        assert_eq!(out.steps[4].op, BuildOp::MakeWire);
        assert_eq!(out.steps[5].op, BuildOp::MakeLineEdge);
        assert_eq!(out.steps[6].op, BuildOp::MakeWire);
        assert_eq!(out.steps[7].op, BuildOp::Pipe);

        // 「可用」尺子（静态侧）：每步的 inputs 只引用前序步骤（is_well_ordered），
        // args 只用 §5.1 权威 key——即配方每步操作数均可被 Recipe 表达。
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

        // MakeWire 断面吃 4 条边（变长）；脊线 wire 吃 1 条边；Pipe 吃 [断面, 脊线]。
        assert_eq!(out.steps[4].inputs, vec![0, 1, 2, 3]);
        assert_eq!(out.steps[6].inputs, vec![5]);
        assert_eq!(
            out.steps[7].inputs,
            vec![4, 6],
            "Pipe 的 inputs 顺序恒为 [profile, spine]"
        );
    }

    #[test]
    fn ductbank_op_rejects_non_positive_parameters() {
        for (key, value) in [("duct_radius", 0.0), ("length", -6000.0)] {
            let input = OpInput::new(PropertyMap::new(CATEGORY).with(key, Value::Float(value)));
            assert!(
                matches!(
                    TunnelDuctbankOp.apply(&input),
                    Err(SdkError::BadParameter(_))
                ),
                "参数 {key}={value} 必须被响亮拒绝（BadParameter）"
            );
        }
    }

    #[test]
    fn ductbank_op_scales_section_and_spine_from_parameters() {
        // 断面宽 = 高 = 4×孔径半径；脊线长 = 延米；定位点 = anchor（配方坐标
        // 在 apply() 时由参数计算，不新增 args key）。
        let input = OpInput::new(
            PropertyMap::new(CATEGORY)
                .with("duct_radius", Value::Float(100.0))
                .with("length", Value::Float(9000.0)),
        )
        .at([100.0, 200.0, 300.0]);
        let out = TunnelDuctbankOp.apply(&input).unwrap();

        let s = DUCT_SECTION_PER_RADIUS * 100.0;
        // 断面底边：(anchor) → (anchor + s 沿 y)
        assert_eq!(out.steps[0].arg_f64("x1"), Some(100.0));
        assert_eq!(out.steps[0].arg_f64("y1"), Some(200.0));
        assert_eq!(out.steps[0].arg_f64("y2"), Some(200.0 + s));
        // 脊线：anchor → anchor + 9000 沿 x
        assert_eq!(out.steps[5].arg_f64("x1"), Some(100.0));
        assert_eq!(out.steps[5].arg_f64("x2"), Some(9100.0));
    }

    // ---------- tunnel.twin：Fuse 双仓配方（G6-M12）----------

    #[test]
    fn twin_op_emits_a_five_step_recipe() {
        let input = OpInput::new(
            PropertyMap::new(CATEGORY)
                .with("clear_width", Value::Float(2000.0))
                .with("wall_thickness", Value::Float(300.0)),
        )
        .at([1000.0, 0.0, 3000.0]);
        let out = TunnelTwinOp.apply(&input).unwrap();

        // 5 步：MakeBox(左) → MakeBox(右) → Translate(让位) → Fuse → Translate(定位)
        assert_eq!(out.step_count(), 5);
        assert_eq!(out.steps[0].op, BuildOp::MakeBox);
        assert_eq!(out.steps[1].op, BuildOp::MakeBox);
        assert_eq!(out.steps[2].op, BuildOp::Translate);
        assert_eq!(out.steps[3].op, BuildOp::Fuse);
        assert_eq!(out.steps[4].op, BuildOp::Translate);

        // 右仓沿 +X 让位 净宽 + 中隔墙厚 = 2300；Fuse 吃 [左仓, 让位后的右仓]；
        // 定位 Translate 吃 Fuse 结果。
        assert_eq!(out.steps[2].inputs, vec![1]);
        assert_eq!(out.steps[2].arg_f64("x"), Some(2300.0));
        assert_eq!(out.steps[3].inputs, vec![0, 2]);
        assert_eq!(out.steps[4].inputs, vec![3]);
        assert_eq!(out.steps[4].arg_f64("x"), Some(1000.0));
        assert_eq!(out.steps[4].arg_f64("z"), Some(3000.0));

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
    fn twin_op_rejects_non_positive_parameters() {
        for (key, value) in [
            ("clear_width", 0.0),
            ("clear_height", -2100.0),
            ("length", 0.0),
            ("wall_thickness", -300.0),
        ] {
            let input = OpInput::new(PropertyMap::new(CATEGORY).with(key, Value::Float(value)));
            assert!(
                matches!(TunnelTwinOp.apply(&input), Err(SdkError::BadParameter(_))),
                "参数 {key}={value} 必须被响亮拒绝（BadParameter）"
            );
        }
    }

    #[test]
    fn twin_op_warns_when_single_cell_exceeds_3m() {
        // 单仓净宽超 3m：双仓正是 rect 算子超宽警告的对策，但单仓自身超限仍要提示。
        let wide =
            OpInput::new(PropertyMap::new(CATEGORY).with("clear_width", Value::Float(3600.0)));
        let out = TunnelTwinOp.apply(&wide).unwrap();
        assert_eq!(out.step_count(), 5);
        assert_eq!(out.warnings.len(), 1, "单仓净宽超过 3m 必须落 1 条警告");
        assert!(out.is_well_ordered(), "警告不阻断，配方仍应合法");

        // 常规宽（默认 2000）：无警告。
        let normal = OpInput::new(PropertyMap::new(CATEGORY));
        let out = TunnelTwinOp.apply(&normal).unwrap();
        assert!(out.warnings.is_empty(), "常规单仓净宽不应产生警告");
    }

    #[test]
    fn twin_op_recipe_roundtrips_through_json() {
        let input =
            OpInput::new(PropertyMap::new(CATEGORY).with("wall_thickness", Value::Float(250.0)))
                .at([10.0, 20.0, 30.0]);
        let out = TunnelTwinOp.apply(&input).unwrap();

        let text = serde_json::to_string(&out).expect("配方应可序列化");
        let back: OpOutput = serde_json::from_str(&text).expect("配方应可反序列化");
        assert_eq!(back, out, "JSON 往返后配方应逐字段相等");
    }

    // ---------- 标注 / 图层 / 边界 ----------

    #[test]
    fn annotation_rule_binds_geometry_with_zero_decimals() {
        let c = TunnelClearWidthRule.context();
        assert_eq!(c.need, AnnotationNeed::LinearDimension);
        assert!(c.binds_geometry());
        assert!(!c.requires_datum);
        assert_eq!(c.decimals, Some(0));
    }

    #[test]
    fn declared_layers_use_known_line_types() {
        // "Continuous" / "Dashed" 是 core-doc 的契约线型名——
        // 模块侧认错名字会在装载时被 core-app 的图层套用逻辑拒绝。
        for layer in tunnel_layers() {
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
            reg.try_define_datum_kind("TunnelAxis"),
            Err(SdkError::DatumKindReserved(_))
        ));
    }
}
