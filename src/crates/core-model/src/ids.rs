//! 持久态标识符。
//!
//! **持久 id 永不复用**：一律用 **UUID v7**（时间有序）。这样"删了又建"不会
//! 造成引用错乱，也让 id 天然可按时间排序（设计文档 3.3）。
//!
//! 与内核**会话态** `ShapeHandle(u32)` 的区别见 `core-kernel` crate 文档。

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use uuid::Uuid;

macro_rules! persistent_id {
    ($name:ident, $doc:expr) => {
        #[doc = $doc]
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub Uuid);

        impl $name {
            /// 生成一个新的持久 id（UUID v7，时间有序）。
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }

            /// 由已有 UUID 构造（反序列化 / 迁移场景）。
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

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }
    };
}

persistent_id!(ElementId, "元素 id（构件、设备、基准元素…）。");
persistent_id!(
    ShapeId,
    "几何记录 id（对应 `geom/<shapeid>.brep` 一份几何）。"
);
persistent_id!(ViewId, "视图 id。视图本体在 `core-doc`，见 crate 文档。");
persistent_id!(SheetId, "图纸 id。图纸本体在 `core-doc`，见 crate 文档。");
persistent_id!(RelationId, "关系 id。");
persistent_id!(LayerId, "图层 id。");

// ---------------------------------------------------------------------------
// 稳定拓扑命名（设计文档 6.6；规划 §2.3 / §3）
//
// 红线（规划附录 A#2/#12）：**会话面哈希不可当持久键**。`TopoName` 必须由
// **几何量**（代表点 / 法向 / 尺度）构造，绝不可由内核 `hash_code` / 面序号构造。
// 量化常量**集中定义在此模块**，禁止各模块各写一份（规划 §3.4 B 段）。
// ---------------------------------------------------------------------------

/// 位置量化步长（mm）：代表点相对形状包围盒 min 的局部坐标，量化到 0.5 mm。
///
/// 0.5 mm 远小于构件尺寸、远大于浮点噪声 → 重建后同一点几乎必然落在同一格。
pub const POS_STEP_MM: f64 = 0.5;

/// 方向量化比例：单位向量分量 × 10000（即 `1e-4` 步长），存 `i16`。
pub const DIR_QUANT_SCALE: f64 = 10_000.0;

/// 面积量化步长（mm²）：面尺度 `size_q = round(area / 1.0)`。
pub const AREA_STEP_MM2: f64 = 1.0;

/// 长度量化步长（mm）：边尺度 `size_q = round(length / 1.0)`。
pub const LENGTH_STEP_MM: f64 = 1.0;

/// `exact_match` 的位置容差（量化格数，1 格 = 0.5 mm）。
pub const POS_MATCH_TOL_Q: i32 = 1;

/// `exact_match` 的方向容差（量化分量，50 ≈ 0.005 ≈ 0.3°）。
pub const DIR_MATCH_TOL_Q: i16 = 50;

/// `exact_match` 的尺度容差（量化格数）。
pub const SIZE_MATCH_TOL_Q: i64 = 1;

/// `distance` 的位置参照尺度（mm）：代表点相距 100 mm 记为分量距离 1.0。
pub const POS_REF_MM: f64 = 100.0;

/// `distance` 的方向参照：单位向量弦距 0.2（约 11.5°）记为分量距离 1.0。
pub const DIR_REF_CHORD: f64 = 0.2;

/// `distance` 的尺度相对差参照：相对差 0.5（50%）记为分量距离 1.0。
pub const SIZE_REF_REL: f64 = 0.5;

/// 最近匹配的可信阈值：`distance > MAX_TOPO_DISTANCE` 视为不可信（拒绝解析）。
pub const MAX_TOPO_DISTANCE: f64 = 1.0;

/// 稳定名适用的子形状种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum TopoKind {
    /// 面
    Face,
    /// 边
    Edge,
    /// 顶点
    Vertex,
}

