//! 元素工厂（设计文档 9.2）—— "这个专业构件长什么样、默认属性是什么"。
//!
//! ## 工厂**不建几何**
//!
//! 这一点必须说清楚，否则很容易把专业逻辑全塞进工厂里：
//!
//! | 工厂负责 | 工厂**不**负责 |
//! |---|---|
//! | 品类 / 显示名 / 属性模式 | 几何构造（那是 [`crate::GeometryOp`] 的 recipe）|
//! | 从参数算默认属性 | 画到图纸上（那是 `core-doc`）|
//! | 生成**语义元素**（`Element`）| 拾取、消隐、投影（那是 `core-kernel`）|
//!
//! 所以 [`ElementFactory::create`] 返回的是一个 **没有几何** 的
//! [`core_model::Element`]——它的 `geometry_ref` 由后续的几何算子 recipe
//! 跑完后再回填。

use core_model::{Category, Element, PropertyMap, PropertySchema, Value};
use serde::{Deserialize, Serialize};

use crate::error::SdkResult;

/// 一次"新建构件"的请求。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateRequest {
    /// 用户给的构件名（可为空，由工厂补默认名）
    pub name: String,
    /// 定位点（mm，Z 轴朝上右手系）——通常是用户在三维视图里点的那个点
    pub anchor: [f64; 3],
    /// 用户填的参数（键值对由工厂自行解释）
    pub params: PropertyMap,
    /// 用户指定的图层名（`None` = 由制图标准决定）
    pub layer: Option<String>,
}

impl CreateRequest {
    /// 以名字构造一个位于原点的请求。
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            anchor: [0.0, 0.0, 0.0],
            params: PropertyMap::new(""),
            layer: None,
        }
    }

    /// 指定定位点（链式）。
    #[must_use]
    pub fn at(mut self, anchor: [f64; 3]) -> Self {
        self.anchor = anchor;
        self
    }

    /// 指定参数集（链式）。
    #[must_use]
    pub fn with_params(mut self, params: PropertyMap) -> Self {
        self.params = params;
        self
    }

    /// 加一个参数（链式）。
    #[must_use]
    pub fn with_param(mut self, key: impl Into<String>, value: Value) -> Self {
        self.params.set(key, value);
        self
    }

    /// 指定图层（链式）。
    #[must_use]
    pub fn on_layer(mut self, layer: impl Into<String>) -> Self {
        self.layer = Some(layer.into());
        self
    }

    /// 取一个浮点参数。
    #[must_use]
    pub fn param_f64(&self, key: &str) -> Option<f64> {
        self.params.get_float(key)
    }

    /// 取一个文本参数。
    #[must_use]
    pub fn param_text(&self, key: &str) -> Option<&str> {
        self.params.get_text(key)
    }
}

/// 元素工厂。
pub trait ElementFactory: Send + Sync {
    /// 我造哪个品类（须与 [`crate::Registry`] 里登记的品类名一致）。
    fn category(&self) -> &str;

    /// 界面显示名（如 `"动力电缆桥架"`）。
    fn display_name(&self) -> &str;

    /// 我遵循的属性模式（`None` 表示无属性模式）。
    fn schema(&self) -> Option<PropertySchema> {
        None
    }

    /// 造一个语义元素。
    ///
    /// 返回的 `Element` **不带几何**（见模块文档）。名字为空时应由实现补默认名。
    fn create(&self, request: &CreateRequest) -> SdkResult<Element>;

    /// 新建时的默认参数（界面预填用）。
    fn default_params(&self) -> PropertyMap {
        PropertyMap::new("")
    }
}

