//! 模块契约（设计文档 9.2）。
//!
//! ## 一个模块长什么样
//!
//! ```text
//! Module
//!   ├─ manifest()  → ModuleManifest   我是谁、要哪个 SDK 版本
//!   └─ register()  → 往 Registry 里塞：
//!                      ├─ 品类（Category）
//!                      ├─ 属性模式（PropertySchema）
//!                      ├─ 元素工厂（ElementFactory）
//!                      ├─ 标注规则（AnnotationRule）
//!                      ├─ 几何算子（GeometryOp）
//!                      └─ 专业图层（LayerSpec）
//! ```
//!
//! 模块**不实现任何算法基础设施**：几何算子在 `core-kernel`、图纸语义在
//! `core-doc`、事务在 `core-model`。模块只负责"定义一个专业构件是什么、
//! 画出来该什么样、标注该标什么"。

use serde::{Deserialize, Serialize};

use crate::error::{SdkError, SdkResult};
use crate::registry::Registry;

/// 当前 SDK 的 API 版本。
///
/// **任何时候改动 trait 签名都要 +1**——这是唯一能防住"模块悄悄与 Core 不兼容"
/// 的机制。
pub const SDK_API_VERSION: u32 = 1;

/// 模块标识（`"ceec.power.electrical"` 这类反向域名风格）。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ModuleId(String);

impl ModuleId {
    /// 由字符串构造。
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// 取字符串。
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 是否形如反向域名（至少两个点分段）。
    ///
    /// 不做强制，只在 [`ModuleManifest::validate`] 里作为提醒。
    #[must_use]
    pub fn looks_namespaced(&self) -> bool {
        self.0.split('.').filter(|s| !s.is_empty()).count() >= 2
    }
}

impl std::fmt::Display for ModuleId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for ModuleId {
    fn from(v: &str) -> Self {
        Self::new(v)
    }
}

/// 模块清单。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleManifest {
    /// 模块 id
    pub id: ModuleId,
    /// 显示名
    pub name: String,
    /// 模块版本
    pub version: String,
    /// 要求的 SDK API 版本
    pub api_version: u32,
    /// 说明
    pub description: String,
    /// 提供方
    pub vendor: String,
}

impl ModuleManifest {
    /// 构造清单，`api_version` 默认取当前 SDK 版本。
    #[must_use]
    pub fn new(
        id: impl Into<ModuleId>,
        name: impl Into<String>,
        version: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            version: version.into(),
            api_version: SDK_API_VERSION,
            description: String::new(),
            vendor: String::new(),
        }
    }

    /// 说明（链式）。
    #[must_use]
    pub fn with_description(mut self, text: impl Into<String>) -> Self {
        self.description = text.into();
        self
    }

    /// 提供方（链式）。
    #[must_use]
    pub fn with_vendor(mut self, vendor: impl Into<String>) -> Self {
        self.vendor = vendor.into();
        self
    }

    /// 声明要求的 SDK API 版本（链式）——**测试不兼容处理时用**。
    #[must_use]
    pub const fn requiring_api(mut self, api_version: u32) -> Self {
        self.api_version = api_version;
        self
    }

    /// 校验清单本身是否合法。
    pub fn validate(&self) -> SdkResult<()> {
        if self.id.as_str().is_empty() {
            return Err(SdkError::InvalidManifest("模块 id 不能为空".to_string()));
        }
        if self.name.trim().is_empty() {
            return Err(SdkError::InvalidManifest("模块名不能为空".to_string()));
        }
        if self.version.trim().is_empty() {
            return Err(SdkError::InvalidManifest("模块版本不能为空".to_string()));
        }
        if self.api_version != SDK_API_VERSION {
            return Err(SdkError::ApiVersionMismatch {
                required: self.api_version,
                current: SDK_API_VERSION,
            });
        }
        Ok(())
    }
}

/// 一个专业模块。
///
/// **实现要求**：`register` 必须是**纯粹往注册表里塞东西**的动作——
/// 不做 I/O、不读数、不启线程。原因是注册发生在启动期，
/// 一个慢模块会拖死整个加载流程。
pub trait Module: Send + Sync {
    /// 我是谁。
    fn manifest(&self) -> ModuleManifest;

    /// 我把什么塞进 Core。
    fn register(&self, registry: &mut Registry) -> SdkResult<()>;

    /// 模块自检（可选）。默认什么都不查。
    ///
    /// 用途：给 SDK 做"体检"——把刚注册进去的东西取出来验一遍，
    /// 确认注册表真的收到了（而不是登记时写了错名字）。
    fn self_check(&self, _registry: &Registry) -> SdkResult<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一个最小模块，只登记自己的清单。
    struct Minimal;

    impl Module for Minimal {
        fn manifest(&self) -> ModuleManifest {
            ModuleManifest::new("demo.minimal", "最小模块", "0.1.0")
        }
        fn register(&self, registry: &mut Registry) -> SdkResult<()> {
            registry.register_module(&self.manifest())
        }
    }

    /// 一个声明了错误 API 版本的模块。
    struct Legacy;
    impl Module for Legacy {
        fn manifest(&self) -> ModuleManifest {
            ModuleManifest::new("demo.legacy", "老模块", "0.0.1").requiring_api(0)
        }
        fn register(&self, _registry: &mut Registry) -> SdkResult<()> {
            Ok(())
        }
    }

    #[test]
    fn manifest_defaults_to_the_current_api_version() {
        let m = ModuleManifest::new("a.b", "N", "1.0");
        assert_eq!(m.api_version, SDK_API_VERSION);
        assert!(m.validate().is_ok());
    }

    #[test]
    fn mismatched_api_version_is_rejected() {
        let m = ModuleManifest::new("a.b", "N", "1.0").requiring_api(999);
        assert!(matches!(
            m.validate(),
            Err(SdkError::ApiVersionMismatch { required: 999, .. })
        ));
    }

    #[test]
    fn empty_fields_are_rejected() {
        assert!(ModuleManifest::new("a.b", "", "1.0").validate().is_err());
        assert!(ModuleManifest::new("", "N", "1.0").validate().is_err());
        assert!(ModuleManifest::new("a.b", "N", "").validate().is_err());
    }

    #[test]
    fn module_ids_report_namespacing() {
        assert!(ModuleId::new("ceec.power.electrical").looks_namespaced());
        assert!(!ModuleId::new("beam").looks_namespaced());
    }

    #[test]
    fn a_minimal_module_can_register_itself() {
        let mut reg = Registry::new();
        reg.install(&Minimal).unwrap();
        assert_eq!(reg.module_count(), 1);
        assert!(reg.module("demo.minimal").is_some());
    }

    #[test]
    fn installing_a_legacy_module_fails_before_registration() {
        let mut reg = Registry::new();
        let err = reg.install(&Legacy).unwrap_err();
        assert!(matches!(err, SdkError::ApiVersionMismatch { .. }));
        assert_eq!(reg.module_count(), 0, "版本不符的模块不得进入注册表");
    }

    #[test]
    fn installing_the_same_module_twice_is_rejected() {
        let mut reg = Registry::new();
        reg.install(&Minimal).unwrap();
        assert!(matches!(
            reg.install(&Minimal),
            Err(SdkError::DuplicateModule(_))
        ));
    }

    #[test]
    fn default_self_check_passes() {
        let reg = Registry::new();
        assert!(Minimal.self_check(&reg).is_ok());
    }
}