/// 子形状的**跨会话稳定名**：由"局部量化几何指纹"构造（规划 §3.3）。
///
/// 三个分量都是**形状的内蕴几何量**，与内核子形状排序无关，因此跨会话、跨重建稳定：
/// - `pos_q`：代表点相对**该形状包围盒 min** 的局部坐标 → **对整体平移不变**；
/// - `dir_q`：面法向 / 边切向（顶点为 `[0;3]`）；
/// - `size_q`：面=面积 / 边=长度（顶点为 0）。
///
/// ⚠ **绝不可**由内核 `hash_code` / 面序号构造（会话内哈希跨会话不稳）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TopoName {
    /// 子形状种类
    pub kind: TopoKind,
    /// 代表点（**相对形状包围盒 min** 的局部坐标，量化到 [`POS_STEP_MM`]）
    pub pos_q: [i32; 3],
    /// 面法向 / 边切向（单位向量分量 × [`DIR_QUANT_SCALE`]，顶点为零）
    pub dir_q: [i16; 3],
    /// 尺度：面=面积 / 边=长度（量化）；顶点=0
    pub size_q: i64,
}

impl TopoName {
    /// 由**面**的代表点（世界坐标）、法向、面积 + 形状包围盒 min 构造。
    #[must_use]
    pub fn of_face(world_point: [f64; 3], normal: [f64; 3], area: f64, bbox_min: [f64; 3]) -> Self {
        Self {
            kind: TopoKind::Face,
            pos_q: quantize_pos(world_point, bbox_min),
            dir_q: quantize_dir(normal),
            size_q: quantize_scalar(area, AREA_STEP_MM2),
        }
    }

    /// 由**边**的中点（世界坐标）、切向、长度 + 形状包围盒 min 构造。
    #[must_use]
    pub fn of_edge(
        mid_point: [f64; 3],
        tangent: [f64; 3],
        length: f64,
        bbox_min: [f64; 3],
    ) -> Self {
        Self {
            kind: TopoKind::Edge,
            pos_q: quantize_pos(mid_point, bbox_min),
            dir_q: quantize_dir(tangent),
            size_q: quantize_scalar(length, LENGTH_STEP_MM),
        }
    }

    /// 由**顶点**坐标 + 形状包围盒 min 构造。
    #[must_use]
    pub fn of_vertex(world_point: [f64; 3], bbox_min: [f64; 3]) -> Self {
        Self {
            kind: TopoKind::Vertex,
            pos_q: quantize_pos(world_point, bbox_min),
            dir_q: [0, 0, 0],
            size_q: 0,
        }
    }

    /// 完全相等（同种类且三个分量都在容差内）。
    #[must_use]
    pub fn exact_match(&self, other: &Self) -> bool {
        self.kind == other.kind
            && (0..3).all(|i| (self.pos_q[i] - other.pos_q[i]).abs() <= POS_MATCH_TOL_Q)
            && (0..3).all(|i| (self.dir_q[i] - other.dir_q[i]).abs() <= DIR_MATCH_TOL_Q)
            && (self.size_q - other.size_q).abs() <= SIZE_MATCH_TOL_Q
    }

    /// 归一化距离（用于"最近匹配"回退；越小越像；`> MAX_TOPO_DISTANCE` 视为不可信）。
    ///
    /// 不同种类返回 `f64::INFINITY`（"绝不可能同一个子形状"）。
    #[must_use]
    pub fn distance(&self, other: &Self) -> f64 {
        if self.kind != other.kind {
            return f64::INFINITY;
        }
        let d_pos = {
            let mut sum = 0.0;
            for i in 0..3 {
                let d = f64::from(self.pos_q[i] - other.pos_q[i]) * POS_STEP_MM;
                sum += d * d;
            }
            sum.sqrt() / POS_REF_MM
        };
        let d_dir = {
            let mut sum = 0.0;
            for i in 0..3 {
                let d = f64::from(self.dir_q[i] - other.dir_q[i]) / DIR_QUANT_SCALE;
                sum += d * d;
            }
            sum.sqrt() / DIR_REF_CHORD
        };
        let d_size = {
            let a = self.size_q as f64;
            let b = other.size_q as f64;
            let denom = a.abs().max(b.abs()).max(1.0);
            ((a - b).abs() / denom) / SIZE_REF_REL
        };
        (d_pos * d_pos + d_dir * d_dir + d_size * d_size).sqrt()
    }
}

