//! 内核适配层的对外数据类型。
//!
//! 这些类型是**内核的抽象**：上层只见它们，不见任何 `occt_wasm::` 类型（铁律 1）。

pub use glam::{DVec2, DVec3};

use crate::error::{KernelError, KernelResult};
use crate::handle::ShapeHandle;

/// 投影后二维曲线的解析类型。
///
/// **为什么必须区分类型**：出图要求"圆仍是圆、弧仍是弧"——只有这样才
/// 能标注半径/直径、才能捕捉圆心、DXF 里才能导出真 `CIRCLE` 而非折线
/// （设计文档 6.5、7.4）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CurveKind {
    /// 直线
    Line,
    /// 圆
    Circle,
    /// 圆弧
    Arc,
    /// 椭圆 / 椭圆弧
    Ellipse,
    /// B 样条曲线
    BSpline,
    /// 其它（退化情形或暂未识别的类型）
    Other,
}

/// HLR 的线型分类（对应 OCCT `HLRBRep_HLRToShape` 的六类输出）。
///
/// 决定出图线型：轮廓 / 尖锐 → 实线、光顺 → 细实线（设计文档 6.2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LineClass {
    /// 轮廓母线与外轮廓
    Outline,
    /// 光顺边（相切连续）
    Smooth,
    /// 尖锐边（真实棱线）
    Sharp,
}

/// 二维投影曲线的几何数据。
#[derive(Debug, Clone, PartialEq)]
pub enum CurveGeometry {
    /// 直线段：两端点
    Line {
        /// 起点
        a: DVec2,
        /// 终点
        b: DVec2,
    },
    /// 整圆
    Circle {
        /// 圆心
        center: DVec2,
        /// 半径
        radius: f64,
    },
    /// 圆弧（角度单位：弧度）
    Arc {
        /// 圆心
        center: DVec2,
        /// 半径
        radius: f64,
        /// 起始角（弧度）
        start: f64,
        /// 终止角（弧度）
        end: f64,
    },
    /// 椭圆 / 椭圆弧
    Ellipse {
        /// 中心
        center: DVec2,
        /// 长半轴
        rx: f64,
        /// 短半轴
        ry: f64,
        /// 旋转角（弧度）
        rot: f64,
    },
    /// B 样条
    BSpline {
        /// 控制点
        ctrl: Vec<DVec2>,
        /// 次数
        degree: u8,
        /// 节点向量
        knots: Vec<f64>,
    },
    /// 其它
    Other,
}

/// 一条投影后的二维曲线。
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectedCurve {
    /// 解析类型
    pub kind: CurveKind,
    /// 几何参数
    pub data: CurveGeometry,
    /// 线型分类
    pub layer_hint: LineClass,
}

/// 一次 HLR 投影的结果。
///
/// 对应 `occt-wasm` 的 `ProjectionData`（六个 B-Rep 复合体），
/// 这里按**可见性**归为两组，线型分类保留在 [`ProjectedCurve::layer_hint`]。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Projection {
    /// 可见曲线
    pub visible: Vec<ProjectedCurve>,
    /// 隐藏曲线（是否显示由出图层决定）
    pub hidden: Vec<ProjectedCurve>,
}

impl Projection {
    /// 是否为空（无任何曲线）。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.visible.is_empty() && self.hidden.is_empty()
    }

    /// 曲线总数。
    #[must_use]
    pub fn curve_count(&self) -> usize {
        self.visible.len() + self.hidden.len()
    }
}

/// 视图定义（设计文档 3.4 / 6.2）。
///
/// 精度要求：`direction` 与 `x_axis` 应为**单位向量**，否则结果不可预期。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewDef {
    /// 视点（投影中心）
    pub origin: DVec3,
    /// 视线方向（由物体指向观察者）
    pub direction: DVec3,
    /// 屏幕向右轴；`None` 时由内核自选
    pub x_axis: Option<DVec3>,
}

impl ViewDef {
    /// 构造一个"不给屏幕右轴"的视图。
    #[must_use]
    pub fn new(origin: DVec3, direction: DVec3) -> Self {
        Self {
            origin,
            direction,
            x_axis: None,
        }
    }

