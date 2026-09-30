//! 几何记录：文档里几何的**持久形态**。
//!
//! 设计文档 4.2 的核心决策：**不自研 B-Rep 二进制格式**，而是把 OCCT 原生的
//! BREP 文本（可读、可 diff）内嵌进 `.bimdoc` 的 `geom/<shapeid>.brep`。
//! 打开文档时**按需**把文本读成内核句柄（惰性重建，见 3.3）。

use serde::{Deserialize, Serialize};

use crate::ids::ShapeId;

/// 几何记录的存储格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GeomFormat {
    /// OCCT **BREP 文本**格式（首选：可读、可 diff）
    BrepText,
    /// OCCT BREP 二进制格式
    BrepBinary,
    /// STEP（对外交付格式）
    Step,
}

/// 一份持久化的几何记录。
///
/// **这只是数据**：它不包含任何几何算法，也不持有任何内核句柄。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeometryRecord {
    /// 持久 id（与文件名 `geom/<shapeid>.brep` 对应）
    pub shape_id: ShapeId,
    /// 存储格式
    pub format: GeomFormat,
    /// 几何正文
    pub text: String,
}

impl GeometryRecord {
    /// 由 BREP 文本构造。
    #[must_use]
    pub fn brep(shape_id: ShapeId, text: impl Into<String>) -> Self {
        Self {
            shape_id,
            format: GeomFormat::BrepText,
            text: text.into(),
        }
    }

    /// 由 STEP 文本构造。
    #[must_use]
    pub fn step(shape_id: ShapeId, text: impl Into<String>) -> Self {
        Self {
            shape_id,
            format: GeomFormat::Step,
            text: text.into(),
        }
    }

    /// 正文长度（字节）。
    #[must_use]
    pub fn byte_len(&self) -> usize {
        self.text.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_keep_format_and_payload() {
        let id = ShapeId::new();
        let r = GeometryRecord::brep(id, "CASCADE Topology V3");
        assert_eq!(r.format, GeomFormat::BrepText);
        assert_eq!(r.shape_id, id);
        assert_eq!(r.byte_len(), 19);
    }

    #[test]
    fn records_roundtrip_through_json() {
        let r = GeometryRecord::step(ShapeId::new(), "ISO-10303-21;");
        let s = serde_json::to_string(&r).unwrap();
        assert_eq!(serde_json::from_str::<GeometryRecord>(&s).unwrap(), r);
    }
}
