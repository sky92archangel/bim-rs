//! 几何装载器：把**持久化的几何记录**装回成**会话句柄**。
//!
//! 这是设计文档 3.3「惰性重建」后半程的落地：
//!
//! ```text
//! 文档里的 GeometryRecord（BREP 文本，可存盘）
//!         │  ShapeLoader::load（本模块）
//!         ▼
//! 内核会话句柄 ShapeHandle（arena u32，进程内有效）
//! ```
//!
//! ## 为什么分派逻辑放在 `core-app` 而不是内核里
//!
//! `GeomFormat`（`BrepText` / `BrepBinary` / `Step`）是**数据层**的概念，
//! 定义在 `core-model`。而依赖方向铁律规定 `core-kernel` 不许依赖 `core-model`
//! ——所以"看格式选内核接口"这件事只能落在同时看得见两边的 `core-app`。
//!
//! ## 一阶段只支持 BREP 文本
//!
//! | 格式 | 一阶段 | 原因 |
//! |---|---|---|
//! | `BrepText` | ✅ | 内核门面已提供 `import_brep` / `export_brep` |
//! | `BrepBinary` | ❌ 明确报错 | `occt-wasm` 只暴露 BREP **文本**读写 |
//! | `Step` | ❌ 明确报错 | STEP 读写属**能力④互操作**（设计文档 7.5），M4 才接线 |
//!
//! 不支持就**明确报错**，绝不退回"造一个占位盒子"——那会让用户以为
//! 文档里的几何真的被读出来了。

use core_kernel::{Kernel, KernelError, KernelResult, ShapeHandle};
use core_model::{GeomFormat, GeometryRecord};

use crate::geometry_service::ShapeLoader;

/// 按 `GeomFormat` 分派的装载器。
///
/// 与内核后端**无关**：真实后端就读真 BREP，占位后端读自带的自描述文本格式。
/// 所以它对两种后端都成立，也就没有理由在默认构造时缺席。
#[derive(Debug, Default, Clone, Copy)]
pub struct BrepShapeLoader;

impl BrepShapeLoader {
    /// 新建（无状态，纯函数式）。
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl ShapeLoader for BrepShapeLoader {
    fn load(&mut self, kernel: &mut Kernel, record: &GeometryRecord) -> KernelResult<ShapeHandle> {
        match record.format {
            GeomFormat::BrepText => kernel.import_brep(&record.text),
            GeomFormat::BrepBinary => Err(KernelError::Unsupported(
                "BREP 二进制格式暂无内核接口：occt-wasm 只暴露 BREP 文本读写；\
                 请改用 GeomFormat::BrepText 存盘",
            )),
            GeomFormat::Step => Err(KernelError::Unsupported(
                "STEP 读取属能力④互操作（设计文档 7.5），M4 才接线；\
                 一阶段的持久化几何只走 BREP 文本",
            )),
        }
    }
}

/// 一阶段支持的格式（供 UI 提示与自检）。
pub const SUPPORTED_LOAD_FORMATS: &[GeomFormat] = &[GeomFormat::BrepText];

/// 某格式当前是否可装载。
#[must_use]
pub fn can_load(format: GeomFormat) -> bool {
    SUPPORTED_LOAD_FORMATS.contains(&format)
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_model::ShapeId;

    #[test]
    fn only_brep_text_is_loadable_in_phase_one() {
        assert!(can_load(GeomFormat::BrepText));
        assert!(!can_load(GeomFormat::BrepBinary));
        assert!(!can_load(GeomFormat::Step));
        assert_eq!(SUPPORTED_LOAD_FORMATS.len(), 1);
    }

    #[test]
    fn step_is_refused_with_a_pointer_to_the_right_milestone() {
        let mut kernel = Kernel::mock();
        let record = GeometryRecord::step(ShapeId::new(), "ISO-10303-21;");
        let err = BrepShapeLoader.load(&mut kernel, &record).unwrap_err();
        assert!(
            matches!(err, KernelError::Unsupported(msg) if msg.contains("M4")),
            "错误信息应指向 M4，实得 {err:?}"
        );
    }

    #[test]
    fn brep_binary_is_refused_rather_than_guessed() {
        let mut kernel = Kernel::mock();
        let record = GeometryRecord {
            shape_id: ShapeId::new(),
            format: GeomFormat::BrepBinary,
            text: "binary blob".to_string(),
        };
        let err = BrepShapeLoader.load(&mut kernel, &record).unwrap_err();
        assert!(matches!(err, KernelError::Unsupported(_)));
    }

    #[test]
    fn a_real_brep_text_record_round_trips_through_the_loader() {
        // 占位后端用自描述文本；真实后端用 OCCT BREP 文本。
        // 这里用占位后端验证"导出 → 装载"这条链在形态上成立。
        let mut kernel = Kernel::mock();
        let handle = kernel.make_box(2.0, 3.0, 4.0).unwrap();
        let text = kernel.export_brep(handle).unwrap();

        let record = GeometryRecord::brep(ShapeId::new(), text);
        let restored = BrepShapeLoader.load(&mut kernel, &record).unwrap();

        assert!(kernel.is_alive(restored));
        assert!((kernel.volume(restored).unwrap() - 24.0).abs() < 1e-9);
    }

    #[test]
    fn a_malformed_brep_text_is_surfaced_not_swallowed() {
        let mut kernel = Kernel::mock();
        let record = GeometryRecord::brep(ShapeId::new(), "这不是 BREP");
        // 占位后端会拒绝无法解析的文本；真实后端会拒绝非 CASCADE 文本。
        // 两种后端都不许"成功返回一个假句柄"。
        assert!(BrepShapeLoader.load(&mut kernel, &record).is_err());
    }
}