/// 位置 → 相对包围盒 min 的量化局部坐标。
fn quantize_pos(p: [f64; 3], min: [f64; 3]) -> [i32; 3] {
    [0, 1, 2].map(|i| ((p[i] - min[i]) / POS_STEP_MM).round() as i32)
}

/// 单位向量 → 缩放量化分量（`i16` 足够：|分量| ≤ 10000）。
fn quantize_dir(d: [f64; 3]) -> [i16; 3] {
    [0, 1, 2].map(|i| (d[i] * DIR_QUANT_SCALE).round() as i16)
}

/// 标量（面积 / 长度）→ 量化整数值。
fn quantize_scalar(v: f64, step: f64) -> i64 {
    (v / step).round() as i64
}

/// 子形状定位：`index` 是**会话内提示**（可能失效），`name` 是**跨会话稳定名**。
///
/// M5 起 `name` 非空（由 §3 指纹算出）；旧文档里为 `None`（向后兼容）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TopoRef {
    /// 会话内子形状序号（提示，跨会话可能失效）
    pub index: u32,
    /// 跨会话稳定名（旧文档为 `None`）
    pub name: Option<TopoName>,
}

/// 子形状的拓扑定位。
///
/// **kind 与旧版一致**，仅把承载的 `u32` 序号换成 [`TopoRef`]（可携带稳定名）。
/// ⚠ 内核给的 `hash_code` 跨会话不稳定，不能当持久 id（设计文档 6.6）；
/// 稳定身份由 [`TopoName`] 承载。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SubShapeRef {
    /// 整个实体
    Whole,
    /// 某个面
    Face(TopoRef),
    /// 某条边
    Edge(TopoRef),
    /// 某个顶点
    Vertex(TopoRef),
}

/// 几何单元键：**哪个形状的哪个子形状**。
///
/// 关联更新引擎（设计文档 6.4）用它把"这次操作改了哪些面/边"映射到
/// "哪些二维图元与标注要跟着更新"。
///
/// 需要 `Ord`：关联更新索引（`core_doc::DependencyGraph`）用它做
/// `BTreeMap` / `BTreeSet` 的键，而 `BTree*` 要求键有序。
///
/// ## 为什么 JSON 里是**字符串**而不是对象
///
/// [`GeometryKey`] 要当 `BTreeMap` 的键用，而 **JSON 的对象键只能是字符串**。
/// 若按结构体序列化成 `{"shape": …, "sub": …}`，一旦把它放进 map，
/// `serde_json` 会直接报 `key must be a string`。
/// 因此这里做一层自定义 serde：**写成 `<shape-uuid>/<sub>`**，
/// 既当得了 map 键，又比嵌套对象更好读、更好 diff。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GeometryKey {
    /// 所属几何记录
    pub shape: ShapeId,
    /// 子形状定位
    pub sub: SubShapeRef,
}

impl GeometryKey {
    /// 构造"整个实体"的键。
    #[must_use]
    pub const fn whole(shape: ShapeId) -> Self {
        Self {
            shape,
            sub: SubShapeRef::Whole,
        }
    }

    /// 构造"某个面"的键（不带稳定名；旧调用点照旧编译）。
    #[must_use]
    pub const fn face(shape: ShapeId, index: u32) -> Self {
        Self {
            shape,
            sub: SubShapeRef::Face(TopoRef { index, name: None }),
        }
    }

    /// 构造"某条边"的键（不带稳定名；旧调用点照旧编译）。
    #[must_use]
    pub const fn edge(shape: ShapeId, index: u32) -> Self {
        Self {
            shape,
            sub: SubShapeRef::Edge(TopoRef { index, name: None }),
        }
    }

