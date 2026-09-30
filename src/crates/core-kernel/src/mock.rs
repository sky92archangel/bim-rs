//! 纯 Rust 的占位后端（无 WASM 环境的开发与测试用）。
//!
//! **它不精确**——几何是简化的 bbox / 规则网格，投影是解析解的理想化结果。
//! 它的价值在于：
//! 1. 让 `core-doc` / `core-app` / `modules/*` 在无 WASM 环境下也能写与测；
//! 2. 用一个**确定性后端**固定住上层对内核抽象的假设（接口错了能早发现）。
//!
//! ## ⚠ 它的 BREP 文本**不是** OCCT BREP
//!
//! [`MockBackend::export_brep`] 输出的是一种**自描述占位格式**（一行文本，
//! 形如 `MOCK-BREP v3 box 10 20 30 6000 1000 0 3000 0 0 0 1`）。它的唯一用途是让
//! "执行配方 → 导出 → 重新装载 → 缓存回收"整条链在**没有 WASM 时也能被测通**。
//!
//! ## v2：占位几何**如实记账**定位点
//!
//! v1 只记 `种类 + 尺寸 + 体积`，`translate` 出来的新形状与原件**文本逐字节相同**
//! ——那是"静默成功"：平移看起来做了，存盘后却凭空消失。v2 起 [`Shape`] 增记
//! `origin: [f64; 3]`（相对创建点的累积偏移，mm），于是：
//!
//! - `translate` **累加**偏移进 `origin`，导出文本随之改变（存盘不丢）；
//! - [`from_text`](Shape::from_text) **同时接受 v1 与 v2**：v1 文本一律视作
//!   `origin = [0, 0, 0]`，保证旧文档仍可读回；
//! - `tessellate` / `project_edges` **把 `origin` 纳入计算**（见各自实现注释），
//!   真正做到"位置变了，几何就跟着挪"；`volume` 是平移不变量，不受 `origin` 影响。
//!
//! ## v3：占位几何**如实记账**旋转与子实体数
//!
//! 与 v2 同源的理由：`rotate` 若只校验参数、返回一份**不改文本的副本**，就是
//! v2 `translate` 坑的复刻（"旋转"能过门禁、能在演示里看，**存盘后凭空消失**）。
//! 因此 v3 起 [`Shape`] 再增两个字段，文本升到 **v3**（末尾各追加三段 / 一段）：
//!
//! - `rotation: [f64; 3]`——累积旋转向量（axis·angle，rad）。**占位口径**：
//!   按旋转向量**线性累加**（不做真四元数复合），仅供姿态存证与"文本必须随旋转变"；
//! - `solid_count: u32`——复合体里的**实体个数**（`fuse`/`cut`/`common` 按 AABB
//!   是否重叠如实记账）。**必须进文本**，否则"一次布尔产出多个不相连体"这条状态
//!   会在存盘后丢失（又一个"静默成功"）。
//!
//! [`from_text`](Shape::from_text) **同时接受 v1 / v2 / v3**：v1/v2 一律视作
//! `rotation = [0,0,0]` 且 `solid_count = 1`。
//!
//! 真实后端（[`OcctBackend`](crate::OcctBackend)）输出的是货真价实的
//! `OCCT BREP` 文本，能跨软件交换、能进 `.bimdoc` 的 `geom/*.brep`。
//! **两者格式不互通**，`Mock` 写出的文档别指望真实后端能读回来。
//! 这一点由 [`BackendKind::Mock`] 显式标出，内核池也会据此拒绝静默降级。

use crate::arena::SlotArena;
use crate::backend::{BackendKind, KernelBackend};
use crate::error::{KernelError, KernelResult};
use crate::handle::ShapeHandle;
use crate::types::{
    BoundingBox, CurveGeometry, CurveKind, CurveRange, DVec2, DVec3, Evolution, FaceGroup,
    LineClass, MeshData, PointOnEdge, PointOnFace, ProjectedCurve, Projection, SubShapeKind,
    ViewDef,
};

/// 占位几何的持久化格式魔数（见模块文档"它的 BREP 文本不是 OCCT BREP"）。
const MOCK_BREP_MAGIC: &str = "MOCK-BREP";
/// 占位几何的持久化格式版本。
///
/// - v1 → v2：末尾追加三段 `origin`（定位点）；
/// - v2 → v3：末尾再追加三段 `rotation`（姿态）与一段 `solid_count`（子实体数）。
///
/// `from_text` 仍接受 v1 / v2（分别按 `origin`/`rotation` 归零、`solid_count = 1`
/// 读回），因此升级不会让旧文档失效。
const MOCK_BREP_VERSION: u32 = 3;

/// 一个占位形状：用包围盒尺寸、种类、体积、**定位点**、**姿态**与**子实体数**描述。
#[derive(Debug, Clone, Copy)]
struct Shape {
    kind: ShapeKind,
    /// 包围盒尺寸（mm）。box 约定一角在原点；其余以原点为中心。
    size: [f64; 3],
    /// 体积（mm³）——**占位估算**，只保证确定性，不保证是精确几何结果。
    volume: f64,
    /// 相对创建点的**累积平移量**（mm）——`translate` 的成果记在这里，
    /// 并随 BREP 文本一起存盘（v2 起）。平移不改变 `size` / `volume`。
    origin: [f64; 3],
    /// **累积旋转向量**（axis·angle，rad）——`rotate` 的姿态记录（v3 起）。
    /// 占位口径：按旋转向量线性累加，不做真四元数复合（见模块文档）。
    /// 旋转已**折进** `size`/`origin`（转 8 角点取 AABB），此处只为文本存证。
    rotation: [f64; 3],
    /// 复合体里的**实体个数**（v3 起）：普通形状为 1；布尔结果按 AABB 重叠记账。
    solid_count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShapeKind {
    Box,
    Sphere,
    Cylinder,
    Cone,
    Torus,
    /// 拉伸体（轮廓 → 体）
    Extrude,
    /// 旋转体（轮廓 → 体）
    Revolve,
    /// 复合体（布尔结果等）
    Compound,
    /// **壳**（`sew` 缝合的产出；`solid_count = 0`，见 [`MockBackend::sew`]）。
    ///
    /// v3 文本兼容：它用既有的 `solid_count` 字段如实记 0，故**不升版本**。
    Shell,
    /// **直边占位**（M7a）：**记录型**——`origin` 记第 1 端点，`size` 记
    /// 第 2 端点（⚠ 对线种类，`size` 携带**端点坐标**而非包围盒尺寸）。
    EdgeLine,
    /// **三点弧占位**（M7a）：**记录型**——`origin` / `size` / `rotation`
    /// 依次记起点 / 中间点 / 终点（9 坐标折进既有字段，v3 格式不变）。
    EdgeArc,
    /// **线框占位**（M7a）：extents（成员边记录点的 min/max）折进 `origin` /
    /// `size`，**边数**记在 `rotation[0]`。
    Wire,
    /// **面占位**（M7a）：由 wire 造出，extents / 边数抄自输入 wire。
    Face,
}

impl ShapeKind {
    /// 持久化用的短名（小写，见 [`MOCK_BREP_MAGIC`]）。
    const fn tag(self) -> &'static str {
        match self {
            Self::Box => "box",
            Self::Sphere => "sphere",
            Self::Cylinder => "cylinder",
            Self::Cone => "cone",
            Self::Torus => "torus",
            Self::Extrude => "extrude",
            Self::Revolve => "revolve",
            Self::Compound => "compound",
            Self::Shell => "shell",
            Self::EdgeLine => "edge_line",
            Self::EdgeArc => "edge_arc",
            Self::Wire => "wire",
            Self::Face => "face",
        }
    }

    /// 从短名还原。
    fn from_tag(tag: &str) -> Option<Self> {
        match tag {
            "box" => Some(Self::Box),
            "sphere" => Some(Self::Sphere),
            "cylinder" => Some(Self::Cylinder),
            "cone" => Some(Self::Cone),
            "torus" => Some(Self::Torus),
            "extrude" => Some(Self::Extrude),
            "revolve" => Some(Self::Revolve),
            "compound" => Some(Self::Compound),
            "shell" => Some(Self::Shell),
            "edge_line" => Some(Self::EdgeLine),
            "edge_arc" => Some(Self::EdgeArc),
            "wire" => Some(Self::Wire),
            "face" => Some(Self::Face),
            _ => None,
        }
    }
}

impl Shape {
    fn new(kind: ShapeKind, size: [f64; 3], volume: f64) -> Self {
        Self {
            kind,
            size,
            volume,
            origin: [0.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0],
            solid_count: 1,
        }
    }

    /// 世界轴对齐包围盒的**最小角点**（占位口径：所有种类一律 `origin`）。
    fn aabb_min(&self) -> [f64; 3] {
        self.origin
    }

    /// 世界轴对齐包围盒的**最大角点**（占位口径：一律 `origin + size`）。
    fn aabb_max(&self) -> [f64; 3] {
        [
            self.origin[0] + self.size[0],
            self.origin[1] + self.size[1],
            self.origin[2] + self.size[2],
        ]
    }

    /// 世界 AABB 是否与另一个形状重叠（含相切）。
    fn aabb_overlaps(&self, other: &Self) -> bool {
        let (a_min, a_max) = (self.aabb_min(), self.aabb_max());
        let (b_min, b_max) = (other.aabb_min(), other.aabb_max());
        (0..3).all(|i| a_min[i] <= b_max[i] && b_min[i] <= a_max[i])
    }

    /// 序列化成一行自描述文本（v3：末尾追加 `rotation`(3) 与 `solid_count`(1)）。
    fn to_text(self) -> String {
        format!(
            "{MOCK_BREP_MAGIC} v{MOCK_BREP_VERSION} {} {} {} {} {} {} {} {} {} {} {} {}",
            self.kind.tag(),
            self.size[0],
            self.size[1],
            self.size[2],
            self.volume,
            self.origin[0],
            self.origin[1],
            self.origin[2],
            self.rotation[0],
            self.rotation[1],
            self.rotation[2],
            self.solid_count
        )
    }

    /// 从一行自描述文本还原（**同时接受 v1 / v2 / v3**）。
    ///
    /// - `v1`：`种类 + 尺寸(3) + 体积(1)` → `origin/rotation = 0`、`solid_count = 1`；
    /// - `v2`：`种类 + 尺寸(3) + 体积(1) + origin(3)` → `rotation = 0`、`solid_count = 1`；
    /// - `v3`：`种类 + 尺寸(3) + 体积(1) + origin(3) + rotation(3) + solid_count(1)`。
    ///
    /// 其它版本、参数个数不符、出现非数字 token、或出现**非有限数值**
    /// （`NaN` / `±inf`）一律判为格式错误——**绝不**静默补零吞掉坏输入。
    fn from_text(text: &str) -> KernelResult<Self> {
        let malformed = || KernelError::Geometry {
            op: "from_brep",
            msg: format!("占位几何文本格式不正确：{text:?}"),
        };
        let mut parts = text.split_whitespace();
        let magic = parts.next().ok_or_else(malformed)?;
        let version = parts.next().ok_or_else(malformed)?;
        let kind = parts.next().ok_or_else(malformed)?;

        if magic != MOCK_BREP_MAGIC {
            return Err(malformed());
        }
        let kind = ShapeKind::from_tag(kind).ok_or_else(malformed)?;

        // 严格解析：任一 token 不是合法 f64 即判错（不再 filter_map 静默丢弃）。
        let mut nums: Vec<f64> = Vec::new();
        for token in parts {
            nums.push(token.parse::<f64>().map_err(|_| malformed())?);
        }

        let (size, volume, origin, rotation, solid_count) = match (version, nums.len()) {
            ("v1", 4) => (
                [nums[0], nums[1], nums[2]],
                nums[3],
                [0.0, 0.0, 0.0],
                [0.0, 0.0, 0.0],
                1.0,
            ),
            ("v2", 7) => (
                [nums[0], nums[1], nums[2]],
                nums[3],
                [nums[4], nums[5], nums[6]],
                [0.0, 0.0, 0.0],
                1.0,
            ),
            ("v3", 11) => (
                [nums[0], nums[1], nums[2]],
                nums[3],
                [nums[4], nums[5], nums[6]],
                [nums[7], nums[8], nums[9]],
                nums[10],
            ),
            _ => return Err(malformed()),
        };

        // 有限性校验：`NaN` / `±inf` 都是**合法** f64 token，
        // 必须**显式**用 `is_finite()` 拦下，否则会把非有限几何静默注入文档。
        // 注意：**不能**靠 `> 0.0` 之类的比较兜底——`NaN` 参与任何比较恒为 false，
        // 会从比较里漏过；只有 `is_finite()` 能确定性拦住它。
        let finite = |field: &str, v: f64| -> KernelResult<f64> {
            if v.is_finite() {
                Ok(v)
            } else {
                Err(KernelError::Geometry {
                    op: "from_brep",
                    msg: format!("占位几何文本含非有限数值：{field} = {v}（文本 {text:?}）"),
                })
            }
        };
        let size = [
            finite("size.x", size[0])?,
            finite("size.y", size[1])?,
            finite("size.z", size[2])?,
        ];
        let volume = finite("volume", volume)?;
        let origin = [
            finite("origin.x", origin[0])?,
            finite("origin.y", origin[1])?,
            finite("origin.z", origin[2])?,
        ];
        // rotation 三分量同样做 is_finite 校验（v3 新增字段，绝不漏拦）。
        let rotation = [
            finite("rotation.x", rotation[0])?,
            finite("rotation.y", rotation[1])?,
            finite("rotation.z", rotation[2])?,
        ];
        // solid_count 必须是**非负整数**且落在 u32 内；小数 / 负数 / 非有限一律拒。
        let solid_count_f = finite("solid_count", solid_count)?;
        if solid_count_f < 0.0
            || solid_count_f.fract() != 0.0
            || solid_count_f > f64::from(u32::MAX)
        {
            return Err(malformed());
        }
        let solid_count = solid_count_f as u32;

        Ok(Self {
            kind,
            size,
            volume,
            origin,
            rotation,
            solid_count,
        })
    }
}