    /// 指定屏幕向右轴。
    #[must_use]
    pub fn with_x_axis(mut self, x_axis: DVec3) -> Self {
        self.x_axis = Some(x_axis);
        self
    }

    /// 俯视（视线朝下）——平面图常用。
    #[must_use]
    pub fn top(origin: DVec3) -> Self {
        Self::new(origin, DVec3::Z)
    }

    /// 正视（视线沿 -Y）——立面图常用。
    #[must_use]
    pub fn front(origin: DVec3) -> Self {
        Self::new(origin, DVec3::NEG_Y)
    }
}

/// 一个三角面对应的网格区间（拾取粗筛用，设计文档 6.6）。
///
/// `face_hash` 是内核给出的**面哈希**。⚠ **它跨会话不稳定，不能当持久 ID**；
/// 持久化必须落到 `core-model` 的 `ShapeId` + 拓扑绑定上。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FaceGroup {
    /// 该面在 `indices` 中的起始三角形序号（每三角形 3 个索引）
    pub tri_start: u32,
    /// 该面的三角形数量
    pub tri_count: u32,
    /// 面的会话哈希（**不可持久化**）
    pub face_hash: u64,
}

/// 网格数据（渲染层消费；也是拾取粗筛的输入）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MeshData {
    /// 顶点坐标（单位 mm）
    pub positions: Vec<[f64; 3]>,
    /// 三角形索引（每 3 个构成一个三角形）
    pub indices: Vec<u32>,
    /// 顶点法线（可能与 `positions` 等长，也可能为空表示待计算）
    pub normals: Vec<[f64; 3]>,
    /// 面分组（三角形 → 面哈希的映射）
    pub face_groups: Vec<FaceGroup>,
}

impl MeshData {
    /// 三角形数量。
    #[must_use]
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// 顶点数量。
    #[must_use]
    pub fn vertex_count(&self) -> usize {
        self.positions.len()
    }
}

/// 点到面的最近点投影结果（`project_point_on_face` 的适配层类型）。
///
/// ⚠ 这是**最近点投影**，**不是"射线命中判定"**：平面投影**无界**，
/// 任意远点都有结果。命中判定须自建（射线-三角粗筛，B4.1）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointOnFace {
    /// 面上最近点（mm）
    pub point: DVec3,
    /// 面局部参数 u
    pub u: f64,
    /// 面局部参数 v
    pub v: f64,
    /// 查询点到最近点的距离（mm）
    pub distance: f64,
}

impl PointOnFace {
    /// 由内核返回的原始向量构造；**长度必须精确 6**，否则 `Err`（绝不静默错位）。
    ///
    /// 内核返回语义：`[px,py,pz, u,v, distance]`。
    pub fn from_kernel_vec(v: &[f64]) -> KernelResult<Self> {
        if v.len() != 6 {
            return Err(KernelError::Geometry {
                op: "project_point_on_face",
                msg: format!(
                    "点投影结果元数不符：期望 6 元 [px,py,pz,u,v,distance]，实得 {} 元",
                    v.len()
                ),
            });
        }
        Ok(Self {
            point: DVec3::new(v[0], v[1], v[2]),
            u: v[3],
            v: v[4],
            distance: v[5],
        })
    }
}

/// 点到边的最近点投影结果（`project_point_on_edge` 的适配层类型）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointOnEdge {
    /// 边上最近点（mm）
    pub point: DVec3,
    /// 曲线切向 `dP/dparam`（**非归一化**，通常非单位向量）
    ///
    /// ⚠ **不是单位向量**：这是曲线对**参数**的导数。对直线边（按弧长参数化）
    /// 模长恰为 1，但对**圆弧边**模长 = 半径（实测 φ15 孔边 `|tangent| = 15`）。
    /// 需要单位切向时调用方**必须自行 `normalize()`**。
    pub tangent: DVec3,
    /// 曲线参数（**非归一化**）
    pub param: f64,
}