    /// 构造"某个顶点"的键（不带稳定名；旧调用点照旧编译）。
    #[must_use]
    pub const fn vertex(shape: ShapeId, index: u32) -> Self {
        Self {
            shape,
            sub: SubShapeRef::Vertex(TopoRef { index, name: None }),
        }
    }

    /// 构造"带稳定名的面"的键（M5 关联更新主用）。
    #[must_use]
    pub const fn face_named(shape: ShapeId, index: u32, name: TopoName) -> Self {
        Self {
            shape,
            sub: SubShapeRef::Face(TopoRef {
                index,
                name: Some(name),
            }),
        }
    }

    /// 构造"带稳定名的边"的键。
    #[must_use]
    pub const fn edge_named(shape: ShapeId, index: u32, name: TopoName) -> Self {
        Self {
            shape,
            sub: SubShapeRef::Edge(TopoRef {
                index,
                name: Some(name),
            }),
        }
    }

    /// 构造"带稳定名的顶点"的键。
    #[must_use]
    pub const fn vertex_named(shape: ShapeId, index: u32, name: TopoName) -> Self {
        Self {
            shape,
            sub: SubShapeRef::Vertex(TopoRef {
                index,
                name: Some(name),
            }),
        }
    }

    /// 给现有键补一个稳定名（`Whole` 保持 `Whole` 不变）。
    #[must_use]
    pub const fn with_topo_name(self, name: TopoName) -> Self {
        let sub = match self.sub {
            SubShapeRef::Whole => SubShapeRef::Whole,
            SubShapeRef::Face(r) => SubShapeRef::Face(TopoRef {
                index: r.index,
                name: Some(name),
            }),
            SubShapeRef::Edge(r) => SubShapeRef::Edge(TopoRef {
                index: r.index,
                name: Some(name),
            }),
            SubShapeRef::Vertex(r) => SubShapeRef::Vertex(TopoRef {
                index: r.index,
                name: Some(name),
            }),
        };
        Self {
            shape: self.shape,
            sub,
        }
    }

    /// 取该键绑定的稳定名（`Whole` / 旧格式键为 `None`）。
    #[must_use]
    pub const fn topo_name(&self) -> Option<&TopoName> {
        match &self.sub {
            SubShapeRef::Whole => None,
            SubShapeRef::Face(TopoRef { name, .. })
            | SubShapeRef::Edge(TopoRef { name, .. })
            | SubShapeRef::Vertex(TopoRef { name, .. }) => match name {
                Some(n) => Some(n),
                None => None,
            },
        }
    }

    /// 序列化用的稳定短串：`<shape-uuid>/<sub>`，
    /// 其中 `sub` 为 `whole` / `f<n>` / `e<n>` / `v<n>`；
    /// 若子形状带稳定名，则追加 `@<name>`（**旧格式原样保留**，见 [`GeometryKey::parse`]）。
    #[must_use]
    pub fn to_key_string(&self) -> String {
        let sub = match &self.sub {
            SubShapeRef::Whole => "whole".to_string(),
            SubShapeRef::Face(r) => sub_to_string('f', r),
            SubShapeRef::Edge(r) => sub_to_string('e', r),
            SubShapeRef::Vertex(r) => sub_to_string('v', r),
        };
        format!("{}/{}", self.shape, sub)
    }