/// 纯 Rust 占位后端。
#[derive(Debug, Default)]
pub struct MockBackend {
    /// 槽位表（与真实后端共用同一份 [`SlotArena`]，语义必须一致）
    arena: SlotArena<Shape>,
}

fn positive(op: &'static str, name: &str, v: f64) -> KernelResult<f64> {
    if !v.is_finite() || v <= 0.0 {
        return Err(KernelError::InvalidArgument(format!(
            "{op}: {name} 必须为有限正数，实得 {v}"
        )));
    }
    Ok(v)
}

/// 校验"有限非负数"（允许 0，如圆锥尖顶半径 `r2 = 0`）。
fn non_negative(op: &'static str, name: &str, v: f64) -> KernelResult<f64> {
    if !v.is_finite() || v < 0.0 {
        return Err(KernelError::InvalidArgument(format!(
            "{op}: {name} 必须为有限非负数，实得 {v}"
        )));
    }
    Ok(v)
}

/// 校验"有限数"（可正可负、可为 0）。
fn finite_arg(op: &'static str, name: &str, v: f64) -> KernelResult<f64> {
    if !v.is_finite() {
        return Err(KernelError::InvalidArgument(format!(
            "{op}: {name} 必须为有限数，实得 {v}"
        )));
    }
    Ok(v)
}

/// 校验并**单位化**旋转/回转轴：三分量有限、非零向量。
///
/// 返回单位轴向量的三分量 `(ux, uy, uz)`。
fn normalize_axis(op: &'static str, dx: f64, dy: f64, dz: f64) -> KernelResult<(f64, f64, f64)> {
    let (dx, dy, dz) = (
        finite_arg(op, "dx", dx)?,
        finite_arg(op, "dy", dy)?,
        finite_arg(op, "dz", dz)?,
    );
    let len = (dx * dx + dy * dy + dz * dz).sqrt();
    if len == 0.0 {
        return Err(KernelError::InvalidArgument(format!(
            "{op}: 旋转轴不能为零向量（dx=dy=dz=0）"
        )));
    }
    Ok((dx / len, dy / len, dz / len))
}

/// 把点 `p` 绕**过点 `pivot`、方向 `axis`（单位化后）、角 `angle`（rad）**的轴旋转
/// （Rodrigues 公式）。占位后端用它把 AABB 的 8 个角点转过去再取新 AABB。
fn rotate_point(p: [f64; 3], pivot: [f64; 3], axis: [f64; 3], angle: f64) -> [f64; 3] {
    let (s, c) = angle.sin_cos();
    let (ux, uy, uz) = (axis[0], axis[1], axis[2]);
    let v = [p[0] - pivot[0], p[1] - pivot[1], p[2] - pivot[2]];
    // Rodrigues: v' = v·cosθ + (u×v)·sinθ + u·(u·v)·(1−cosθ)
    let dot = ux * v[0] + uy * v[1] + uz * v[2];
    let cross = [
        uy * v[2] - uz * v[1],
        uz * v[0] - ux * v[2],
        ux * v[1] - uy * v[0],
    ];
    let k = dot * (1.0 - c);
    [
        pivot[0] + v[0] * c + cross[0] * s + ux * k,
        pivot[1] + v[1] * c + cross[1] * s + uy * k,
        pivot[2] + v[2] * c + cross[2] * s + uz * k,
    ]
}

impl MockBackend {
    /// 新建一个空后端。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 取形状的只读副本（占位形状是 `Copy`）。
    fn get(&self, h: ShapeHandle) -> KernelResult<Shape> {
        self.arena.get(h).copied()
    }

    /// 两个形状都存活，才允许布尔。
    fn both(&self, a: ShapeHandle, b: ShapeHandle) -> KernelResult<(Shape, Shape)> {
        Ok((self.get(a)?, self.get(b)?))
    }
}

/// 把线段加入投影列表。
fn push_line(out: &mut Vec<ProjectedCurve>, a: DVec2, b: DVec2, class: LineClass) {
    out.push(ProjectedCurve {
        kind: CurveKind::Line,
        data: CurveGeometry::Line { a, b },
        layer_hint: class,
    });
}

/// 生成矩形轮廓的四条线（用于 box / cylinder 的占位投影）。
///
/// `offset` 是投影平面内的定位点偏移（`origin` 的 x/y 分量，见 `project_edges`）。
fn rect_lines(out: &mut Vec<ProjectedCurve>, w: f64, h: f64, class: LineClass, offset: DVec2) {
    let (x0, y0) = (offset.x, offset.y);
    let (x1, y1) = (offset.x + w, offset.y + h);
    push_line(out, DVec2::new(x0, y0), DVec2::new(x1, y0), class);
    push_line(out, DVec2::new(x1, y0), DVec2::new(x1, y1), class);
    push_line(out, DVec2::new(x1, y1), DVec2::new(x0, y1), class);
    push_line(out, DVec2::new(x0, y1), DVec2::new(x0, y0), class);
}

/// 长方体的规则网格：8 顶点 / 12 三角形 / 6 个面。
fn box_mesh(dx: f64, dy: f64, dz: f64) -> MeshData {
    let positions = vec![
        [0.0, 0.0, 0.0],
        [dx, 0.0, 0.0],
        [dx, dy, 0.0],
        [0.0, dy, 0.0],
        [0.0, 0.0, dz],
        [dx, 0.0, dz],
        [dx, dy, dz],
        [0.0, dy, dz],
    ];
    let indices = vec![
        0, 2, 1, 0, 3, 2, // 底
        4, 5, 6, 4, 6, 7, // 顶
        0, 1, 5, 0, 5, 4, // Y-
        3, 7, 6, 3, 6, 2, // Y+
        0, 4, 7, 0, 7, 3, // X-
        1, 2, 6, 1, 6, 5, // X+
    ];
    let face_groups = (0..6)
        .map(|i| FaceGroup {
            tri_start: i * 2,
            tri_count: 2,
            face_hash: i as u64 + 1,
        })
        .collect();
    MeshData {
        positions,
        indices,
        normals: Vec::new(),
        face_groups,
    }
}

/// UV 球网格（`segments × rings`）。
fn sphere_mesh(r: f64) -> MeshData {
    const SEGMENTS: u32 = 16;
    const RINGS: u32 = 8;
    let mut positions = Vec::new();
    for i in 0..=RINGS {
        let theta = std::f64::consts::PI * f64::from(i) / f64::from(RINGS);
        let (st, ct) = theta.sin_cos();
        for j in 0..=SEGMENTS {
            let phi = std::f64::consts::TAU * f64::from(j) / f64::from(SEGMENTS);
            let (sp, cp) = phi.sin_cos();
            positions.push([r * st * cp, r * st * sp, r * ct]);
        }
    }
    let mut indices = Vec::new();
    let stride = SEGMENTS + 1;
    for i in 0..RINGS {
        for j in 0..SEGMENTS {
            let a = i * stride + j;
            let b = a + stride;
            indices.extend_from_slice(&[a, b, a + 1, a + 1, b, b + 1]);
        }
    }
    MeshData {
        positions,
        indices,
        normals: Vec::new(),
        face_groups: vec![FaceGroup {
            tri_start: 0,
            tri_count: (RINGS * SEGMENTS) * 2,
            face_hash: 1,
        }],
    }
}

/// 圆柱网格（侧面 + 上下端面扇形）。
fn cylinder_mesh(r: f64, h: f64) -> MeshData {
    const SIDES: u32 = 16;
    let mut positions = Vec::new();
    let mut indices = Vec::new();
    for j in 0..=SIDES {
        let phi = std::f64::consts::TAU * f64::from(j) / f64::from(SIDES);
        let (sp, cp) = phi.sin_cos();
        positions.push([r * cp, r * sp, 0.0]);
        positions.push([r * cp, r * sp, h]);
    }
    // 侧面
    for j in 0..SIDES {
        let a = j * 2;
        let b = a + 1;
        let c = a + 2;
        let d = a + 3;
        indices.extend_from_slice(&[a, c, b, b, c, d]);
    }
    // 端面（以中心点做扇形）
    let bottom = positions.len() as u32;
    positions.push([0.0, 0.0, 0.0]);
    let top = positions.len() as u32;
    positions.push([0.0, 0.0, h]);
    for j in 0..SIDES {
        let a = j * 2;
        let c = a + 2;
        indices.extend_from_slice(&[bottom, a, c]);
        indices.extend_from_slice(&[top, c + 1, a + 1]);
    }
    let tri_count = indices_len(&indices) as u32;
    MeshData {
        positions,
        indices,
        normals: Vec::new(),
        face_groups: vec![FaceGroup {
            tri_start: 0,
            tri_count,
            face_hash: 1,
        }],
    }
}

fn indices_len(v: &[u32]) -> usize {
    v.len() / 3
}

/// 线 / 线框占位网格：没有可细分的表面，返回空网格（0 顶点 / 0 三角形）。
fn empty_mesh() -> MeshData {
    MeshData {
        positions: Vec::new(),
        indices: Vec::new(),
        normals: Vec::new(),
        face_groups: Vec::new(),
    }
}

/// 线 / 面占位种类的**已记录坐标点**（供 `make_wire` 求 extents；其余种类无点集语义）。
fn member_points(s: &Shape) -> Vec<[f64; 3]> {
    match s.kind {
        ShapeKind::EdgeLine => vec![s.origin, s.size],
        ShapeKind::EdgeArc => vec![s.origin, s.size, s.rotation],
        // Wire / Face 的 extents 已折进 origin/size，取其两角点
        ShapeKind::Wire | ShapeKind::Face => vec![s.origin, s.aabb_max()],
        _ => Vec::new(),
    }
}