impl PointOnEdge {
    /// 由内核返回的原始向量构造；**长度必须精确 7**，否则 `Err`（绝不静默错位）。
    ///
    /// 内核返回语义：`[px,py,pz, tx,ty,tz, param]`（切向为曲线对**参数**的导数
    /// `dP/dparam`，**非归一化**——对圆弧边其模长等于半径；`param` 为**非归一化**
    /// 曲线参数）。
    pub fn from_kernel_vec(v: &[f64]) -> KernelResult<Self> {
        if v.len() != 7 {
            return Err(KernelError::Geometry {
                op: "project_point_on_edge",
                msg: format!(
                    "点投影结果元数不符：期望 7 元 [px,py,pz,tx,ty,tz,param]，实得 {} 元",
                    v.len()
                ),
            });
        }
        Ok(Self {
            point: DVec3::new(v[0], v[1], v[2]),
            tangent: DVec3::new(v[3], v[4], v[5]),
            param: v[6],
        })
    }
}

/// 曲线参数域 `[first, last]`（`curve_parameters` 的适配层类型）。
///
/// **非归一化**：这是内核曲线的原始参数区间，用于把 [`PointOnEdge::param`]
/// 归一化到 `[0, 1]`（`(t - first) / (last - first)`）。与 [`PointOnEdge`] 一样
/// **不带 serde**（会话态几何量，非持久正文）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CurveRange {
    /// 参数域下界。
    pub first: f64,
    /// 参数域上界。
    pub last: f64,
}

impl CurveRange {
    /// 由内核返回的原始向量构造；**长度必须精确 2**，否则 `Err`（绝不静默错位）。
    ///
    /// 内核返回语义：`[first, last]`（**非归一化**参数域）。
    pub fn from_kernel_vec(v: &[f64]) -> KernelResult<Self> {
        if v.len() != 2 {
            return Err(KernelError::Geometry {
                op: "curve_parameters",
                msg: format!(
                    "曲线参数域元数不符：期望 2 元 [first,last]，实得 {} 元",
                    v.len()
                ),
            });
        }
        Ok(Self {
            first: v[0],
            last: v[1],
        })
    }
}

/// 由内核返回的 3 元向量构造 `DVec3`；**长度必须精确 3**，否则 `Err`（绝不静默错位）。
///
/// 供 `surface_normal` 的 `[nx, ny, nz]` 结果映射使用（`op = "surface_normal"`）。
/// 外露 [`DVec3`] 而非 vendor 类型，符合铁律①（`occt_wasm::` 类型不出 `occt.rs`）。
pub fn dvec3_from_kernel_vec(v: &[f64]) -> KernelResult<DVec3> {
    dvec3_from_kernel_vec_op(v, "surface_normal")
}

/// 由内核返回的 3 元向量构造 `DVec3`，**错误时用调用方给定的 `op` 标签**。
///
/// 与 [`dvec3_from_kernel_vec`] 同一契约（**精确 3 元守卫，绝不静默错位**），
/// 但把 `op` 外提——因为多个门面（`surface_normal` / `curve_point_at_param` / …）
/// 都复用这条 3 元映射，若把 `op` 硬编码成 `"surface_normal"`，别的门面报错时
/// 的算子标签就会**误导定位**（B6.1a 据实细化）。
pub fn dvec3_from_kernel_vec_op(v: &[f64], op: &'static str) -> KernelResult<DVec3> {
    if v.len() != 3 {
        return Err(KernelError::Geometry {
            op,
            msg: format!("三维向量元数不符：期望 3 元 [x,y,z]，实得 {} 元", v.len()),
        });
    }
    Ok(DVec3::new(v[0], v[1], v[2]))
}

/// 轴对齐包围盒（设计文档 §5.3 / 几何查询）。
///
/// 由内核的 `get_bounding_box` 得出，**转换成适配层类型**后才允许外传
/// （铁律 1：`occt_wasm::` 类型不得出 `occt.rs`）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoundingBox {
    /// 最小角点（mm）
    pub min: [f64; 3],
    /// 最大角点（mm）
    pub max: [f64; 3],
}

impl BoundingBox {
    /// 由两角点构造。
    #[must_use]
    pub const fn new(min: [f64; 3], max: [f64; 3]) -> Self {
        Self { min, max }
    }