    /// 解析 [`GeometryKey::to_key_string`] 产出的串。
    ///
    /// **向后兼容**：无 `@` → `name: None`（旧格式原样读回）；
    /// 有 `@` → 解析 [`TopoName`]，坏载荷一律 `Err`（绝不静默吞掉）。
    ///
    /// 解析失败返回一句可读的说明（供 serde 报错与排查用）。
    pub fn parse(s: &str) -> Result<Self, String> {
        let (shape_part, sub_part) = s
            .split_once('/')
            .ok_or_else(|| format!("几何键缺少 `/` 分隔符：{s}"))?;
        let shape = Uuid::parse_str(shape_part)
            .map_err(|e| format!("几何键里的 shape 不是合法 UUID（{shape_part}）：{e}"))?;
        let sub = if sub_part == "whole" {
            SubShapeRef::Whole
        } else {
            let (sub_head, name_part) = match sub_part.split_once('@') {
                Some((head, named)) => (head, Some(named)),
                None => (sub_part, None),
            };
            let (tag, digits) = sub_head.split_at(sub_head.len().min(1));
            let index: u32 = digits
                .parse()
                .map_err(|e| format!("几何键子形状序号非法（{sub_part}）：{e}"))?;
            let name = match name_part {
                Some(named) => Some(decode_topo_name(named)?),
                None => None,
            };
            let topo = TopoRef { index, name };
            match tag {
                "f" => SubShapeRef::Face(topo),
                "e" => SubShapeRef::Edge(topo),
                "v" => SubShapeRef::Vertex(topo),
                _ => return Err(format!("几何键子形状前缀非法：{sub_part}")),
            }
        };
        Ok(Self {
            shape: ShapeId::from_uuid(shape),
            sub,
        })
    }
}

/// 子形状短串：`<tag><index>`，带名时追加 `@<name>`。
fn sub_to_string(tag: char, r: &TopoRef) -> String {
    match &r.name {
        None => format!("{tag}{}", r.index),
        Some(name) => format!("{tag}{}@{}", r.index, encode_topo_name(name)),
    }
}

/// [`TopoName`] 的紧凑编码：`<k>:<p0>,<p1>,<p2>:<d0>,<d1>,<d2>:<size>`，
/// 其中 `k ∈ {F,E,V}`。
fn encode_topo_name(n: &TopoName) -> String {
    let k = match n.kind {
        TopoKind::Face => 'F',
        TopoKind::Edge => 'E',
        TopoKind::Vertex => 'V',
    };
    format!(
        "{k}:{},{},{}:{},{},{}:{}",
        n.pos_q[0], n.pos_q[1], n.pos_q[2], n.dir_q[0], n.dir_q[1], n.dir_q[2], n.size_q
    )
}

/// [`encode_topo_name`] 的逆：任何字段缺失 / 越界 / 非数字都返回 `Err`。
fn decode_topo_name(s: &str) -> Result<TopoName, String> {
    let mut parts = s.split(':');
    let kind_part = parts
        .next()
        .ok_or_else(|| format!("拓扑名缺少种类段：{s}"))?;
    let pos_part = parts
        .next()
        .ok_or_else(|| format!("拓扑名缺少位置段：{s}"))?;
    let dir_part = parts
        .next()
        .ok_or_else(|| format!("拓扑名缺少方向段：{s}"))?;
    let size_part = parts
        .next()
        .ok_or_else(|| format!("拓扑名缺少尺度段：{s}"))?;
    if parts.next().is_some() {
        return Err(format!("拓扑名段数过多：{s}"));
    }
    let kind = match kind_part {
        "F" => TopoKind::Face,
        "E" => TopoKind::Edge,
        "V" => TopoKind::Vertex,
        _ => return Err(format!("拓扑名种类非法（{kind_part}）：{s}")),
    };
    let pos_q = parse_triple_i32(pos_part, "位置", s)?;
    let dir_q = parse_triple_i16(dir_part, "方向", s)?;
    let size_q: i64 = size_part
        .parse()
        .map_err(|e| format!("拓扑名尺度非法（{size_part}）：{e}"))?;
    // `pos_q` 是"相对形状包围盒 min 的局部坐标"（面质心必在包围盒内 → 必 ≥ 0），
    // `size_q` 是面积 / 长度（必 ≥ 0）；只有 `dir_q` 允许为负。负值 ⇒ 载荷损坏，
    // 必须 `Err`（绝不静默收下一个不可能的指纹）。
    if pos_q.iter().any(|&v| v < 0) {
        return Err(format!("拓扑名位置分量不得为负（{pos_part}）：{s}"));
    }
    if size_q < 0 {
        return Err(format!("拓扑名尺度不得为负（{size_part}）：{s}"));
    }
    Ok(TopoName {
        kind,
        pos_q,
        dir_q,
        size_q,
    })
}