impl KernelBackend for MockBackend {
    fn name(&self) -> &'static str {
        "mock"
    }

    fn kind(&self) -> BackendKind {
        // 明确声明"我不精确"——内核池据此拒绝把本后端当成真实内核使用。
        BackendKind::Mock
    }

    fn make_box(&mut self, dx: f64, dy: f64, dz: f64) -> KernelResult<ShapeHandle> {
        let size = [
            positive("make_box", "dx", dx)?,
            positive("make_box", "dy", dy)?,
            positive("make_box", "dz", dz)?,
        ];
        Ok(self.arena.alloc(Shape::new(
            ShapeKind::Box,
            size,
            size[0] * size[1] * size[2],
        )))
    }

    fn make_sphere(&mut self, r: f64) -> KernelResult<ShapeHandle> {
        let r = positive("make_sphere", "r", r)?;
        let volume = 4.0 / 3.0 * std::f64::consts::PI * r * r * r;
        Ok(self.arena.alloc(Shape::new(
            ShapeKind::Sphere,
            [2.0 * r, 2.0 * r, 2.0 * r],
            volume,
        )))
    }

    fn make_cylinder(&mut self, r: f64, h: f64) -> KernelResult<ShapeHandle> {
        let r = positive("make_cylinder", "r", r)?;
        let h = positive("make_cylinder", "h", h)?;
        let volume = std::f64::consts::PI * r * r * h;
        Ok(self.arena.alloc(Shape::new(
            ShapeKind::Cylinder,
            [2.0 * r, 2.0 * r, h],
            volume,
        )))
    }

    fn make_cone(&mut self, r1: f64, r2: f64, h: f64) -> KernelResult<ShapeHandle> {
        // 半径允许为 0（尖顶）；只有负半径才非法——这正好让"非法参数"路径可用
        // （见 geometry_service 的 Rotate 钉子改造：`MakeCone(r1 = -1)` 触发
        // `InvalidArgument`，走真实内核错误路径）。
        let r1 = non_negative("make_cone", "r1", r1)?;
        let r2 = non_negative("make_cone", "r2", r2)?;
        let h = positive("make_cone", "h", h)?;
        let rmax = r1.max(r2);
        // 圆台/圆锥通用体积公式：πh/3·(r1² + r1·r2 + r2²)
        let volume = std::f64::consts::PI * h / 3.0 * (r1 * r1 + r1 * r2 + r2 * r2);
        Ok(self.arena.alloc(Shape::new(
            ShapeKind::Cone,
            [2.0 * rmax, 2.0 * rmax, h],
            volume,
        )))
    }

    fn make_torus(&mut self, major: f64, minor: f64) -> KernelResult<ShapeHandle> {
        let major = positive("make_torus", "major", major)?;
        let minor = positive("make_torus", "minor", minor)?;
        // 环面体积：2π²·R·r²
        let volume = 2.0 * std::f64::consts::PI * std::f64::consts::PI * major * minor * minor;
        let d = 2.0 * (major + minor);
        Ok(self
            .arena
            .alloc(Shape::new(ShapeKind::Torus, [d, d, d], volume)))
    }

    fn extrude(
        &mut self,
        shape: ShapeHandle,
        dx: f64,
        dy: f64,
        dz: f64,
    ) -> KernelResult<ShapeHandle> {
        let s = self.get(shape)?;
        let delta = [
            finite_arg("extrude", "dx", dx)?,
            finite_arg("extrude", "dy", dy)?,
            finite_arg("extrude", "dz", dz)?,
        ];
        // 沿拉伸向"长大"：新尺寸 = 原尺寸 + |增量|；定位点沿用输入。
        let size = [
            s.size[0].abs() + delta[0].abs(),
            s.size[1].abs() + delta[1].abs(),
            s.size[2].abs() + delta[2].abs(),
        ];
        let volume = size[0] * size[1] * size[2];
        let mut out = Shape::new(ShapeKind::Extrude, size, volume);
        out.origin = s.origin;
        Ok(self.arena.alloc(out))
    }

    #[allow(clippy::too_many_arguments)]
    fn revolve(
        &mut self,
        shape: ShapeHandle,
        px: f64,
        py: f64,
        pz: f64,
        dx: f64,
        dy: f64,
        dz: f64,
        angle_rad: f64,
    ) -> KernelResult<ShapeHandle> {
        let s = self.get(shape)?;
        for (name, v) in [("px", px), ("py", py), ("pz", pz)] {
            finite_arg("revolve", name, v)?;
        }
        let axis = normalize_axis("revolve", dx, dy, dz)?;
        let angle = finite_arg("revolve", "angle_rad", angle_rad)?;
        // 占位口径：回转半径取轮廓较大半跨，体绕轴扫 angle 弧度。
        let r = s.size[0].max(s.size[1]) / 2.0;
        let size = [2.0 * r, 2.0 * r, s.size[2]];
        let volume =
            std::f64::consts::PI * r * r * s.size[2] * (angle.abs() / std::f64::consts::TAU);
        let mut out = Shape::new(ShapeKind::Revolve, size, volume);
        out.origin = s.origin;
        let _ = axis;
        Ok(self.arena.alloc(out))
    }

    /// 占位旋转（v3）。
    ///
    /// ⚠ **占位口径，勿做几何断言**：
    /// - `rotation` 字段是按旋转向量**线性累加**的**姿态存证**，**不是**真实的旋转矩阵组合；
    /// - AABB 是把当前世界包围盒的 8 个角点**真转**后取 min/max —— 所以 `tessellate` /
    ///   `project_edges` / `bounding_box` 会如实反映旋转；
    /// - **浮点非幂等**：转 `2π` 后 `size.x` 会是 `10.000000000000005` 这类值，转 `90°` 再
    ///   `-90°` 后 `origin.x` 会是 `-1.2e-15` 这类值，**文本不保证逐字节还原**。因此
    ///   **不要对 `rotate` 的结果做逐字节幂等断言**（如 `rotate(2π) ≡ 原形状`）。
    /// - 真实几何精度以 **OCCT 内核腿（`--features occt`）** 为准。
    #[allow(clippy::too_many_arguments)]
    fn rotate(
        &mut self,
        shape: ShapeHandle,
        px: f64,
        py: f64,
        pz: f64,
        dx: f64,
        dy: f64,
        dz: f64,
        angle_rad: f64,
    ) -> KernelResult<ShapeHandle> {
        let s = self.get(shape)?;
        for (name, v) in [("px", px), ("py", py), ("pz", pz)] {
            finite_arg("rotate", name, v)?;
        }
        let axis = normalize_axis("rotate", dx, dy, dz)?;
        let angle = finite_arg("rotate", "angle_rad", angle_rad)?;

        // **姿态必须进文本**（v3）：按旋转向量线性累加（占位口径，见模块文档）。
        let rv = [axis.0 * angle, axis.1 * angle, axis.2 * angle];

        // **几何必须跟着转**：把当前世界 AABB 的 8 个角点绕轴旋转，取新 min/max。
        // 这样 tessellate / project_edges / bounding_box 都如实反映旋转，
        // 绝不出现"旋转能过门禁、存盘后凭空消失"。
        let pivot = [px, py, pz];
        let unit = [axis.0, axis.1, axis.2];
        let (mut min, mut max) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
        let lo = s.aabb_min();
        let hi = s.aabb_max();
        for i in 0..8 {
            let corner = [
                if i & 1 == 0 { lo[0] } else { hi[0] },
                if i & 2 == 0 { lo[1] } else { hi[1] },
                if i & 4 == 0 { lo[2] } else { hi[2] },
            ];
            let p = rotate_point(corner, pivot, unit, angle);
            for k in 0..3 {
                min[k] = min[k].min(p[k]);
                max[k] = max[k].max(p[k]);
            }
        }
        let mut out = s;
        out.origin = min;
        out.size = [max[0] - min[0], max[1] - min[1], max[2] - min[2]];
        // rotation 是姿态存证（累积旋转向量）。
        for (dst, add) in out.rotation.iter_mut().zip(rv.iter()) {
            *dst += *add;
        }
        Ok(self.arena.alloc(out))
    }

    fn bounding_box(&mut self, shape: ShapeHandle) -> KernelResult<BoundingBox> {
        let s = self.get(shape)?;
        Ok(BoundingBox::new(s.aabb_min(), s.aabb_max()))
    }

    fn surface_area(&mut self, shape: ShapeHandle) -> KernelResult<f64> {
        let s = self.get(shape)?;
        // 占位口径：把形状当作尺寸的盒，表面积 = 2·(sx·sy + sy·sz + sx·sz)。
        let [a, b, c] = s.size;
        Ok(2.0 * (a * b + b * c + a * c))
    }

    fn sub_shapes(
        &mut self,
        shape: ShapeHandle,
        kind: SubShapeKind,
    ) -> KernelResult<Vec<ShapeHandle>> {
        let s = self.get(shape)?;
        match kind {
            // 复合体 / 壳：按 solid_count 拆出 n 个"单实体副本"（各记 solid_count = 1）。
            // 普通形状：返回 1 个自身副本。
            // ⚠ 壳（`Shell`）的 `solid_count = 0`，因此这里拆出**空集**——这正是
            // "`sew` 不成实体"这条语义在 mock 腿上的可观测形态。
            SubShapeKind::Solid => {
                let n = if matches!(
                    s.kind,
                    ShapeKind::Compound
                        | ShapeKind::Shell
                        | ShapeKind::EdgeLine
                        | ShapeKind::EdgeArc
                        | ShapeKind::Wire
                        | ShapeKind::Face
                ) {
                    s.solid_count
                } else {
                    1
                };
                let mut out = Vec::with_capacity(n as usize);
                for _ in 0..n {
                    let mut one = s;
                    one.solid_count = 1;
                    out.push(self.arena.alloc(one));
                }
                Ok(out)
            }
            // 占位后端没有拓扑：其余子形状种类一律返回空（**已在文档中写明**，
            // 绝不让上层误以为"有 12 条边"）。
            SubShapeKind::Shell
            | SubShapeKind::Face
            | SubShapeKind::Edge
            | SubShapeKind::Vertex
            | SubShapeKind::Wire => Ok(Vec::new()),
        }
    }

    fn sub_shape_count(&mut self, shape: ShapeHandle, kind: SubShapeKind) -> KernelResult<usize> {
        let s = self.get(shape)?;
        Ok(match kind {
            SubShapeKind::Solid => {
                if matches!(
                    s.kind,
                    ShapeKind::Compound
                        | ShapeKind::Shell
                        | ShapeKind::EdgeLine
                        | ShapeKind::EdgeArc
                        | ShapeKind::Wire
                        | ShapeKind::Face
                ) {
                    s.solid_count as usize
                } else {
                    1
                }
            }
            _ => 0,
        })
    }

    fn fuse(&mut self, a: ShapeHandle, b: ShapeHandle) -> KernelResult<ShapeHandle> {
        let (sa, sb) = self.both(a, b)?;
        // 占位估算：并集体积 ≤ 两者之和（真几何里重叠部分会被扣掉）
        let volume = sa.volume + sb.volume;
        // **复合体如实记账**：用 AABB 真判两输入是否重叠——
        // 不相连 → 并集含各体之和个实体；相连 → 取 max（融合成更少/同等个实体）。
        // 不真判的话，"一次布尔产出多个不相连体"这条钉子在 mock 腿会永远红，
        // 极易被人把断言改成 `== 1`，从而把 M3 唯一的复合体验证悄悄废掉。
        let solid_count = if sa.aabb_overlaps(&sb) {
            sa.solid_count.max(sb.solid_count)
        } else {
            sa.solid_count + sb.solid_count
        };
        let mut out = Shape::new(
            ShapeKind::Compound,
            [
                sa.size[0].max(sb.size[0]),
                sa.size[1].max(sb.size[1]),
                sa.size[2].max(sb.size[2]),
            ],
            volume,
        );
        out.solid_count = solid_count;
        Ok(self.arena.alloc(out))
    }

    fn cut(&mut self, a: ShapeHandle, b: ShapeHandle) -> KernelResult<ShapeHandle> {
        let (sa, sb) = self.both(a, b)?;
        // 占位估算：差集体积介于 0 与主体体积之间
        let volume = (sa.volume - sb.volume).max(0.0);
        let mut out = Shape::new(ShapeKind::Compound, sa.size, volume);
        // 差集的实体数跟随主体 `a`（占位口径）。
        out.solid_count = sa.solid_count;
        Ok(self.arena.alloc(out))
    }

    fn common(&mut self, a: ShapeHandle, b: ShapeHandle) -> KernelResult<ShapeHandle> {
        let (sa, sb) = self.both(a, b)?;
        // 占位估算：交集体积 ≤ 两者之较小
        let volume = sa.volume.min(sb.volume);
        let mut out = Shape::new(
            ShapeKind::Compound,
            [
                sa.size[0].min(sb.size[0]),
                sa.size[1].min(sb.size[1]),
                sa.size[2].min(sb.size[2]),
            ],
            volume,
        );
        // 交集的实体数 = 两者较小（占位口径）。
        out.solid_count = sa.solid_count.min(sb.solid_count);
        Ok(self.arena.alloc(out))
    }

    // ---- 带历史布尔（M5-B3）：占位后端**响亮拒绝，不静默空转** -------------
    //
    // 带历史布尔（`EvolutionData`）是**真实内核能力**，占位后端没有拓扑、也没有
    // 面哈希演化可言。此处照本文件"未实现即响亮拒绝"的惯例返回
    // [`KernelError::Unsupported`]——**绝不**返回空 `Evolution`（那会让上层误以为
    // "本次操作没有面发生变化"，从而在 mock 腿假绿）。

    /// 占位后端不实现带历史布尔：响亮拒绝。
    fn fuse_with_history(
        &mut self,
        _a: ShapeHandle,
        _b: ShapeHandle,
        _input_face_hashes: &[u64],
    ) -> KernelResult<Evolution> {
        Err(KernelError::Unsupported(
            "MockBackend 不实现 fuse_with_history：带历史布尔仅为真实内核能力",
        ))
    }

    /// 占位后端不实现带历史布尔：响亮拒绝。
    fn cut_with_history(
        &mut self,
        _a: ShapeHandle,
        _b: ShapeHandle,
        _input_face_hashes: &[u64],
    ) -> KernelResult<Evolution> {
        Err(KernelError::Unsupported(
            "MockBackend 不实现 cut_with_history：带历史布尔仅为真实内核能力",
        ))
    }

    /// 占位后端不实现点投影：**响亮拒绝，绝不返回假点**。
    ///
    /// 点投影是**真实内核能力**（`occt-wasm` 的 `project_point_on_face`）。占位后端
    /// 没有真实曲面，返回任何"最近点"都是编造的，会让拾取在 mock 腿假绿。
    fn project_point_on_face(
        &mut self,
        _face: ShapeHandle,
        _point: DVec3,
    ) -> KernelResult<PointOnFace> {
        Err(KernelError::Unsupported(
            "MockBackend 不实现 project_point_on_face：点投影仅为真实内核能力",
        ))
    }

    /// 占位后端不实现点投影：**响亮拒绝，绝不返回假点**。
    fn project_point_on_edge(
        &mut self,
        _edge: ShapeHandle,
        _point: DVec3,
    ) -> KernelResult<PointOnEdge> {
        Err(KernelError::Unsupported(
            "MockBackend 不实现 project_point_on_edge：点投影仅为真实内核能力",
        ))
    }

    /// 占位后端不实现面法向：**响亮拒绝，绝不返回假法向**。
    ///
    /// `surface_normal` 是**真实内核能力**（`occt-wasm`）。占位后端没有真实曲面，
    /// 返回任何法向都是编造的，会让拾取在 mock 腿假绿。
    fn surface_normal(&mut self, _face: ShapeHandle, _u: f64, _v: f64) -> KernelResult<DVec3> {
        Err(KernelError::Unsupported(
            "MockBackend 不实现 surface_normal：面法向仅为真实内核能力",
        ))
    }

    /// 占位后端不实现曲线参数域：**响亮拒绝，绝不返回假区间**。
    ///
    /// `curve_parameters` 是**真实内核能力**（`occt-wasm`）。占位后端的曲线是简化的，
    /// 返回任何 `[first,last]` 都是编造的，会让边拾取在 mock 腿假绿。
    fn curve_parameters(&mut self, _curve: ShapeHandle) -> KernelResult<CurveRange> {
        Err(KernelError::Unsupported(
            "MockBackend 不实现 curve_parameters：曲线参数域仅为真实内核能力",
        ))
    }

    /// 占位后端不实现按参数取点：**响亮拒绝，绝不返回假点**。
    ///
    /// `curve_point_at_param` 是**真实内核能力**（`occt-wasm`）。占位后端没有真实
    /// 曲线，返回任何点都是编造的，会让边指纹在 mock 腿假绿。
    fn curve_point_at_param(&mut self, _edge: ShapeHandle, _param: f64) -> KernelResult<DVec3> {
        Err(KernelError::Unsupported(
            "MockBackend 不实现 curve_point_at_param：按参数取点仅为真实内核能力",
        ))
    }

    /// 占位后端不实现曲线长度：**响亮拒绝，绝不返回假长度**。
    ///
    /// `curve_length` 是**真实内核能力**（`occt-wasm`）。占位后端的曲线是简化的，
    /// 返回任何长度都是编造的，会让边指纹在 mock 腿假绿。
    fn curve_length(&mut self, _edge: ShapeHandle) -> KernelResult<f64> {
        Err(KernelError::Unsupported(
            "MockBackend 不实现 curve_length：曲线几何长度仅为真实内核能力",
        ))
    }

    // ---- 修复类算子（M4 heal ops）：诚实占位 --------------------------------
    //
    // ⚠ **mock 不保真几何**：这里**不做**任何真正的缝合 / 修复 / 同域合并，
    // 只按 mock 的记账风格产出"形状 + op 存证"。真正语义以
    // **OCCT 内核腿（`--features occt --release`）** 为准。
    //
    // 但 `sew` 与 `sew_and_solidify` **必须产出可区分的结果**——否则"缝合不成
    // 实体、缝合并实体化成一个实体"这条语义会在占位后端里塌成同一件事，
    // 让人误以为两者可互换。故：
    //   - `sew`               → [`ShapeKind::Shell`]，`solid_count = 0`（不是实体）；
    //   - `sew_and_solidify`  → [`ShapeKind::Compound`]，`solid_count = 1`（一个实体）。

    /// 占位"缝合（成壳）"：原样记成一个**壳**（`solid_count = 0`，体积 = 输入体积）。
    fn sew(&mut self, shape: ShapeHandle, tolerance: f64) -> KernelResult<ShapeHandle> {
        let _tol = non_negative("sew", "tolerance", tolerance)?;
        let s = self.get(shape)?;
        let mut out = Shape::new(ShapeKind::Shell, s.size, s.volume);
        out.origin = s.origin;
        out.rotation = s.rotation;
        out.solid_count = 0;
        Ok(self.arena.alloc(out))
    }

    /// 占位"缝合并实体化"：原样记成一个**含单实体的复合体**（`solid_count = 1`）。
    fn sew_and_solidify(
        &mut self,
        shape: ShapeHandle,
        tolerance: f64,
    ) -> KernelResult<ShapeHandle> {
        let _tol = non_negative("sew_and_solidify", "tolerance", tolerance)?;
        let s = self.get(shape)?;
        let mut out = Shape::new(ShapeKind::Compound, s.size, s.volume);
        out.origin = s.origin;
        out.rotation = s.rotation;
        out.solid_count = 1;
        Ok(self.arena.alloc(out))
    }

    /// 占位"修复形状"：原样返回一份副本（mock 没有可分类型的内核缺陷）。
    fn fix_shape(&mut self, shape: ShapeHandle) -> KernelResult<ShapeHandle> {
        let s = self.get(shape)?;
        Ok(self.arena.alloc(s))
    }

    /// 占位"同域合并"：原样返回一份副本（mock 没有"相邻共面"这类拓扑可分）。
    fn unify_same_domain(&mut self, shape: ShapeHandle) -> KernelResult<ShapeHandle> {
        let s = self.get(shape)?;
        Ok(self.arena.alloc(s))
    }

    // ---- 八算子（M5-B6.2a）：**一律响亮拒绝，绝不返假几何** ------------------
    //
    // 与 `sew` / `fix_shape`（mock 做"恒等副本"）**不同**：八算子依赖真实拓扑
    // （选边 / 选面 / 放样截面 / 扫掠脊线），而占位后端 `sub_shapes(Edge/Vertex)`
    // 恒返空、无真实曲面。若在此返回一个"看起来成功"的句柄，会让**默认腿的几何
    // 断言假绿**（M0 铁律「跳过与通过必须可区分」所禁）。故：默认腿只验"响亮拒绝"，
    // 真实几何只挂 occt 腿（与 B4.0 / B6.1a 同构）。

    /// 占位后端不实现圆角：响亮拒绝，绝不返假几何。
    fn fillet(
        &mut self,
        _solid: ShapeHandle,
        _edges: &[ShapeHandle],
        _radius: f64,
    ) -> KernelResult<ShapeHandle> {
        Err(KernelError::Unsupported(
            "MockBackend 不实现 fillet：圆角依赖真实拓扑选边，仅为真实内核能力",
        ))
    }

    /// 占位后端不实现倒角：响亮拒绝，绝不返假几何。
    fn chamfer(
        &mut self,
        _solid: ShapeHandle,
        _edges: &[ShapeHandle],
        _distance: f64,
    ) -> KernelResult<ShapeHandle> {
        Err(KernelError::Unsupported(
            "MockBackend 不实现 chamfer：倒角依赖真实拓扑选边，仅为真实内核能力",
        ))
    }

    /// 占位后端不实现抽壳：响亮拒绝，绝不返假几何。
    fn shell(
        &mut self,
        _solid: ShapeHandle,
        _faces: &[ShapeHandle],
        _thickness: f64,
        _tolerance: f64,
    ) -> KernelResult<ShapeHandle> {
        Err(KernelError::Unsupported(
            "MockBackend 不实现 shell：抽壳依赖真实拓扑选面，仅为真实内核能力",
        ))
    }

    /// 占位后端不实现偏置：响亮拒绝，绝不返假几何。
    fn offset(
        &mut self,
        _shape: ShapeHandle,
        _distance: f64,
        _tolerance: f64,
    ) -> KernelResult<ShapeHandle> {
        Err(KernelError::Unsupported(
            "MockBackend 不实现 offset：偏置需真实曲面偏移，仅为真实内核能力",
        ))
    }

    /// 占位后端不实现加厚：响亮拒绝，绝不返假几何。
    fn thicken(
        &mut self,
        _shape: ShapeHandle,
        _thickness: f64,
        _tolerance: f64,
    ) -> KernelResult<ShapeHandle> {
        Err(KernelError::Unsupported(
            "MockBackend 不实现 thicken：加厚需真实面/壳拓扑，仅为真实内核能力",
        ))
    }

    /// 占位后端不实现放样：响亮拒绝，绝不返假几何。
    fn loft(
        &mut self,
        _wires: &[ShapeHandle],
        _is_solid: bool,
        _ruled: bool,
    ) -> KernelResult<ShapeHandle> {
        Err(KernelError::Unsupported(
            "MockBackend 不实现 loft：放样需真实截面 wire，仅为真实内核能力",
        ))
    }

    /// 占位后端不实现扫掠：响亮拒绝，绝不返假几何。
    fn sweep(
        &mut self,
        _profile: ShapeHandle,
        _spine: ShapeHandle,
        _transition_mode: i32,
    ) -> KernelResult<ShapeHandle> {
        Err(KernelError::Unsupported(
            "MockBackend 不实现 sweep：扫掠需真实轮廓与脊线，仅为真实内核能力",
        ))
    }

    /// 占位后端不实现管道：响亮拒绝，绝不返假几何。
    fn pipe(&mut self, _profile: ShapeHandle, _spine: ShapeHandle) -> KernelResult<ShapeHandle> {
        Err(KernelError::Unsupported(
            "MockBackend 不实现 pipe：管道需真实轮廓与脊线，仅为真实内核能力",
        ))
    }

    // ---- 线框生产（M7a）：记录型占位 -----------------------------------------
    //
    // mock 的定位是「几何是简化的，只能用于跑通链路与单测」——**跑通链路正是
    // mock 的职责**：线 / 面 占位**如实记账**输入坐标与边数，支持
    // export/import_brep 自描述文本往返（v3 格式不变：坐标折进既有字段）。
    // 真组装（wire 连接、面朝向、外环提取）以 **OCCT 内核腿
    // （`--features occt --release`）** 为准。与八算子不同：线/面不依赖真实拓扑
    // 选元，占位记账是**可复现的确定性记录**，不是编造几何。

    /// 占位"直边"：**记录型**——`origin` 记第 1 端点、`size` 记第 2 端点，
    /// 体积 = 0（线无体积）、`solid_count = 0`（不是实体）。
    fn make_line_edge(
        &mut self,
        x1: f64,
        y1: f64,
        z1: f64,
        x2: f64,
        y2: f64,
        z2: f64,
    ) -> KernelResult<ShapeHandle> {
        for (name, v) in [
            ("x1", x1),
            ("y1", y1),
            ("z1", z1),
            ("x2", x2),
            ("y2", y2),
            ("z2", z2),
        ] {
            finite_arg("make_line_edge", name, v)?;
        }
        let mut out = Shape::new(ShapeKind::EdgeLine, [x2, y2, z2], 0.0);
        out.origin = [x1, y1, z1];
        out.solid_count = 0;
        Ok(self.arena.alloc(out))
    }

    /// 占位"三点弧"：**记录型**——`origin` / `size` / `rotation` 依次记
    /// 起点 / 中间点 / 终点。
    #[allow(clippy::too_many_arguments)]
    fn make_arc_edge(
        &mut self,
        x1: f64,
        y1: f64,
        z1: f64,
        x2: f64,
        y2: f64,
        z2: f64,
        x3: f64,
        y3: f64,
        z3: f64,
    ) -> KernelResult<ShapeHandle> {
        for (name, v) in [
            ("x1", x1),
            ("y1", y1),
            ("z1", z1),
            ("x2", x2),
            ("y2", y2),
            ("z2", z2),
            ("x3", x3),
            ("y3", y3),
            ("z3", z3),
        ] {
            finite_arg("make_arc_edge", name, v)?;
        }
        let mut out = Shape::new(ShapeKind::EdgeArc, [x2, y2, z2], 0.0);
        out.origin = [x1, y1, z1];
        out.rotation = [x3, y3, z3];
        out.solid_count = 0;
        Ok(self.arena.alloc(out))
    }

    /// 占位"线框"：**记录型**——extents（成员边记录点的 min/max）折进
    /// `origin` / `size`，**边数**记在 `rotation[0]`；空表 →
    /// [`KernelError::InvalidArgument`] **响亮拒绝**（与 occt 腿同款，内核层把关）。
    fn make_wire(&mut self, edges: &[ShapeHandle]) -> KernelResult<ShapeHandle> {
        if edges.is_empty() {
            return Err(KernelError::InvalidArgument(
                "make_wire: 边集为空，至少需要 1 条边".to_string(),
            ));
        }
        let members = edges
            .iter()
            .map(|&e| self.get(e))
            .collect::<KernelResult<Vec<_>>>()?;
        let (mut min, mut max) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
        for m in &members {
            for p in member_points(m) {
                for k in 0..3 {
                    min[k] = min[k].min(p[k]);
                    max[k] = max[k].max(p[k]);
                }
            }
        }
        let mut out = Shape::new(
            ShapeKind::Wire,
            [max[0] - min[0], max[1] - min[1], max[2] - min[2]],
            0.0,
        );
        out.origin = min;
        out.rotation = [members.len() as f64, 0.0, 0.0];
        out.solid_count = 0;
        Ok(self.arena.alloc(out))
    }

    /// 占位"由 wire 造面"：**记录型**——extents / 边数抄自输入 wire；
    /// wire 不存活时经 `get` 响亮拒绝。
    fn make_face(&mut self, wire: ShapeHandle) -> KernelResult<ShapeHandle> {
        let w = self.get(wire)?;
        let mut out = Shape::new(ShapeKind::Face, w.size, 0.0);
        out.origin = w.origin;
        out.rotation = w.rotation;
        out.solid_count = 0;
        Ok(self.arena.alloc(out))
    }

    /// 占位"提取外环"：**记录型**——返回入参 face 的**占位 wire 句柄**
    /// （extents / 边数抄自 face，种类为 [`ShapeKind::Wire`]，文本与 face 不同）。
    fn outer_wire(&mut self, face: ShapeHandle) -> KernelResult<ShapeHandle> {
        let f = self.get(face)?;
        let mut out = Shape::new(ShapeKind::Wire, f.size, 0.0);
        out.origin = f.origin;
        out.rotation = f.rotation;
        out.solid_count = 0;
        Ok(self.arena.alloc(out))
    }

    fn translate(
        &mut self,
        shape: ShapeHandle,
        dx: f64,
        dy: f64,
        dz: f64,
    ) -> KernelResult<ShapeHandle> {
        // 平移是刚体运动：种类 / 尺寸 / 体积都不变，但**定位点真的会变**——
        // 把偏移量累加进 origin，导出文本随之改变（v2 起），存盘不再丢位置。
        // 原句柄仍有效，本操作返回一个**记了新定位点的新槽位**。
        for (name, v) in [("dx", dx), ("dy", dy), ("dz", dz)] {
            if !v.is_finite() {
                return Err(KernelError::InvalidArgument(format!(
                    "translate: {name} 必须为有限数，实得 {v}"
                )));
            }
        }
        let mut moved = self.get(shape)?;
        moved.origin[0] += dx;
        moved.origin[1] += dy;
        moved.origin[2] += dz;
        Ok(self.arena.alloc(moved))
    }

    fn tessellate(
        &mut self,
        shape: ShapeHandle,
        linear: f64,
        angular: f64,
    ) -> KernelResult<MeshData> {
        let _ = (linear, angular); // 占位后端不做挠度细分
        let s = self.get(shape)?;
        let mut mesh = match s.kind {
            ShapeKind::Sphere => sphere_mesh(s.size[0] / 2.0),
            // 占位：锥体的网格退化为柱状包围盒（复用 cylinder_mesh）。
            ShapeKind::Cylinder | ShapeKind::Cone => cylinder_mesh(s.size[0] / 2.0, s.size[2]),
            // 占位：环 / 拉伸 / 旋转体 / 复合体 / 壳 / 面的网格退化为其 AABB 盒
            // （尺寸已含旋转效应）。
            ShapeKind::Box
            | ShapeKind::Torus
            | ShapeKind::Extrude
            | ShapeKind::Revolve
            | ShapeKind::Compound
            | ShapeKind::Shell
            | ShapeKind::Face => box_mesh(s.size[0], s.size[1], s.size[2]),
            // 占位：线与线框没有可细分的表面 —— 返回空网格（线无表面，如实记账）。
            ShapeKind::EdgeLine | ShapeKind::EdgeArc | ShapeKind::Wire => empty_mesh(),
        };
        // **origin 纳入计算**：网格顶点整体挪到定位点。占位网格本是理想化结果，
        // 但位置必须如实反映——否则平移后的构件会"看起来还在原点"。
        for p in &mut mesh.positions {
            p[0] += s.origin[0];
            p[1] += s.origin[1];
            p[2] += s.origin[2];
        }
        Ok(mesh)
    }

    fn face_hashes(&mut self, shape: ShapeHandle) -> KernelResult<Vec<u64>> {
        // 占位后端的面哈希**就是它自己网格的面哈希**——由构造保证同域，
        // 正好对应真实后端"必须与网格化器用同一个 hash bound"那条约束。
        let mesh = self.tessellate(shape, 0.1, 0.5)?;
        Ok(mesh.face_groups.iter().map(|g| g.face_hash).collect())
    }

    fn project_edges(&mut self, shape: ShapeHandle, view: ViewDef) -> KernelResult<Projection> {
        let _ = view; // 占位后端忽略视向，输出正面视的理想化结果
        let s = self.get(shape)?;
        // **origin 纳入计算**：这份"理想正面视"只关心平面内的 (x, y)，
        // 故把投影坐标整体平移 (origin[0], origin[1])；origin[2]（进深）不影响
        // 这个理想正面视的轮廓（真实 HLR 由 OCCT 保证）。
        let offset = DVec2::new(s.origin[0], s.origin[1]);
        let mut visible = Vec::new();
        let mut hidden = Vec::new();
        match s.kind {
            ShapeKind::Box
            | ShapeKind::Torus
            | ShapeKind::Extrude
            | ShapeKind::Revolve
            | ShapeKind::Compound
            | ShapeKind::Shell => {
                rect_lines(&mut visible, s.size[0], s.size[1], LineClass::Sharp, offset);
            }
            // 占位：线框 / 面退化为 AABB 轮廓矩形（面尺寸来自 wire extents，非负）。
            ShapeKind::Wire | ShapeKind::Face => {
                rect_lines(&mut visible, s.size[0], s.size[1], LineClass::Sharp, offset);
            }
            // 占位：边种类投成一条线段（两端点 / 记录点的 x/y；三点弧退化为
            // 起点→中间点线段，理想化结果与既有占位口径一致）。
            ShapeKind::EdgeLine | ShapeKind::EdgeArc => {
                push_line(
                    &mut visible,
                    DVec2::new(s.origin[0], s.origin[1]),
                    DVec2::new(s.size[0], s.size[1]),
                    LineClass::Sharp,
                );
            }
            ShapeKind::Cylinder | ShapeKind::Cone => {
                rect_lines(
                    &mut visible,
                    s.size[0],
                    s.size[2],
                    LineClass::Outline,
                    offset,
                );
                visible.push(ProjectedCurve {
                    kind: CurveKind::Ellipse,
                    data: CurveGeometry::Ellipse {
                        center: offset,
                        rx: s.size[0] / 2.0,
                        ry: s.size[1] / 2.0,
                        rot: 0.0,
                    },
                    layer_hint: LineClass::Outline,
                });
            }
            ShapeKind::Sphere => {
                // 球的轮廓投影是一个精确圆——这正是 M0"精确圆判定"要守的能力
                let r = s.size[0] / 2.0;
                visible.push(ProjectedCurve {
                    kind: CurveKind::Circle,
                    data: CurveGeometry::Circle {
                        center: offset,
                        radius: r,
                    },
                    layer_hint: LineClass::Outline,
                });
                hidden.push(ProjectedCurve {
                    kind: CurveKind::Circle,
                    data: CurveGeometry::Circle {
                        center: offset,
                        radius: r,
                    },
                    layer_hint: LineClass::Smooth,
                });
            }
        }
        Ok(Projection { visible, hidden })
    }

    fn import_brep(&mut self, text: &str) -> KernelResult<ShapeHandle> {
        let shape = Shape::from_text(text)?;
        Ok(self.arena.alloc(shape))
    }

    fn export_brep(&mut self, shape: ShapeHandle) -> KernelResult<String> {
        Ok(self.get(shape)?.to_text())
    }

    fn volume(&mut self, shape: ShapeHandle) -> KernelResult<f64> {
        // 体积是平移**不变量**：`origin` 怎么挪，`volume` 都不变，故此处不读它
        // ——这是有意的（不是"忘了 origin"）。见模块文档 v2 说明。
        Ok(self.get(shape)?.volume)
    }

    fn is_alive(&self, shape: ShapeHandle) -> bool {
        self.arena.is_alive(shape)
    }

    fn release(&mut self, shape: ShapeHandle) -> KernelResult<()> {
        // 占位后端没有"内核侧"要通知，取出即释放。
        self.arena.take(shape).map(|_| ())
    }

    fn live_count(&self) -> usize {
        self.arena.live_count()
    }

    fn live_handles(&self) -> Vec<ShapeHandle> {
        self.arena.live_handles()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::DVec3;

    #[test]
    fn box_allocation_and_release_roundtrip() {
        let mut b = MockBackend::new();
        let h = b.make_box(100.0, 200.0, 300.0).unwrap();
        assert!(b.is_alive(h));
        assert_eq!(b.live_count(), 1);
        b.release(h).unwrap();
        assert!(!b.is_alive(h));
        assert_eq!(b.live_count(), 0);
    }

    #[test]
    fn released_handle_is_rejected() {
        let mut b = MockBackend::new();
        let h = b.make_box(1.0, 1.0, 1.0).unwrap();
        b.release(h).unwrap();
        assert_eq!(
            b.tessellate(h, 0.1, 0.5),
            Err(KernelError::InvalidHandle(h))
        );
        // 二次释放也报错
        assert!(b.release(h).is_err());
    }

    #[test]
    fn arena_reuses_freed_slots() {
        let mut b = MockBackend::new();
        let h1 = b.make_box(1.0, 1.0, 1.0).unwrap();
        b.release(h1).unwrap();
        let h2 = b.make_box(2.0, 2.0, 2.0).unwrap();
        assert_eq!(h1, h2, "释放的槽位应被复用（arena 语义）");
    }

    #[test]
    fn non_positive_dimensions_are_rejected() {
        let mut b = MockBackend::new();
        assert!(matches!(
            b.make_box(0.0, 1.0, 1.0),
            Err(KernelError::InvalidArgument(_))
        ));
        assert!(matches!(
            b.make_sphere(f64::NAN),
            Err(KernelError::InvalidArgument(_))
        ));
    }

    #[test]
    fn boolean_requires_both_alive() {
        let mut b = MockBackend::new();
        let a = b.make_box(1.0, 1.0, 1.0).unwrap();
        let dead = ShapeHandle(999);
        assert!(matches!(
            b.fuse(a, dead),
            Err(KernelError::InvalidHandle(_))
        ));
    }

    #[test]
    fn boolean_result_is_a_fresh_handle() {
        let mut b = MockBackend::new();
        let a = b.make_box(10.0, 10.0, 10.0).unwrap();
        let c = b.make_sphere(3.0).unwrap();
        let f = b.fuse(a, c).unwrap();
        assert!(b.is_alive(f));
        assert_ne!(f, a);
        assert_eq!(b.live_count(), 3);
    }

    #[test]
    fn sphere_projects_to_exact_circle() {
        let mut b = MockBackend::new();
        let s = b.make_sphere(50.0).unwrap();
        let p = b
            .project_edges(s, ViewDef::top(DVec3::new(0.0, 0.0, 1000.0)))
            .unwrap();
        assert_eq!(p.visible.len(), 1);
        assert_eq!(p.visible[0].kind, CurveKind::Circle);
        match p.visible[0].data {
            CurveGeometry::Circle { radius, .. } => assert!((radius - 50.0).abs() < 1e-12),
            _ => panic!("应当是圆"),
        }
    }

    #[test]
    fn box_projects_to_four_sharp_lines() {
        let mut b = MockBackend::new();
        let bx = b.make_box(100.0, 200.0, 300.0).unwrap();
        let p = b.project_edges(bx, ViewDef::front(DVec3::ZERO)).unwrap();
        assert_eq!(p.visible.len(), 4);
        assert!(p.visible.iter().all(|c| c.layer_hint == LineClass::Sharp));
    }

    #[test]
    fn box_mesh_has_six_faces_and_twelve_triangles() {
        let mut b = MockBackend::new();
        let h = b.make_box(1.0, 1.0, 1.0).unwrap();
        let m = b.tessellate(h, 0.1, 0.5).unwrap();
        assert_eq!(m.vertex_count(), 8);
        assert_eq!(m.triangle_count(), 12);
        assert_eq!(m.face_groups.len(), 6);
        let total: u32 = m.face_groups.iter().map(|g| g.tri_count).sum();
        assert_eq!(total, 12, "面分组的三角形总数应等于总三角形数");

        // 拾取粗筛的契约（两个后端必须一致，见 `KernelBackend::face_hashes`）：
        // 内核面哈希与网格面哈希**同域同序**。这条在真实后端是"bound 必须对齐"
        // （`FACE_HASH_UPPER_BOUND`），在占位后端是"由构造保证"——但**契约一样**。
        let from_mesh: Vec<u64> = m.face_groups.iter().map(|g| g.face_hash).collect();
        assert_eq!(b.face_hashes(h).unwrap(), from_mesh);
    }

    #[test]
    fn mock_declares_itself_as_a_mock_backend() {
        // 这条断言是"不许静默降级"的底座：内核池靠它分辨真假。
        assert_eq!(MockBackend::new().kind(), BackendKind::Mock);
        assert!(!MockBackend::new().kind().is_real());
    }

    #[test]
    fn brep_text_roundtrip_preserves_kind_size_and_volume() {
        let mut b = MockBackend::new();
        let h = b.make_box(10.0, 20.0, 30.0).unwrap();
        let v1 = b.volume(h).unwrap();
        assert!((v1 - 6000.0).abs() < 1e-9, "长方体体积应为 6000，实得 {v1}");

        let text = b.export_brep(h).unwrap();
        assert!(
            text.starts_with(MOCK_BREP_MAGIC),
            "占位格式应自带魔数，便于一眼看出它不是 OCCT BREP：{text}"
        );

        let back = b.import_brep(&text).unwrap();
        assert_ne!(back, h, "导入应得到新的会话句柄");
        assert!((b.volume(back).unwrap() - v1).abs() < 1e-9);
        // 形状本身也要一致：网格化结果能对上
        assert_eq!(
            b.tessellate(back, 0.1, 0.5).unwrap().vertex_count(),
            b.tessellate(h, 0.1, 0.5).unwrap().vertex_count()
        );
    }

    #[test]
    fn malformed_brep_text_is_rejected_not_silently_accepted() {
        let mut b = MockBackend::new();
        for bad in [
            "",
            "garbage",
            "MOCK-BREP v1 box 1 2",         // 参数不够
            "MOCK-BREP v9 box 1 2 3 6",     // 版本不对
            "MOCK-BREP v1 pyramid 1 2 3 6", // 未知种类
            // 非有限数值：`NaN` / `±inf` 都是合法 f64 token，必须显式拦下
            "MOCK-BREP v2 box NaN 200 300 6e6 0 0 0", // size.x = NaN
            "MOCK-BREP v2 box 100 200 300 NaN 0 0 0", // volume = NaN
            "MOCK-BREP v2 box 100 200 300 6e6 inf 0 0", // origin.x = +inf
            "MOCK-BREP v2 box 100 200 300 6e6 0 -inf 0", // origin.y = -inf
            "MOCK-BREP v1 box 100 200 300 NaN",       // v1 volume = NaN
        ] {
            assert!(
                matches!(b.import_brep(bad), Err(KernelError::Geometry { .. })),
                "非法文本应报 Geometry 错误：{bad:?}"
            );
        }
        assert_eq!(b.live_count(), 0, "失败的导入不应留下句柄");
    }

    #[test]
    fn boolean_volumes_follow_the_documented_placeholder_rules() {
        let mut b = MockBackend::new();
        let big = b.make_box(10.0, 10.0, 10.0).unwrap(); // 1000
        let small = b.make_box(2.0, 2.0, 2.0).unwrap(); // 8
        let fused = b.fuse(big, small).unwrap();
        let cut = b.cut(big, small).unwrap();
        let common = b.common(big, small).unwrap();
        assert!((b.volume(fused).unwrap() - 1008.0).abs() < 1e-9);
        assert!((b.volume(cut).unwrap() - 992.0).abs() < 1e-9);
        assert!((b.volume(common).unwrap() - 8.0).abs() < 1e-9);
    }

    #[test]
    fn translate_yields_a_new_handle_with_unchanged_volume() {
        let mut b = MockBackend::new();
        let h = b.make_box(10.0, 20.0, 30.0).unwrap();
        let moved = b.translate(h, 1000.0, 0.0, 3000.0).unwrap();
        assert!(b.is_alive(h), "原句柄仍有效（平移产生新形状）");
        assert_ne!(moved, h);
        assert!((b.volume(moved).unwrap() - b.volume(h).unwrap()).abs() < 1e-9);
        assert_eq!(b.live_count(), 2);
    }

    #[test]
    fn translate_of_a_released_handle_is_rejected() {
        let mut b = MockBackend::new();
        let h = b.make_box(1.0, 1.0, 1.0).unwrap();
        b.release(h).unwrap();
        assert_eq!(
            b.translate(h, 1.0, 0.0, 0.0),
            Err(KernelError::InvalidHandle(h))
        );
    }

    #[test]
    fn translate_rejects_non_finite_deltas() {
        let mut b = MockBackend::new();
        let h = b.make_box(1.0, 1.0, 1.0).unwrap();
        assert!(matches!(
            b.translate(h, f64::NAN, 0.0, 0.0),
            Err(KernelError::InvalidArgument(_))
        ));
    }

    /// v2/v3 的**要害**：平移必须改变 BREP 文本，否则存盘后就丢了位置。
    #[test]
    fn brep_text_now_carries_the_origin_and_roundtrips() {
        let mut b = MockBackend::new();
        let h = b.make_box(100.0, 200.0, 300.0).unwrap();
        let plain = b.export_brep(h).unwrap();
        assert!(plain.starts_with("MOCK-BREP v3"), "新格式应为 v3：{plain}");

        let moved = b.translate(h, 1000.0, 0.0, 3000.0).unwrap();
        let text = b.export_brep(moved).unwrap();
        assert_ne!(
            text, plain,
            "平移必须改变 BREP 文本（否则就是静默丢数据）：{text:?}"
        );
        assert_eq!(
            text, "MOCK-BREP v3 box 100 200 300 6000000 1000 0 3000 0 0 0 1",
            "v3 文本应如实带上 origin（末尾 rotation 归零、单实体计 1）"
        );

        // 文本 → 句柄 → 文本：定位点必须原样往返
        let back = b.import_brep(&text).unwrap();
        assert_eq!(b.export_brep(back).unwrap(), text);
    }

    /// v1 旧文本仍可读回：一律视作 `origin / rotation = 0`、`solid_count = 1`。
    #[test]
    fn v1_text_is_still_accepted_with_zero_origin() {
        let mut b = MockBackend::new();
        let back = b
            .import_brep("MOCK-BREP v1 box 100 200 300 6e6")
            .expect("v1 文本必须仍可解析");
        assert!((b.volume(back).unwrap() - 6_000_000.0).abs() < 1e-6);
        // 再导出会升级成 v3，且 origin / rotation 归零、solid_count = 1
        let text = b.export_brep(back).unwrap();
        assert_eq!(
            text, "MOCK-BREP v3 box 100 200 300 6000000 0 0 0 0 0 0 1",
            "v1 读回应落在原点、姿态归零、单实体：{text}"
        );

        // v1 的球体 + 网格化也不受影响
        let sphere = b.import_brep("MOCK-BREP v1 sphere 4 4 4 33.5").unwrap();
        assert_eq!(
            b.tessellate(sphere, 0.1, 0.5).unwrap().triangle_count(),
            16 * 8 * 2
        );
    }

    /// 多次平移累积到 `origin`；体积是不变量。
    #[test]
    fn translate_offsets_accumulate_and_volume_is_unchanged() {
        let mut b = MockBackend::new();
        let h = b.make_box(10.0, 20.0, 30.0).unwrap();
        let m1 = b.translate(h, 1000.0, 0.0, 3000.0).unwrap();
        let m2 = b.translate(m1, 5.0, -2.0, 1.0).unwrap();
        assert_eq!(
            b.export_brep(m2).unwrap(),
            "MOCK-BREP v3 box 10 20 30 6000 1005 -2 3001 0 0 0 1",
            "偏移应逐次累加"
        );
        assert!(
            (b.volume(m2).unwrap() - 6000.0).abs() < 1e-9,
            "平移不改体积"
        );
        assert!((b.volume(m1).unwrap() - 6000.0).abs() < 1e-9);
    }

    /// `tessellate` / `project_edges` 必须把 `origin` 纳入计算（不许静默忽略）。
    #[test]
    fn origin_moves_the_tessellated_mesh_and_the_projection() {
        let mut b = MockBackend::new();
        let h = b.make_box(10.0, 20.0, 30.0).unwrap();
        let moved = b.translate(h, 5.0, 6.0, 7.0).unwrap();

        let m0 = b.tessellate(h, 0.1, 0.5).unwrap();
        let m1 = b.tessellate(moved, 0.1, 0.5).unwrap();
        assert_eq!(m0.positions[0], [0.0, 0.0, 0.0]);
        assert_eq!(m1.positions[0], [5.0, 6.0, 7.0], "网格顶点应挪到定位点");

        let p0 = b.project_edges(h, ViewDef::front(DVec3::ZERO)).unwrap();
        let p1 = b.project_edges(moved, ViewDef::front(DVec3::ZERO)).unwrap();
        let first = |p: &Projection| match p.visible[0].data {
            CurveGeometry::Line { a, .. } => a,
            _ => panic!("box 投影应是线段"),
        };
        assert_eq!(first(&p0), DVec2::new(0.0, 0.0));
        assert_eq!(first(&p1), DVec2::new(5.0, 6.0), "投影应带上定位点偏移");
    }

    #[test]
    fn live_handles_expose_the_arena_contents() {
        let mut b = MockBackend::new();
        let a = b.make_box(1.0, 1.0, 1.0).unwrap();
        let c = b.make_sphere(1.0).unwrap();
        b.release(a).unwrap();
        assert_eq!(b.live_handles(), vec![c]);
        assert_eq!(b.live_count(), 1);
    }

    #[test]
    fn volume_of_released_handle_is_rejected() {
        let mut b = MockBackend::new();
        let h = b.make_box(1.0, 1.0, 1.0).unwrap();
        b.release(h).unwrap();
        assert_eq!(b.volume(h), Err(KernelError::InvalidHandle(h)));
        assert_eq!(b.export_brep(h), Err(KernelError::InvalidHandle(h)));
    }

    // ---- M3 新算子：锥 / 环 / 拉伸 / 旋转体 / 刚体旋转 ----------------------

    #[test]
    fn cone_and_torus_have_tagged_text_and_expected_volumes() {
        let mut b = MockBackend::new();
        let cone = b.make_cone(3.0, 0.0, 10.0).unwrap(); // 尖顶锥
        let want = std::f64::consts::PI * 10.0 / 3.0 * 9.0; // πh/3·(r1²)
        assert!((b.volume(cone).unwrap() - want).abs() < 1e-9);
        assert!(b.export_brep(cone).unwrap().contains(" cone "));

        let frustum = b.make_cone(3.0, 3.0, 10.0).unwrap(); // r1==r2 → 退化为柱
        let want_f = std::f64::consts::PI * 10.0 / 3.0 * (9.0 + 9.0 + 9.0);
        assert!((b.volume(frustum).unwrap() - want_f).abs() < 1e-9);

        let torus = b.make_torus(10.0, 2.0).unwrap();
        let want_t = 2.0 * std::f64::consts::PI * std::f64::consts::PI * 10.0 * 4.0;
        assert!((b.volume(torus).unwrap() - want_t).abs() < 1e-9);
        assert!(b.export_brep(torus).unwrap().contains(" torus "));
    }

    #[test]
    fn cone_rejects_negative_radius_but_allows_zero_tip() {
        let mut b = MockBackend::new();
        assert!(matches!(
            b.make_cone(-1.0, 0.0, 10.0),
            Err(KernelError::InvalidArgument(_))
        ));
        assert!(b.make_cone(0.0, 0.0, 10.0).is_ok(), "双 0 半径是退化但合法");
        assert!(matches!(
            b.make_cone(1.0, 1.0, 0.0),
            Err(KernelError::InvalidArgument(_))
        ));
    }

    #[test]
    fn extrude_grows_the_bbox_along_the_delta_and_tags_the_shape() {
        let mut b = MockBackend::new();
        let base = b.make_box(10.0, 10.0, 10.0).unwrap();
        let ext = b.extrude(base, 5.0, 0.0, 20.0).unwrap();
        let bb = b.bounding_box(ext).unwrap();
        assert_eq!(bb.size(), [15.0, 10.0, 30.0], "沿拉伸向长大");
        assert_eq!(b.volume(ext).unwrap(), 15.0 * 10.0 * 30.0);
        assert!(b.export_brep(ext).unwrap().contains(" extrude "));
    }

    #[test]
    fn revolve_volume_depends_on_the_swept_angle() {
        let mut b = MockBackend::new();
        let profile = b.make_box(10.0, 10.0, 4.0).unwrap();
        let half = b
            .revolve(profile, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, std::f64::consts::PI)
            .unwrap();
        let full = b
            .revolve(profile, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, std::f64::consts::TAU)
            .unwrap();
        assert!(
            (b.volume(full).unwrap() - 2.0 * b.volume(half).unwrap()).abs() < 1e-6,
            "扫掠 2π 的体积应是 π 的两倍（角度必须纳入）"
        );
        assert!(b.export_brep(half).unwrap().contains(" revolve "));
    }

    /// **M3 头号占位钉子**：旋转必须改变文本**且**改变几何（不许静默成功）。
    #[test]
    fn rotate_changes_brep_text_and_moves_geometry() {
        let mut b = MockBackend::new();
        // 用**非立方**盒，保证绕 Z 转 90° 后 AABB 尺寸确实改变。
        let h = b.make_box(10.0, 20.0, 30.0).unwrap();
        let plain = b.export_brep(h).unwrap();
        let bb0 = b.bounding_box(h).unwrap();
        let m0 = b.tessellate(h, 0.1, 0.5).unwrap();

        let rot = b
            .rotate(h, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, std::f64::consts::FRAC_PI_2)
            .unwrap();

        // ① 文本必须变（姿态进了文本）
        let text = b.export_brep(rot).unwrap();
        assert_ne!(text, plain, "旋转必须改变 BREP 文本（否则存盘后凭空消失）");
        // ② 体积是刚体不变量
        assert!((b.volume(rot).unwrap() - b.volume(h).unwrap()).abs() < 1e-9);
        // ③ 几何（AABB / 网格）必须跟着转：绕 Z 转 90° → x/y 尺寸互换
        let bb1 = b.bounding_box(rot).unwrap();
        assert!((bb1.size()[0] - 20.0).abs() < 1e-9 && (bb1.size()[1] - 10.0).abs() < 1e-9);
        let m1 = b.tessellate(rot, 0.1, 0.5).unwrap();
        assert_ne!(m0.positions, m1.positions, "网格顶点必须反映旋转");
        assert_ne!(bb0.size(), bb1.size(), "包围盒必须反映旋转");
    }

    #[test]
    fn rotate_rejects_zero_axis_and_non_finite_angle() {
        let mut b = MockBackend::new();
        let h = b.make_box(1.0, 1.0, 1.0).unwrap();
        assert!(matches!(
            b.rotate(h, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0),
            Err(KernelError::InvalidArgument(_))
        ));
        assert!(matches!(
            b.rotate(h, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, f64::NAN),
            Err(KernelError::InvalidArgument(_))
        ));
    }

    #[test]
    fn bounding_box_and_surface_area_are_reported() {
        let mut b = MockBackend::new();
        let h = b.make_box(2.0, 3.0, 4.0).unwrap();
        let bb = b.bounding_box(h).unwrap();
        assert_eq!(bb.min, [0.0, 0.0, 0.0]);
        assert_eq!(bb.max, [2.0, 3.0, 4.0]);
        // 表面积 = 2·(2·3 + 3·4 + 2·4) = 52
        assert!((b.surface_area(h).unwrap() - 52.0).abs() < 1e-9);

        // 平移不改 bbox 尺寸，只挪 min
        let moved = b.translate(h, 100.0, 0.0, 0.0).unwrap();
        assert_eq!(b.bounding_box(moved).unwrap().min, [100.0, 0.0, 0.0]);
    }

    // ---- M3 复合体：子形状枚举 / 布尔 solid_count 记账 ----------------------

    #[test]
    fn a_disconnected_fuse_yields_two_solids_a_connected_one_does_not() {
        let mut b = MockBackend::new();
        let a = b.make_box(10.0, 10.0, 10.0).unwrap();
        // 挪到远处 → 与 a 不相连
        let far_box = b.make_box(10.0, 10.0, 10.0).unwrap();
        let far = b.translate(far_box, 100.0, 0.0, 0.0).unwrap();
        let disjoint = b.fuse(a, far).unwrap();
        assert_eq!(
            b.sub_shape_count(disjoint, SubShapeKind::Solid).unwrap(),
            2,
            "远离的两个盒 fuse 后应含 2 个实体"
        );
        assert_eq!(
            b.sub_shapes(disjoint, SubShapeKind::Solid).unwrap().len(),
            2
        );

        // 部分重叠 → 相连 → 仍是 1 个实体
        let near_box = b.make_box(10.0, 10.0, 10.0).unwrap();
        let near = b.translate(near_box, 5.0, 0.0, 0.0).unwrap();
        let joined = b.fuse(a, near).unwrap();
        assert_eq!(
            b.sub_shape_count(joined, SubShapeKind::Solid).unwrap(),
            1,
            "重叠的两个盒 fuse 后应仍是 1 个实体"
        );
    }

    #[test]
    fn non_solid_sub_shapes_are_empty_in_the_placeholder() {
        let mut b = MockBackend::new();
        let h = b.make_box(1.0, 1.0, 1.0).unwrap();
        // 占位后端没有拓扑：边/面等一律返回空（已在文档中写明）。
        for kind in [
            SubShapeKind::Edge,
            SubShapeKind::Face,
            SubShapeKind::Shell,
            SubShapeKind::Vertex,
            SubShapeKind::Wire,
        ] {
            assert!(b.sub_shapes(h, kind).unwrap().is_empty());
            assert_eq!(b.sub_shape_count(h, kind).unwrap(), 0);
        }
        // 非复合体的 Solid 子形状 = 自身一份
        assert_eq!(b.sub_shapes(h, SubShapeKind::Solid).unwrap().len(), 1);
    }

    // ---- v3 兼容：旧版读回、非有限 rotation / solid_count 拒收 --------------

    #[test]
    fn v2_text_is_accepted_with_zero_rotation_and_single_solid() {
        let mut b = MockBackend::new();
        let back = b
            .import_brep("MOCK-BREP v2 box 100 200 300 6000000 1000 0 3000")
            .expect("v2 文本必须仍可解析");
        assert_eq!(
            b.export_brep(back).unwrap(),
            "MOCK-BREP v3 box 100 200 300 6000000 1000 0 3000 0 0 0 1",
            "v2 读回后 rotation 归零、solid_count = 1"
        );
    }

    #[test]
    fn v3_text_roundtrips_rotation_and_solid_count() {
        let mut b = MockBackend::new();
        let text = "MOCK-BREP v3 compound 10 20 30 6000 1 2 3 0.5 0 0 2";
        let h = b.import_brep(text).expect("合法 v3 文本应可解析");
        assert_eq!(b.export_brep(h).unwrap(), text, "v3 应逐字节往返");
        assert_eq!(b.sub_shape_count(h, SubShapeKind::Solid).unwrap(), 2);
    }

    #[test]
    fn non_finite_rotation_or_bad_solid_count_are_rejected() {
        let mut b = MockBackend::new();
        for bad in [
            "MOCK-BREP v3 box 1 2 3 6 0 0 0 NaN 0 0 1", // rotation.x = NaN
            "MOCK-BREP v3 box 1 2 3 6 0 0 0 0 inf 0 1", // rotation.y = inf
            "MOCK-BREP v3 box 1 2 3 6 0 0 0 0 0 0 -1",  // solid_count 负
            "MOCK-BREP v3 box 1 2 3 6 0 0 0 0 0 0 1.5", // solid_count 非整数
            "MOCK-BREP v3 box 1 2 3 6 0 0 0 0 0 0",     // 缺 solid_count（10 段）
            "MOCK-BREP v3 box 1 2 3 6 0 0 0 0 0 0 1 2", // 多一段
        ] {
            assert!(
                matches!(b.import_brep(bad), Err(KernelError::Geometry { .. })),
                "非法 v3 文本应报 Geometry 错误：{bad:?}"
            );
        }
        assert_eq!(b.live_count(), 0, "失败的导入不应留下句柄");
    }

    /// B4.0 钉子 ①：占位后端对点投影**响亮拒绝**——绝不返回假点。
    ///
    /// 守"跳过与通过可区分"：点投影是真实内核能力，mock 必须 `Unsupported`
    /// 而不是编造一个最近点（那会让拾取在 mock 腿假绿）。
    #[test]
    fn mock_rejects_point_projection_loudly() {
        let mut b = MockBackend::new();
        let h = b.make_box(1.0, 1.0, 1.0).unwrap();
        assert!(
            matches!(
                b.project_point_on_face(h, DVec3::new(0.5, 0.5, 1.0)),
                Err(KernelError::Unsupported(_))
            ),
            "mock 的 project_point_on_face 必须 Unsupported，不得返回假点"
        );
        assert!(
            matches!(
                b.project_point_on_edge(h, DVec3::ZERO),
                Err(KernelError::Unsupported(_))
            ),
            "mock 的 project_point_on_edge 必须 Unsupported，不得返回假点"
        );
    }

    /// B4.0 钉子 ⑤：`Kernel` 的**一行转发**真的落到后端（守 M3「能编译≠被消费」型盲区）。
    ///
    /// `mock_rejects_point_projection_loudly` 直连 [`MockBackend`]，覆盖不到
    /// [`crate::Kernel::project_point_on_face`] / [`crate::Kernel::project_point_on_edge`]
    /// 这两处 `self.backend.X(...)` 转发——转发是 **backend-泛型**的，故用 `Kernel::mock()`
    /// 走**完整转发路径**即可钉住（默认腿可跑、零 OOM 风险，不必起 occt 腿第二具内核）。
    /// 若转发被改坏（忽略后端返回假 `Ok`、或接到错的 trait 方法），本用例即精确打红。
    #[test]
    fn kernel_forwards_point_projection_to_backend() {
        let mut k = crate::Kernel::mock();
        let h = k.make_box(10.0, 10.0, 10.0).expect("mock 建模应成功");
        let p = DVec3::new(0.0, 0.0, 0.0);
        assert!(
            matches!(
                k.project_point_on_face(h, p),
                Err(crate::KernelError::Unsupported(_))
            ),
            "Kernel::project_point_on_face 必须把调用转发到后端（mock 应响亮拒绝）"
        );
        assert!(
            matches!(
                k.project_point_on_edge(h, p),
                Err(crate::KernelError::Unsupported(_))
            ),
            "Kernel::project_point_on_edge 必须把调用转发到后端（mock 应响亮拒绝）"
        );
    }

    /// B4.1b-1 钉子（默认腿）：`Kernel::surface_normal` / `Kernel::curve_parameters`
    /// 的**一行转发**真的落到后端，且 mock 侧**响亮拒绝**。
    ///
    /// 与 `kernel_forwards_point_projection_to_backend` 同构：转发是 **backend-泛型**的，
    /// 故经 `Kernel::mock()` 走完整转发路径即可钉住两条门面（默认腿可跑、零 OOM 风险）。
    /// 若转发被改坏（忽略后端返回假 `Ok`、或接到错的 trait 方法），本用例即精确打红。
    #[test]
    fn mock_rejects_surface_normal_and_curve_parameters_loudly() {
        let mut k = crate::Kernel::mock();
        let h = k.make_box(10.0, 10.0, 10.0).expect("mock 建模应成功");
        assert!(
            matches!(
                k.surface_normal(h, 0.5, 0.5),
                Err(crate::KernelError::Unsupported(_))
            ),
            "Kernel::surface_normal 必须把调用转发到后端（mock 应响亮拒绝，不得返回假法向）"
        );
        assert!(
            matches!(
                k.curve_parameters(h),
                Err(crate::KernelError::Unsupported(_))
            ),
            "Kernel::curve_parameters 必须把调用转发到后端（mock 应响亮拒绝，不得返回假区间）"
        );
    }

    /// B6.1a 钉子（默认腿）：`Kernel::curve_point_at_param` / `Kernel::curve_length`
    /// 的**一行转发**真的落到后端，且 mock 侧**响亮拒绝**。
    ///
    /// 与 `mock_rejects_surface_normal_and_curve_parameters_loudly` 同构：转发是
    /// **backend-泛型**的，故经 `Kernel::mock()` 走完整转发路径即可钉住两条新门面
    /// （默认腿可跑、零 OOM 风险）。若转发被改坏（忽略后端返回假 `Ok`、或接到错的
    /// trait 方法），本用例即精确打红。
    #[test]
    fn mock_rejects_curve_point_at_param_and_curve_length_loudly() {
        let mut k = crate::Kernel::mock();
        let h = k.make_box(10.0, 10.0, 10.0).expect("mock 建模应成功");
        assert!(
            matches!(
                k.curve_point_at_param(h, 0.0),
                Err(crate::KernelError::Unsupported(_))
            ),
            "Kernel::curve_point_at_param 必须把调用转发到后端（mock 应响亮拒绝，不得返回假点）"
        );
        assert!(
            matches!(k.curve_length(h), Err(crate::KernelError::Unsupported(_))),
            "Kernel::curve_length 必须把调用转发到后端（mock 应响亮拒绝，不得返回假长度）"
        );
    }

    // ---- B6.2a 八算子（默认腿）：8 条**独立**响亮拒绝钉 ---------------------
    //
    // 经 `Kernel::mock()` 走**完整转发路径**（覆盖 `Kernel::<op>` 的一行转发，
    // 避开"只直连后端、转发被改坏也不报"的盲区；转发是 backend-泛型的）。
    // 8 条**独立**：任一算子被误改成"返假几何"时，只有对应那一条精确打红。
    // 断言一律 `Err(Unsupported)`——mock **绝不返假几何**。

    /// B6.2a 钉子 1：`fillet` 经 `Kernel::mock()` 响亮拒绝。
    #[test]
    fn mock_rejects_fillet_loudly() {
        let mut k = crate::Kernel::mock();
        let solid = k.make_box(10.0, 10.0, 10.0).expect("mock 建模应成功");
        assert!(
            matches!(
                k.fillet(solid, &[solid], 1.0),
                Err(crate::KernelError::Unsupported(_))
            ),
            "Kernel::fillet 必须把调用转发到后端（mock 应响亮拒绝，不得返假几何）"
        );
    }

    /// B6.2a 钉子 2：`chamfer` 经 `Kernel::mock()` 响亮拒绝。
    #[test]
    fn mock_rejects_chamfer_loudly() {
        let mut k = crate::Kernel::mock();
        let solid = k.make_box(10.0, 10.0, 10.0).expect("mock 建模应成功");
        assert!(
            matches!(
                k.chamfer(solid, &[solid], 1.0),
                Err(crate::KernelError::Unsupported(_))
            ),
            "Kernel::chamfer 必须把调用转发到后端（mock 应响亮拒绝，不得返假几何）"
        );
    }

    /// B6.2a 钉子 3：`shell` 经 `Kernel::mock()` 响亮拒绝。
    #[test]
    fn mock_rejects_shell_loudly() {
        let mut k = crate::Kernel::mock();
        let solid = k.make_box(10.0, 10.0, 10.0).expect("mock 建模应成功");
        assert!(
            matches!(
                k.shell(solid, &[solid], 1.0, 1e-6),
                Err(crate::KernelError::Unsupported(_))
            ),
            "Kernel::shell 必须把调用转发到后端（mock 应响亮拒绝，不得返假几何）"
        );
    }

    /// B6.2a 钉子 4：`offset` 经 `Kernel::mock()` 响亮拒绝。
    #[test]
    fn mock_rejects_offset_loudly() {
        let mut k = crate::Kernel::mock();
        let solid = k.make_box(10.0, 10.0, 10.0).expect("mock 建模应成功");
        assert!(
            matches!(
                k.offset(solid, 1.0, 1e-6),
                Err(crate::KernelError::Unsupported(_))
            ),
            "Kernel::offset 必须把调用转发到后端（mock 应响亮拒绝，不得返假几何）"
        );
    }

    /// B6.2a 钉子 5：`thicken` 经 `Kernel::mock()` 响亮拒绝。
    #[test]
    fn mock_rejects_thicken_loudly() {
        let mut k = crate::Kernel::mock();
        let face = k.make_box(10.0, 10.0, 10.0).expect("mock 建模应成功");
        assert!(
            matches!(
                k.thicken(face, 1.0, 1e-6),
                Err(crate::KernelError::Unsupported(_))
            ),
            "Kernel::thicken 必须把调用转发到后端（mock 应响亮拒绝，不得返假几何）"
        );
    }

    /// B6.2a 钉子 6：`loft` 经 `Kernel::mock()` 响亮拒绝。
    #[test]
    fn mock_rejects_loft_loudly() {
        let mut k = crate::Kernel::mock();
        let w0 = k.make_box(10.0, 10.0, 1.0).expect("mock 建模应成功");
        let w1 = k.make_box(10.0, 10.0, 1.0).expect("mock 建模应成功");
        assert!(
            matches!(
                k.loft(&[w0, w1], true, false),
                Err(crate::KernelError::Unsupported(_))
            ),
            "Kernel::loft 必须把调用转发到后端（mock 应响亮拒绝，不得返假几何）"
        );
    }

    /// B6.2a 钉子 7：`sweep` 经 `Kernel::mock()` 响亮拒绝。
    #[test]
    fn mock_rejects_sweep_loudly() {
        let mut k = crate::Kernel::mock();
        let profile = k.make_box(1.0, 1.0, 1.0).expect("mock 建模应成功");
        let spine = k.make_box(1.0, 1.0, 10.0).expect("mock 建模应成功");
        assert!(
            matches!(
                k.sweep(profile, spine, 0),
                Err(crate::KernelError::Unsupported(_))
            ),
            "Kernel::sweep 必须把调用转发到后端（mock 应响亮拒绝，不得返假几何）"
        );
    }

    /// B6.2a 钉子 8：`pipe` 经 `Kernel::mock()` 响亮拒绝。
    #[test]
    fn mock_rejects_pipe_loudly() {
        let mut k = crate::Kernel::mock();
        let profile = k.make_box(1.0, 1.0, 1.0).expect("mock 建模应成功");
        let spine = k.make_box(1.0, 1.0, 10.0).expect("mock 建模应成功");
        assert!(
            matches!(
                k.pipe(profile, spine),
                Err(crate::KernelError::Unsupported(_))
            ),
            "Kernel::pipe 必须把调用转发到后端（mock 应响亮拒绝，不得返假几何）"
        );
    }

    // ---- M7a 线框生产（默认腿）：记录型占位钉 --------------------------------

    /// **M7a mock 钉子**：线 / 面占位**如实记账**并支持 v3 文本往返——
    /// 4 直边 → make_wire → make_face → outer_wire 全链走通；边记两端点 /
    /// 弧记三点；wire/face 体积 = 0、`sub_shape_count(Solid) = 0`（不是实体）；
    /// 空边集响亮拒绝。
    #[test]
    fn m7a_wire_face_placeholder_records_and_roundtrips() {
        let mut b = MockBackend::new();

        // 4 条直边（单位正方形顶边序）
        let e0 = b.make_line_edge(0.0, 0.0, 0.0, 10.0, 0.0, 0.0).unwrap();
        let e1 = b.make_line_edge(10.0, 0.0, 0.0, 10.0, 10.0, 0.0).unwrap();
        let e2 = b.make_line_edge(10.0, 10.0, 0.0, 0.0, 10.0, 0.0).unwrap();
        let e3 = b.make_line_edge(0.0, 10.0, 0.0, 0.0, 0.0, 0.0).unwrap();

        // 直边记两端点：文本往返后端点不丢
        let e0_text = b.export_brep(e0).unwrap();
        assert!(
            e0_text.contains(" edge_line "),
            "直边应带 edge_line 标签：{e0_text}"
        );
        let e0_back = b.import_brep(&e0_text).unwrap();
        assert_eq!(
            b.export_brep(e0_back).unwrap(),
            e0_text,
            "直边文本应逐字节往返"
        );

        // 三点弧记三点：半圆（r=10，圆心原点，XZ 平面：起点 (−10,0,0)，
        // 中间点 (0,0,10) 在圆上，终点 (10,0,0)）
        let arc = b
            .make_arc_edge(-10.0, 0.0, 0.0, 0.0, 0.0, 10.0, 10.0, 0.0, 0.0)
            .unwrap();
        let arc_text = b.export_brep(arc).unwrap();
        assert!(
            arc_text.contains(" edge_arc "),
            "三点弧应带 edge_arc 标签：{arc_text}"
        );
        let arc_back = b.import_brep(&arc_text).unwrap();
        assert_eq!(
            b.export_brep(arc_back).unwrap(),
            arc_text,
            "三点弧文本应逐字节往返"
        );

        // wire：记边集（边数进 rotation[0]，extents 折进 origin/size）
        let wire = b.make_wire(&[e0, e1, e2, e3]).unwrap();
        assert_eq!(b.volume(wire).unwrap(), 0.0, "线框无体积");
        assert_eq!(
            b.sub_shape_count(wire, SubShapeKind::Solid).unwrap(),
            0,
            "线框不是实体"
        );
        let wire_text = b.export_brep(wire).unwrap();
        assert!(
            wire_text.contains(" wire "),
            "线框应带 wire 标签：{wire_text}"
        );
        let wire_back = b.import_brep(&wire_text).unwrap();
        assert_eq!(
            b.export_brep(wire_back).unwrap(),
            wire_text,
            "线框文本应逐字节往返"
        );

        // face：抄自 wire 的 extents，但种类不同 → 文本必须与 wire 不同
        let face = b.make_face(wire).unwrap();
        assert_eq!(b.volume(face).unwrap(), 0.0, "占位面无体积（不返假体积）");
        let face_text = b.export_brep(face).unwrap();
        assert_ne!(face_text, wire_text, "face 文本必须与 wire 不同");
        assert!(
            face_text.contains(" face "),
            "面应带 face 标签：{face_text}"
        );

        // outer_wire：返回入参 face 的占位 wire 句柄（种类 wire，文本与 face 不同）
        let outer = b.outer_wire(face).unwrap();
        let outer_text = b.export_brep(outer).unwrap();
        assert_ne!(outer_text, face_text, "外环文本必须与 face 不同");
        assert!(
            outer_text.contains(" wire "),
            "外环应为 wire 标签：{outer_text}"
        );

        // 空边集：响亮拒绝（与 occt 腿同款 InvalidArgument）
        assert!(
            matches!(b.make_wire(&[]), Err(KernelError::InvalidArgument(_))),
            "空边集应响亮拒绝"
        );
    }
}