    /// 三个轴向尺寸（mm）。
    #[must_use]
    pub fn size(&self) -> [f64; 3] {
        [
            self.max[0] - self.min[0],
            self.max[1] - self.min[1],
            self.max[2] - self.min[2],
        ]
    }

    /// 几何中心（mm）。
    #[must_use]
    pub fn center(&self) -> [f64; 3] {
        [
            (self.min[0] + self.max[0]) / 2.0,
            (self.min[1] + self.max[1]) / 2.0,
            (self.min[2] + self.max[2]) / 2.0,
        ]
    }
}

/// 子形状种类（与 `occt-wasm` 的 `get_sub_shapes` 字符串一一对应）。
///
/// **这是 M3 定下的枚举原语**，供 §5.3 复合体拆分（`split_solids`）与 M5 的
/// 选择器直接生长。M3 **不实现**选择器 / 持久拓扑命名——那属 M5。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SubShapeKind {
    /// 实体
    Solid,
    /// 壳
    Shell,
    /// 面
    Face,
    /// 边
    Edge,
    /// 顶点
    Vertex,
    /// 线框
    Wire,
}

impl SubShapeKind {
    /// 传给内核 `get_sub_shapes` / `sub_shape_count` 的字符串短名。
    #[must_use]
    pub const fn tag(self) -> &'static str {
        match self {
            Self::Solid => "solid",
            Self::Shell => "shell",
            Self::Face => "face",
            Self::Edge => "edge",
            Self::Vertex => "vertex",
            Self::Wire => "wire",
        }
    }
}

/// 一次带历史布尔 / 变形的**演化结果**（会话内）。
///
/// ⚠ 这是 `core_kernel` 的**自有类型**，**不是** `occt_wasm::EvolutionData`——
/// `occt_wasm::` 类型不得出 `occt.rs`（铁律①）。所有面哈希都是**会话内**哈希，
/// 与 [`Kernel::face_hashes`](crate::Kernel::face_hashes) 同域
/// （`FACE_HASH_UPPER_BOUND`），**不可持久化**。
#[derive(Debug, Clone, PartialEq)]
pub struct Evolution {
    /// 结果形状（已收进 arena 的对外句柄）
    pub result: ShapeHandle,
    /// modified：输入面 → 被修改后的输出面（可能 1 → k）
    pub modified: Vec<FaceEvolution>,
    /// generated：输入面 → 新生成的输出面（可能 1 → k）
    pub generated: Vec<FaceEvolution>,
    /// deleted：被删除的输入面哈希（扁平表，无分组）
    pub deleted: Vec<u64>,
}

/// 一组演化关系：一个输入面哈希对应 k 个输出面哈希。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FaceEvolution {
    /// 输入面会话哈希
    pub input_hash: u64,
    /// 输出面会话哈希（k 个；`k == outputs.len()`）
    pub outputs: Vec<u64>,
}

impl Evolution {
    /// 结果形状面哈希 → 它的**来源输入面哈希**（modified + generated 反查）。
    ///
    /// B3 的"哪些面变了"就靠它：给定一个输出面哈希，
    /// 用 `sources_of(result_hash)` 反查即可知道它来自哪些输入面。
    #[must_use]
    pub fn sources_of(&self, output_hash: u64) -> Vec<u64> {
        self.modified
            .iter()
            .chain(self.generated.iter())
            .filter(|g| g.outputs.contains(&output_hash))
            .map(|g| g.input_hash)
            .collect()
    }

    /// 某个输入面哈希在本次操作后**对应的全部输出面哈希**（modified + generated）。
    ///
    /// 返回空 = 该输入面未受本次操作影响（或落在 `deleted` 里）。
    /// **空要能区分于"调用失败"**（失败是 `Err`，不是空 `Vec`）。
    #[must_use]
    pub fn outputs_of(&self, input_hash: u64) -> Vec<u64> {
        self.modified
            .iter()
            .chain(self.generated.iter())
            .filter(|g| g.input_hash == input_hash)
            .flat_map(|g| g.outputs.iter().copied())
            .collect()
    }
}