/// 解析 `a,b,c` 三整数（`i32`），分量个数不为 3 即 `Err`。
fn parse_triple_i32(s: &str, what: &str, whole: &str) -> Result<[i32; 3], String> {
    let v: Vec<i32> = s
        .split(',')
        .map(str::parse::<i32>)
        .collect::<Result<_, _>>()
        .map_err(|e| format!("拓扑名{what}分量非法（{s}）：{e}"))?;
    if v.len() != 3 {
        return Err(format!("拓扑名{what}分量个数应为 3（{whole}）"));
    }
    Ok([v[0], v[1], v[2]])
}

/// 解析 `a,b,c` 三整数（`i16`），分量个数不为 3 即 `Err`。
fn parse_triple_i16(s: &str, what: &str, whole: &str) -> Result<[i16; 3], String> {
    let v: Vec<i16> = s
        .split(',')
        .map(str::parse::<i16>)
        .collect::<Result<_, _>>()
        .map_err(|e| format!("拓扑名{what}分量非法（{s}）：{e}"))?;
    if v.len() != 3 {
        return Err(format!("拓扑名{what}分量个数应为 3（{whole}）"));
    }
    Ok([v[0], v[1], v[2]])
}

impl fmt::Display for GeometryKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_key_string())
    }
}

impl Serialize for GeometryKey {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_key_string())
    }
}