/// 便捷：按工厂的品类与参数造元素，并自动挂上属性模式名。
///
/// 工厂实现里调它，可以少写几行样板：
///
/// ```ignore
/// fn create(&self, req: &CreateRequest) -> SdkResult<Element> {
///     let mut e = crate::factory::build_element(self.category(), req, "动力电缆桥架");
///     e.properties.set("width", Value::Float(req.param_f64("width").unwrap_or(600.0)));
///     Ok(e)
/// }
/// ```
#[must_use]
pub fn build_element(category: &str, request: &CreateRequest, fallback_name: &str) -> Element {
    let name = if request.name.trim().is_empty() {
        fallback_name.to_string()
    } else {
        request.name.clone()
    };
    let mut element = Element::new(Category::new(category), name);
    element.properties = PropertyMap::new(category);
    element
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_model::{FieldDef, FieldType};

    struct TrayFactory;

    impl ElementFactory for TrayFactory {
        fn category(&self) -> &'static str {
            "POWER_CABLE_TRAY"
        }

        fn display_name(&self) -> &'static str {
            "动力电缆桥架"
        }

        fn schema(&self) -> Option<PropertySchema> {
            Some(
                PropertySchema::new("POWER_CABLE_TRAY")
                    .with_field(FieldDef::new("width", FieldType::Float, "宽").with_unit("mm"))
                    .with_field(FieldDef::new("material", FieldType::Text, "材质")),
            )
        }

        fn create(&self, request: &CreateRequest) -> SdkResult<Element> {
            let mut e = build_element(self.category(), request, "动力电缆桥架");
            e.properties.set(
                "width",
                Value::Float(request.param_f64("width").unwrap_or(600.0)),
            );
            e.properties.set(
                "material",
                Value::Text(
                    request
                        .param_text("material")
                        .unwrap_or("热镀锌钢")
                        .to_string(),
                ),
            );
            Ok(e)
        }

        fn default_params(&self) -> PropertyMap {
            PropertyMap::new("POWER_CABLE_TRAY")
                .with("width", Value::Float(600.0))
                .with("material", Value::Text("热镀锌钢".to_string()))
        }
    }

    #[test]
    fn create_request_is_built_with_chainable_helpers() {
        let r = CreateRequest::new("桥架-1")
            .at([1000.0, 2000.0, 3000.0])
            .with_param("width", Value::Float(400.0))
            .on_layer("TRAY-电缆桥架");

        assert_eq!(r.anchor, [1000.0, 2000.0, 3000.0]);
        assert_eq!(r.param_f64("width"), Some(400.0));
        assert_eq!(r.layer.as_deref(), Some("TRAY-电缆桥架"));
    }

    #[test]
    fn factory_creates_a_geometryless_element() {
        let e = TrayFactory.create(&CreateRequest::new("桥架-1")).unwrap();
        assert_eq!(e.category.as_str(), "POWER_CABLE_TRAY");
        assert!(
            !e.has_geometry(),
            "工厂只造语义元素，几何由算子 recipe 回填"
        );
        assert_eq!(e.name, "桥架-1");
    }

    #[test]
    fn empty_name_falls_back_to_the_display_name() {
        let e = TrayFactory.create(&CreateRequest::new("   ")).unwrap();
        assert_eq!(e.name, "动力电缆桥架");
    }

    #[test]
    fn factory_applies_parameters_and_defaults() {
        let custom = TrayFactory
            .create(&CreateRequest::new("x").with_param("width", Value::Float(400.0)))
            .unwrap();
        assert_eq!(custom.properties.get_float("width"), Some(400.0));

        let defaulted = TrayFactory.create(&CreateRequest::new("x")).unwrap();
        assert_eq!(defaulted.properties.get_float("width"), Some(600.0));
        assert_eq!(defaulted.properties.get_text("material"), Some("热镀锌钢"));
    }

    #[test]
    fn schema_is_exposed_for_registration() {
        let s = TrayFactory.schema().unwrap();
        assert_eq!(s.type_name, "POWER_CABLE_TRAY");
        assert!(s.field("width").is_some());
    }

    #[test]
    fn element_properties_follow_the_category_schema_name() {
        let e = TrayFactory.create(&CreateRequest::new("x")).unwrap();
        assert_eq!(e.properties.schema, "POWER_CABLE_TRAY");
    }

    #[test]
    fn default_params_are_usable_as_a_prefill() {
        let p = TrayFactory.default_params();
        assert_eq!(p.get_float("width"), Some(600.0));
    }

    #[test]
    fn build_element_uses_the_category_as_schema_name() {
        let e = build_element("ANY", &CreateRequest::new("n"), "fallback");
        assert_eq!(e.name, "n");
        assert_eq!(e.properties.schema, "ANY");
    }
}