/// 会话面哈希 `i32` → `u64`（与 [`Kernel::face_hashes`](crate::Kernel::face_hashes)
/// / `OcctBackend::face_hashes` 同一抬升口径：位型无损、恒为非负）。
fn hash_to_u64(v: i32) -> u64 {
    u64::from(v as u32)
}

/// 把扁平的 `[输入面哈希, k, 输出面哈希 × k]*` 三元分组序列解码成
/// [`FaceEvolution`] 列表。
///
/// 这是**分组三元解码器**：`modified` / `generated` 都是这种扁平表（实测编码见
/// M5 规划 §0.3）。**越界 / 负数 `k` 一律 `Err(KernelError::Geometry)`，
/// 绝不静默截断**（`k == 0` 合法：表示该输入面对应零个输出面，但仍占一个组头）。
///
/// 放在本模块（**非 `occt` feature 门控**），以便默认腿可直接单测。
pub fn decode_groups(flat: &[i32]) -> KernelResult<Vec<FaceEvolution>> {
    let mut groups = Vec::new();
    let mut i = 0usize;
    while i < flat.len() {
        // 组头必须至少有 [in, k] 两元。
        if i + 2 > flat.len() {
            return Err(KernelError::Geometry {
                op: "decode_groups",
                msg: format!(
                    "组头不完整：i={i} 起剩余 {} 个元素，不足 [in,k] 两元",
                    flat.len() - i
                ),
            });
        }
        let input = flat[i];
        let k = flat[i + 1];
        if k < 0 {
            return Err(KernelError::Geometry {
                op: "decode_groups",
                msg: format!("i={i} 处组头含负的输出个数 k={k}"),
            });
        }
        let k = k as usize;
        // 组边界不得外溢：声明 k 个输出，就必须真有 k 个。
        if i + 2 + k > flat.len() {
            return Err(KernelError::Geometry {
                op: "decode_groups",
                msg: format!(
                    "组边界外溢：i={i} 声明 k={k}，需 {} 个元素，实际剩余 {}",
                    i + 2 + k,
                    flat.len() - i
                ),
            });
        }
        groups.push(FaceEvolution {
            input_hash: hash_to_u64(input),
            outputs: flat[i + 2..i + 2 + k]
                .iter()
                .map(|&v| hash_to_u64(v))
                .collect(),
        });
        i += 2 + k;
    }
    Ok(groups)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_def_builders_are_consistent() {
        let top = ViewDef::top(DVec3::new(0.0, 0.0, 1000.0));
        assert_eq!(top.direction, DVec3::Z);
        assert!(top.x_axis.is_none());

        let with_axis = ViewDef::front(DVec3::ZERO).with_x_axis(DVec3::X);
        assert_eq!(with_axis.x_axis, Some(DVec3::X));
    }

    #[test]
    fn empty_projection_reports_zero() {
        let p = Projection::default();
        assert!(p.is_empty());
        assert_eq!(p.curve_count(), 0);
    }

    #[test]
    fn mesh_counts_are_derived_from_buffers() {
        let mesh = MeshData {
            positions: vec![[0.0; 3]; 8],
            indices: vec![0, 1, 2, 2, 3, 0],
            normals: Vec::new(),
            face_groups: vec![FaceGroup {
                tri_start: 0,
                tri_count: 2,
                face_hash: 42,
            }],
        };
        assert_eq!(mesh.vertex_count(), 8);
        assert_eq!(mesh.triangle_count(), 2);
    }

    #[test]
    fn bounding_box_derives_size_and_center() {
        let bb = BoundingBox::new([0.0, 0.0, 0.0], [10.0, 20.0, 30.0]);
        assert_eq!(bb.size(), [10.0, 20.0, 30.0]);
        assert_eq!(bb.center(), [5.0, 10.0, 15.0]);
    }

    /// B4.0 钉子 ②：元数守卫——`from_kernel_vec` **长度不符必须 Err**（绝不静默错位）。
    #[test]
    fn point_projection_constructors_guard_arity() {
        // 面：精确 6 元 → [px,py,pz, u,v, distance]
        let f = PointOnFace::from_kernel_vec(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]).unwrap();
        assert_eq!(f.point, DVec3::new(1.0, 2.0, 3.0));
        assert_eq!(f.u, 4.0);
        assert_eq!(f.v, 5.0);
        assert_eq!(f.distance, 6.0);
        // 5 元 → Err(Geometry)
        assert!(matches!(
            PointOnFace::from_kernel_vec(&[1.0, 2.0, 3.0, 4.0, 5.0]),
            Err(KernelError::Geometry { op, .. }) if op == "project_point_on_face"
        ));
        // 7 元也 Err（面不接受 edge 的元数）
        assert!(matches!(
            PointOnFace::from_kernel_vec(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0]),
            Err(KernelError::Geometry { .. })
        ));

        // 边：精确 7 元 → [px,py,pz, tx,ty,tz, param]
        let e = PointOnEdge::from_kernel_vec(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0]).unwrap();
        assert_eq!(e.point, DVec3::new(1.0, 2.0, 3.0));
        assert_eq!(e.tangent, DVec3::new(4.0, 5.0, 6.0));
        assert_eq!(e.param, 7.0);
        // 2 元 → Err(Geometry)
        assert!(matches!(
            PointOnEdge::from_kernel_vec(&[1.0, 2.0]),
            Err(KernelError::Geometry { op, .. }) if op == "project_point_on_edge"
        ));
        // 6 元也 Err（边不接受 face 的元数）
        assert!(matches!(
            PointOnEdge::from_kernel_vec(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]),
            Err(KernelError::Geometry { .. })
        ));
    }

    /// B4.1b 钉子 ①：`CurveRange` / `dvec3_from_kernel_vec` 元数守卫（绝不静默错位）。
    #[test]
    fn curve_range_and_vec3_constructors_guard_arity() {
        // CurveRange：精确 2 元 → [first, last]
        let r = CurveRange::from_kernel_vec(&[1.0, 2.0]).unwrap();
        assert_eq!(r.first, 1.0);
        assert_eq!(r.last, 2.0);
        // 1 元 / 3 元 → Err(Geometry)
        assert!(matches!(
            CurveRange::from_kernel_vec(&[1.0]),
            Err(KernelError::Geometry { op, .. }) if op == "curve_parameters"
        ));
        assert!(matches!(
            CurveRange::from_kernel_vec(&[1.0, 2.0, 3.0]),
            Err(KernelError::Geometry { op, .. }) if op == "curve_parameters"
        ));

        // dvec3_from_kernel_vec：精确 3 元 → DVec3
        assert_eq!(
            dvec3_from_kernel_vec(&[1.0, 2.0, 3.0]).unwrap(),
            DVec3::new(1.0, 2.0, 3.0)
        );
        // 2 元 / 4 元 → Err(Geometry)
        assert!(matches!(
            dvec3_from_kernel_vec(&[1.0, 2.0]),
            Err(KernelError::Geometry { op, .. }) if op == "surface_normal"
        ));
        assert!(matches!(
            dvec3_from_kernel_vec(&[1.0, 2.0, 3.0, 4.0]),
            Err(KernelError::Geometry { op, .. }) if op == "surface_normal"
        ));

        // dvec3_from_kernel_vec_op：同一 3 元守卫，但错误标签取调用方传入的 `op`
        // （B6.1a：`curve_point_at_param` 复用该映射时不得误报成 `surface_normal`）。
        assert_eq!(
            dvec3_from_kernel_vec_op(&[1.0, 2.0, 3.0], "curve_point_at_param").unwrap(),
            DVec3::new(1.0, 2.0, 3.0)
        );
        assert!(matches!(
            dvec3_from_kernel_vec_op(&[1.0, 2.0], "curve_point_at_param"),
            Err(KernelError::Geometry { op, .. }) if op == "curve_point_at_param"
        ));
        assert!(matches!(
            dvec3_from_kernel_vec_op(&[1.0, 2.0, 3.0, 4.0], "curve_point_at_param"),
            Err(KernelError::Geometry { op, .. }) if op == "curve_point_at_param"
        ));
    }

    #[test]
    fn sub_shape_kinds_have_the_expected_tags() {
        assert_eq!(SubShapeKind::Solid.tag(), "solid");
        assert_eq!(SubShapeKind::Shell.tag(), "shell");
        assert_eq!(SubShapeKind::Face.tag(), "face");
        assert_eq!(SubShapeKind::Edge.tag(), "edge");
        assert_eq!(SubShapeKind::Vertex.tag(), "vertex");
        assert_eq!(SubShapeKind::Wire.tag(), "wire");
    }

    /// **"注定会失败"钉子**：分组三元 `[in,k,out×k]*` 必须解成正确分组；
    /// 坏边界（越界 `k` / 负 `k` / 组头残缺）必须 `Err`——**防"以为扁平表就是
    /// 输出哈希集"式误读，且绝不静默截断**。
    #[test]
    fn evolution_groups_decode_from_flat_triples() {
        // [in=11, k=1, out=101] [in=12, k=3, out=201,202,203] [in=13, k=0]
        let flat = [11, 1, 101, 12, 3, 201, 202, 203, 13, 0];
        let groups = decode_groups(&flat).unwrap();
        assert_eq!(groups.len(), 3, "应解出 3 组");
        assert_eq!(
            groups[0],
            FaceEvolution {
                input_hash: 11,
                outputs: vec![101]
            }
        );
        assert_eq!(
            groups[1],
            FaceEvolution {
                input_hash: 12,
                outputs: vec![201, 202, 203]
            }
        );
        assert_eq!(
            groups[2],
            FaceEvolution {
                input_hash: 13,
                outputs: vec![]
            }
        );

        // 空表 → 空分组（合法，不是错误）
        assert!(decode_groups(&[]).unwrap().is_empty());

        // 坏边界 ①：声明 k=3 但只有 2 个输出 → 越界，必须 Err（不许静默截断）
        assert!(
            matches!(
                decode_groups(&[11, 3, 101, 102]),
                Err(KernelError::Geometry { .. })
            ),
            "组边界外溢必须报 Geometry 错误"
        );
        // 坏边界 ②：负 k → Err
        assert!(
            matches!(
                decode_groups(&[11, -1, 101]),
                Err(KernelError::Geometry { .. })
            ),
            "负 k 必须报 Geometry 错误"
        );
        // 坏边界 ③：组头残缺（只剩一个元素）→ Err
        assert!(
            matches!(decode_groups(&[11]), Err(KernelError::Geometry { .. })),
            "组头残缺必须报 Geometry 错误"
        );
    }

    /// `outputs_of` 对**未受影响**的输入面返回**空 Vec**（区分于"调用失败"）。
    #[test]
    fn evolution_outputs_of_is_empty_not_error_when_unaffected() {
        let evo = Evolution {
            result: ShapeHandle(1),
            modified: vec![FaceEvolution {
                input_hash: 11,
                outputs: vec![101, 102],
            }],
            generated: vec![FaceEvolution {
                input_hash: 12,
                outputs: vec![201],
            }],
            deleted: vec![99],
        };
        assert_eq!(evo.outputs_of(11), vec![101, 102]);
        assert_eq!(evo.outputs_of(12), vec![201]);
        // 未受影响的输入面 → 空 Vec（不是 Err）
        assert!(evo.outputs_of(999).is_empty());
        // 落在 deleted 里的输入面同样是空（已删，无输出）
        assert!(evo.outputs_of(99).is_empty());
    }

    /// `sources_of` 能反查来源：modified 与 generated 都参与反查。
    #[test]
    fn evolution_sources_of_reverses_modified_and_generated() {
        let evo = Evolution {
            result: ShapeHandle(1),
            modified: vec![FaceEvolution {
                input_hash: 11,
                outputs: vec![101],
            }],
            generated: vec![FaceEvolution {
                input_hash: 12,
                outputs: vec![101, 202],
            }],
            deleted: vec![],
        };
        // 输出 101 同时来自 modified 的 11 与 generated 的 12
        assert_eq!(evo.sources_of(101), vec![11, 12]);
        // 202 只来自 generated 的 12
        assert_eq!(evo.sources_of(202), vec![12]);
        // 无来源 → 空
        assert!(evo.sources_of(777).is_empty());
    }
}