impl<'de> Deserialize<'de> for GeometryKey {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::parse(&s).map_err(<D::Error as serde::de::Error>::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_time_ordered() {
        let a = ElementId::new();
        let b = ElementId::new();
        assert_ne!(a, b, "持久 id 必须唯一");
    }

    #[test]
    fn ids_roundtrip_through_json() {
        let id = ElementId::new();
        let s = serde_json::to_string(&id).unwrap();
        let back: ElementId = serde_json::from_str(&s).unwrap();
        assert_eq!(id, back);
    }

    #[test]
    fn geometry_keys_are_hashable_by_shape_and_sub() {
        let shape = ShapeId::new();
        assert_ne!(GeometryKey::face(shape, 0), GeometryKey::edge(shape, 0));
        assert_eq!(GeometryKey::whole(shape), GeometryKey::whole(shape));
    }

    #[test]
    fn geometry_keys_roundtrip_through_their_string_form() {
        let shape = ShapeId::new();
        let keys = [
            GeometryKey::whole(shape),
            GeometryKey::face(shape, 3),
            GeometryKey::edge(shape, 0),
        ];
        for key in keys {
            let text = key.to_key_string();
            assert_eq!(
                GeometryKey::parse(&text).unwrap(),
                key,
                "短串往返失败：{text}"
            );

            let json = serde_json::to_string(&key).unwrap();
            assert!(json.starts_with('"'), "JSON 里应是字符串：{json}");
            assert_eq!(serde_json::from_str::<GeometryKey>(&json).unwrap(), key);
        }
    }

    #[test]
    fn geometry_keys_work_as_json_map_keys() {
        // 这是当初把 GeometryKey 改成字符串形态的直接原因：
        // 对象形态无法当 JSON 的 map 键。
        let shape = ShapeId::new();
        let mut map = std::collections::BTreeMap::new();
        map.insert(GeometryKey::face(shape, 1), "a".to_string());
        let json = serde_json::to_string(&map).unwrap();
        let back: std::collections::BTreeMap<GeometryKey, String> =
            serde_json::from_str(&json).unwrap();
        assert_eq!(back, map);
    }

    #[test]
    fn malformed_key_strings_are_rejected() {
        assert!(GeometryKey::parse("no-separator").is_err());
        assert!(GeometryKey::parse("not-a-uuid/whole").is_err());
        assert!(GeometryKey::parse(&format!("{}/x1", ShapeId::new())).is_err());
        assert!(GeometryKey::parse(&format!("{}/f", ShapeId::new())).is_err());
    }

    // ---- B3-b：稳定拓扑命名（TopoName / TopoRef / SubShapeRef） ----

    #[test]
    fn subshape_ref_old_strings_still_parse() {
        // 向后兼容钉子：旧格式（无 `@`）必须原样读回，`name == None`。
        let shape = ShapeId::new();
        let cases = [
            (format!("{shape}/whole"), SubShapeRef::Whole),
            (
                format!("{shape}/f3"),
                SubShapeRef::Face(TopoRef {
                    index: 3,
                    name: None,
                }),
            ),
            (
                format!("{shape}/e0"),
                SubShapeRef::Edge(TopoRef {
                    index: 0,
                    name: None,
                }),
            ),
            (
                format!("{shape}/v7"),
                SubShapeRef::Vertex(TopoRef {
                    index: 7,
                    name: None,
                }),
            ),
        ];
        for (text, expect) in cases {
            let key = GeometryKey::parse(&text).unwrap();
            assert_eq!(key.sub, expect, "旧格式解析结果不符：{text}");
            assert!(key.topo_name().is_none(), "旧格式不该带名：{text}");
            assert_eq!(key.to_key_string(), text, "旧格式必须原样读回：{text}");
        }

        // 旧构造器（签名不变）产出的键，其短串就是旧格式。
        assert_eq!(
            GeometryKey::face(shape, 3).to_key_string(),
            format!("{shape}/f3")
        );
    }

    #[test]
    fn subshape_ref_named_strings_roundtrip() {
        let shape = ShapeId::new();
        let name = TopoName::of_face([12.5, 0.0, 40.0], [0.0, 0.0, 1.0], 2500.0, [0.0, 0.0, 0.0]);
        let key = GeometryKey::face_named(shape, 3, name);
        assert_eq!(key.topo_name(), Some(&name));
        assert_eq!(GeometryKey::face(shape, 3).with_topo_name(name), key);

        let text = key.to_key_string();
        assert!(text.contains('@'), "带名键应含 `@`：{text}");
        assert_eq!(
            GeometryKey::parse(&text).unwrap(),
            key,
            "短串往返失败：{text}"
        );

        let json = serde_json::to_string(&key).unwrap();
        assert_eq!(
            serde_json::from_str::<GeometryKey>(&json).unwrap(),
            key,
            "JSON 往返失败：{json}"
        );
    }

    #[test]
    fn malformed_named_key_is_rejected() {
        let shape = ShapeId::new();
        // 坏 `@` 载荷：非编码串 / 分量不足 / 种类非法 / 缺段，一律 Err。
        assert!(GeometryKey::parse(&format!("{shape}/f3@垃圾")).is_err());
        assert!(GeometryKey::parse(&format!("{shape}/f3@F:1,2:1,2,3:9")).is_err());
        assert!(GeometryKey::parse(&format!("{shape}/f3@X:1,2,3:1,2,3:9")).is_err());
        assert!(GeometryKey::parse(&format!("{shape}/f3@F:1,2,3:1,2,3")).is_err());
        assert!(GeometryKey::parse(&format!("{shape}/f3@F:1,2,3:1,2,3:9:0")).is_err());
    }

    #[test]
    fn topo_name_repeated_construction_is_identical() {
        let a = TopoName::of_face([10.0, 20.0, 30.0], [0.0, 0.0, 1.0], 2500.0, [0.0, 0.0, 0.0]);
        let b = TopoName::of_face([10.0, 20.0, 30.0], [0.0, 0.0, 1.0], 2500.0, [0.0, 0.0, 0.0]);
        assert_eq!(a, b, "同一面重复构造必须完全相等");
        assert!(a.exact_match(&b));
        assert_eq!(a.distance(&b), 0.0);
    }

    #[test]
    fn topo_name_exact_match_within_tolerance() {
        let a = TopoName::of_face([10.0, 20.0, 30.0], [0.0, 0.0, 1.0], 2500.0, [0.0, 0.0, 0.0]);
        // 位置差 0.4 mm：量化后落格差 1（0.5 mm 步长）→ 结构不等但仍在 exact_match 容差内。
        let b = TopoName::of_face([10.4, 20.0, 30.0], [0.0, 0.0, 1.0], 2500.0, [0.0, 0.0, 0.0]);
        assert_ne!(a, b);
        assert!(a.exact_match(&b), "一格内的量化差应视为精确匹配");
        // 差 2 mm：超出容差。
        let c = TopoName::of_face([12.0, 20.0, 30.0], [0.0, 0.0, 1.0], 2500.0, [0.0, 0.0, 0.0]);
        assert!(!a.exact_match(&c));
    }

    #[test]
    fn topo_name_distance_is_zero_only_for_identical_and_grows() {
        let base = TopoName::of_face([0.0, 0.0, 0.0], [0.0, 0.0, 1.0], 1000.0, [0.0, 0.0, 0.0]);
        assert_eq!(base.distance(&base), 0.0);

        // 平移 300 mm（> 100 mm 参照）→ 分量距离 3.0，超过可信阈值。
        let far = TopoName::of_face([300.0, 0.0, 0.0], [0.0, 0.0, 1.0], 1000.0, [0.0, 0.0, 0.0]);
        assert!(base.distance(&far) > MAX_TOPO_DISTANCE, "远距离面应超阈值");

        // 法向转 90°：弦距 √2 ≈ 1.414 → 分量 7.07，远超阈值。
        let turned = TopoName::of_face([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], 1000.0, [0.0, 0.0, 0.0]);
        assert!(base.distance(&turned) > MAX_TOPO_DISTANCE, "转向面应超阈值");
        // 近距离且同向 → 距离明显更小、且在阈值内。
        let near = TopoName::of_face([5.0, 0.0, 0.0], [0.0, 0.0, 1.0], 1000.0, [0.0, 0.0, 0.0]);
        assert!(base.distance(&near) < MAX_TOPO_DISTANCE);
        assert!(base.distance(&near) < base.distance(&far));
    }

    #[test]
    fn topo_name_kind_mismatch_is_untrusted() {
        let f = TopoName::of_face([0.0; 3], [0.0, 0.0, 1.0], 1.0, [0.0; 3]);
        let e = TopoName::of_edge([0.0; 3], [0.0, 0.0, 1.0], 1.0, [0.0; 3]);
        assert!(!f.exact_match(&e));
        assert!(f.distance(&e) > MAX_TOPO_DISTANCE);
    }

    #[test]
    fn topo_name_is_translation_invariant_via_bbox_min() {
        // 世界坐标整体平移，但 bbox_min 同步平移 → 局部量化坐标不变。
        let n1 = TopoName::of_face([10.0, 20.0, 30.0], [0.0, 0.0, 1.0], 500.0, [0.0, 0.0, 0.0]);
        let n2 = TopoName::of_face(
            [110.0, 120.0, 130.0],
            [0.0, 0.0, 1.0],
            500.0,
            [100.0, 100.0, 100.0],
        );
        assert_eq!(n1, n2, "整体平移不应改变面的身份");
    }

    #[test]
    fn negative_topo_name_components_are_rejected() {
        let shape = ShapeId::new();
        // 位置分量不得为负：面质心必在包围盒内（局部坐标 ≥ 0）。
        assert!(GeometryKey::parse(&format!("{shape}/f3@F:-1,2,3:1,2,3:9")).is_err());
        // 尺度（面积 / 长度）不得为负：负面积是不可能指纹。
        assert!(GeometryKey::parse(&format!("{shape}/f3@F:1,2,3:1,2,3:-9")).is_err());
        // 方向分量允许为负、位置 / 尺度为 0 合法 → 必须能正常解析。
        assert!(
            GeometryKey::parse(&format!("{shape}/f3@F:0,2,3:-1,2,-3:9")).is_ok(),
            "方向分量允许为负、位置/尺度为 0 合法"
        );
    }
}
