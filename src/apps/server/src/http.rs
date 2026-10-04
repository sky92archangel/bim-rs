//! # HTTP 桥（G6-M7，外部调用形态第二期）
//!
//! axum 暴露 [`CoreSession`] 会话态门面为 REST——外部程序（CAD 插件 / 脚本 /
//! 其他系统）经 HTTP 驱动「建模 + 出图 + 导出」**多步工作流**。
//! 与 CLI 第一期（G6-M6，一次性子命令）同构：进程内 CoreSession API 之上的一层薄壳，
//! 设计文档「所有功能均开放 SDK API 可供外部调用——GUI 是薄壳」在 HTTP 侧的对应物。
//!
//! ## 会话模型
//!
//! CLI 每条命令装配后即弃；HTTP 是常驻服务，故引入**会话表**：
//! `HashMap<session_id, CoreSession>`（session_id 用 UUID v7，与持久 id 同款时间有序）。
//! 全局一把 [`std::sync::Mutex`]——各门面操作毫秒级，批处理场景串行化可接受；
//! handler 内无 `.await`，guard 不跨 await（不会死锁）。
//!
//! ## 端点清单（12 个，按资源流取舍——非 1:1 门面映射）
//!
//! ```text
//! GET    /health                        # 健康检查（服务名 + 版本 + 活跃会话数）
//! POST   /sessions                      # 创建会话（{"doc": "path"} 打开既有文档，缺省新建并装 ExampleBeam）
//! GET    /sessions/{id}/summary         # 会话摘要（模块/元素/几何/图层 + 视图/图纸数）
//! POST   /sessions/{id}/elements        # 造构件（{"category"?, "name"?}，默认 BEAM / 品类默认名）
//! POST   /sessions/{id}/geometry        # 建几何（{"element_id", "op"?}，默认 example.beam.rect）
//! POST   /sessions/{id}/views           # 建视图（{"name"}，朝向固定俯视——与 CLI new --view 同语义）
//! POST   /sessions/{id}/sheets          # 建图纸（{"name", "number", "format": "A2"}）
//! POST   /sessions/{id}/placements      # 布置视图（{"sheet_id", "view_id", "x", "y", "scale"}）
//! POST   /sessions/{id}/save            # 保存（{"path"}）
//! POST   /sessions/{id}/export          # 导出（{"format": "dxf"|"svg", "out", "sheet"?|"view"?} 二选一）
//! POST   /sessions/{id}/undo | redo     # 撤销 / 重做（返回 can_undo/can_redo/深度）
//! DELETE /sessions/{id}                 # 销毁会话
//! ```
//!
//! ## 错误语义
//!
//! 会话不存在 → 404；业务失败（[`AppError`]）→ 400；请求体不合法 → axum
//! `Json` 提取器自动 4xx。错误体统一 `{"error": "…"}`。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use core_app::CoreSession;
use core_doc::{SheetFormat, ViewOrientation};
use core_interop::Format;
use core_model::{ElementId, SheetId, ViewId};
use example_beam::{CATEGORY, GEOMETRY_OP_ID};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

/// 共享会话表（handler 经 `State` 提取）。
type SharedState = Arc<AppState>;

/// 服务端状态：HTTP 会话表——session_id → 进程内 CoreSession。
struct AppState {
    sessions: Mutex<HashMap<String, CoreSession>>,
}

/// API 错误：状态码 + 错误文本（响应体统一 `{"error": …}`）。
struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

/// 会话不存在的统一 404。
fn session_not_found(id: &str) -> ApiError {
    ApiError(StatusCode::NOT_FOUND, format!("会话 {id} 不存在"))
}

/// 业务失败的统一 400。
fn bad_request(e: core_app::AppError) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, e.to_string())
}

/// 请求体里的 uuid 字符串 → 底层 Uuid（解析失败 → 400）。
fn parse_uuid(s: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(s).map_err(|_| ApiError(StatusCode::BAD_REQUEST, format!("无效 id：{s}")))
}

/// 路由表（`with_state` 在 serve 处装配；只被本模块 `serve` 消费，私有）。
fn router() -> Router<SharedState> {
    Router::new()
        .route("/health", get(health))
        .route("/sessions", post(create_session))
        .route("/sessions/{id}/summary", get(session_summary))
        .route("/sessions/{id}/elements", post(session_create_element))
        .route("/sessions/{id}/geometry", post(session_build_geometry))
        .route("/sessions/{id}/views", post(session_create_view))
        .route("/sessions/{id}/sheets", post(session_create_sheet))
        .route("/sessions/{id}/placements", post(session_place_view))
        .route("/sessions/{id}/save", post(session_save))
        .route("/sessions/{id}/export", post(session_export))
        .route("/sessions/{id}/undo", post(session_undo))
        .route("/sessions/{id}/redo", post(session_redo))
        .route("/sessions/{id}", delete(destroy_session))
}

/// 启动 HTTP 服务（`serve` 子命令入口，`main` 薄 runtime 阻塞其上）。
///
/// # Errors
/// 端口绑定失败时返回错误文本。
pub async fn serve(host: &str, port: u16) -> Result<(), String> {
    let state: SharedState = Arc::new(AppState {
        sessions: Mutex::new(HashMap::new()),
    });
    let app = router().with_state(state);
    let addr = format!("{host}:{port}");
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| format!("绑定 {addr} 失败：{e}"))?;
    // `--port 0` 时 OS 分配实际端口——打印真实地址供探针解析（测试入口）
    let local = listener
        .local_addr()
        .map_err(|e| format!("读取监听地址失败：{e}"))?;
    println!("✓ HTTP 服务已启动：http://{local}");
    println!("（Ctrl+C 停止）");
    axum::serve(listener, app)
        .await
        .map_err(|e| format!("服务运行失败：{e}"))
}

// ---------------------------------------------------------------------------
// 请求体
// ---------------------------------------------------------------------------

/// `POST /sessions`：`doc` 缺省新建并装 ExampleBeam，给出则打开既有文档。
#[derive(Deserialize, Default)]
struct CreateSessionBody {
    #[serde(default)]
    doc: Option<String>,
}

/// `POST /sessions/{id}/elements`：品类 / 名称缺省走 ExampleBeam 默认值。
#[derive(Deserialize, Default)]
struct CreateElementBody {
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

/// `POST /sessions/{id}/geometry`：算子缺省走 `GEOMETRY_OP_ID`。
#[derive(Deserialize)]
struct BuildGeometryBody {
    element_id: String,
    #[serde(default)]
    op: Option<String>,
}

/// `POST /sessions/{id}/views`：朝向固定俯视（`ViewOrientation::top()`）。
#[derive(Deserialize)]
struct CreateViewBody {
    name: String,
}

/// `POST /sessions/{id}/sheets`：图幅直接反序列化 `SheetFormat`（serde 长名，如 "A2"）。
#[derive(Deserialize)]
struct CreateSheetBody {
    name: String,
    number: String,
    format: SheetFormat,
}

/// `POST /sessions/{id}/placements`：布置点图纸坐标 mm + 比例分母。
#[derive(Deserialize)]
struct PlaceViewBody {
    sheet_id: String,
    view_id: String,
    x: f64,
    y: f64,
    scale: u32,
}

/// `POST /sessions/{id}/save`：保存路径（.bimdoc）。
#[derive(Deserialize)]
struct SaveBody {
    path: PathBuf,
}

/// `POST /sessions/{id}/export`：`sheet`（图号）与 `view`（视图名）二选一，
/// 输出落盘 `out`——与 CLI `export` 同语义。
#[derive(Deserialize)]
struct ExportBody {
    format: ExportFormat,
    out: PathBuf,
    #[serde(default)]
    sheet: Option<String>,
    #[serde(default)]
    view: Option<String>,
}

/// `format` 的请求体枚举：字符串 → [`Format`] 的映射薄壳（照 CLI `FormatArg` 同款）。
#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum ExportFormat {
    Dxf,
    Svg,
}

impl From<ExportFormat> for Format {
    fn from(f: ExportFormat) -> Self {
        match f {
            ExportFormat::Dxf => Format::Dxf,
            ExportFormat::Svg => Format::Svg,
        }
    }
}

// ---------------------------------------------------------------------------
// handlers
// ---------------------------------------------------------------------------

/// `GET /health`：服务名 + 版本 + 活跃会话数（轻量，不装配模块）。
async fn health(State(state): State<SharedState>) -> Json<Value> {
    let sessions = state.sessions.lock().expect("会话表锁中毒").len();
    Json(json!({
        "service": env!("CARGO_PKG_NAME"),
        "version": env!("CARGO_PKG_VERSION"),
        "sessions": sessions,
    }))
}

/// `POST /sessions`：创建会话（新装配 ExampleBeam / 打开既有文档）。
async fn create_session(
    State(state): State<SharedState>,
    body: Option<Json<CreateSessionBody>>,
) -> Result<Json<Value>, ApiError> {
    let doc = body.and_then(|Json(b)| b.doc);
    let session = match doc {
        Some(path) => CoreSession::open(PathBuf::from(&path).as_path())
            .map_err(|e| ApiError(StatusCode::BAD_REQUEST, e.to_string()))?,
        None => crate::new_session().map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e))?,
    };
    let sid = Uuid::now_v7().to_string();
    state
        .sessions
        .lock()
        .expect("会话表锁中毒")
        .insert(sid.clone(), session);
    Ok(Json(json!({ "session_id": sid })))
}

/// `GET /sessions/{id}/summary`：摘要 + 视图/图纸数（`SessionSummary` 无这两项）。
async fn session_summary(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let sessions = state.sessions.lock().expect("会话表锁中毒");
    let session = sessions.get(&id).ok_or_else(|| session_not_found(&id))?;
    let s = session.summary();
    let drawing = session.document().drawing();
    Ok(Json(json!({
        "modules": s.modules,
        "elements": s.elements,
        "geometry": s.geometry,
        "layers": s.layers,
        "views": drawing.view_count(),
        "sheets": drawing.sheets.len(),
    })))
}

/// `POST /sessions/{id}/elements`：造构件（默认品类 BEAM，单一事实源）。
async fn session_create_element(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Json(body): Json<CreateElementBody>,
) -> Result<Json<Value>, ApiError> {
    let mut sessions = state.sessions.lock().expect("会话表锁中毒");
    let session = sessions
        .get_mut(&id)
        .ok_or_else(|| session_not_found(&id))?;
    let category = body.category.as_deref().unwrap_or(CATEGORY);
    let name = body.name.as_deref().unwrap_or("");
    let element = session
        .create_element(category, name)
        .map_err(bad_request)?;
    Ok(Json(json!({ "element_id": element.to_string() })))
}

/// `POST /sessions/{id}/geometry`：建几何（默认算子 `example.beam.rect`）。
async fn session_build_geometry(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Json(body): Json<BuildGeometryBody>,
) -> Result<Json<Value>, ApiError> {
    let mut sessions = state.sessions.lock().expect("会话表锁中毒");
    let session = sessions
        .get_mut(&id)
        .ok_or_else(|| session_not_found(&id))?;
    let element = ElementId::from_uuid(parse_uuid(&body.element_id)?);
    let op = body.op.as_deref().unwrap_or(GEOMETRY_OP_ID);
    let shape = session.build_geometry(element, op).map_err(bad_request)?;
    Ok(Json(json!({ "shape_id": shape.to_string() })))
}

/// `POST /sessions/{id}/views`：建视图（朝向固定俯视，`export view` 的目标）。
async fn session_create_view(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Json(body): Json<CreateViewBody>,
) -> Result<Json<Value>, ApiError> {
    let mut sessions = state.sessions.lock().expect("会话表锁中毒");
    let session = sessions
        .get_mut(&id)
        .ok_or_else(|| session_not_found(&id))?;
    let view_id = session
        .create_view(&body.name, ViewOrientation::top())
        .map_err(bad_request)?;
    Ok(Json(json!({ "view_id": view_id.to_string() })))
}

/// `POST /sessions/{id}/sheets`：建图纸（图幅 serde 直接反序列化）。
async fn session_create_sheet(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Json(body): Json<CreateSheetBody>,
) -> Result<Json<Value>, ApiError> {
    let mut sessions = state.sessions.lock().expect("会话表锁中毒");
    let session = sessions
        .get_mut(&id)
        .ok_or_else(|| session_not_found(&id))?;
    let sheet_id = session
        .create_sheet(&body.name, &body.number, body.format)
        .map_err(bad_request)?;
    Ok(Json(json!({ "sheet_id": sheet_id.to_string() })))
}

/// `POST /sessions/{id}/placements`：布置视图（越界 / 比例 0 在放置时当场拦截）。
async fn session_place_view(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Json(body): Json<PlaceViewBody>,
) -> Result<Json<Value>, ApiError> {
    let mut sessions = state.sessions.lock().expect("会话表锁中毒");
    let session = sessions
        .get_mut(&id)
        .ok_or_else(|| session_not_found(&id))?;
    let sheet_id = SheetId::from_uuid(parse_uuid(&body.sheet_id)?);
    let view_id = ViewId::from_uuid(parse_uuid(&body.view_id)?);
    session
        .place_view_on_sheet(
            sheet_id,
            view_id,
            glam::DVec2::new(body.x, body.y),
            body.scale,
        )
        .map_err(bad_request)?;
    Ok(Json(json!({ "placed": true })))
}

/// `POST /sessions/{id}/save`：保存到 `.bimdoc`。
async fn session_save(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Json(body): Json<SaveBody>,
) -> Result<Json<Value>, ApiError> {
    let mut sessions = state.sessions.lock().expect("会话表锁中毒");
    let session = sessions
        .get_mut(&id)
        .ok_or_else(|| session_not_found(&id))?;
    session
        .save(&body.path)
        .map_err(|e| ApiError(StatusCode::BAD_REQUEST, e.to_string()))?;
    Ok(Json(json!({ "saved": body.path.display().to_string() })))
}

/// `POST /sessions/{id}/export`：导出图纸（图号）或视图（视图名）二选一——
/// match 四臂穷尽（两两校验并入分支，不 panic）。
async fn session_export(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Json(body): Json<ExportBody>,
) -> Result<Json<Value>, ApiError> {
    let mut sessions = state.sessions.lock().expect("会话表锁中毒");
    let session = sessions
        .get_mut(&id)
        .ok_or_else(|| session_not_found(&id))?;
    let drawing = session.document().drawing().clone();
    let standard = drawing.standard.clone();
    let format: Format = body.format.into();

    let output = match (body.sheet.as_deref(), body.view.as_deref()) {
        (Some(_), Some(_)) => {
            return Err(ApiError(
                StatusCode::BAD_REQUEST,
                "sheet 与 view 只能二选一".to_string(),
            ));
        }
        (None, None) => {
            return Err(ApiError(
                StatusCode::BAD_REQUEST,
                "请指定 sheet（图号）或 view（视图名）".to_string(),
            ));
        }
        (Some(number), None) => {
            let sheet_id = crate::find_sheet_by_number(&drawing, number)
                .map_err(|e| ApiError(StatusCode::NOT_FOUND, e))?;
            session
                .interop()
                .export_sheet(format, &drawing, sheet_id)
                .map_err(bad_request)?
        }
        (None, Some(name)) => {
            let view = crate::find_view_by_name(&drawing, name)
                .map_err(|e| ApiError(StatusCode::NOT_FOUND, e))?;
            session
                .interop()
                .export_view(format, &view, &standard)
                .map_err(bad_request)?
        }
    };

    std::fs::write(&body.out, &output.bytes)
        .map_err(|e| ApiError(StatusCode::BAD_REQUEST, e.to_string()))?;
    Ok(Json(json!({
        "out": body.out.display().to_string(),
        "bytes": output.bytes.len(),
        "format": crate::format_name(body.format.into()),
    })))
}

/// `POST /sessions/{id}/undo`：撤销一步（返回步数对账）。
async fn session_undo(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let mut sessions = state.sessions.lock().expect("会话表锁中毒");
    let session = sessions
        .get_mut(&id)
        .ok_or_else(|| session_not_found(&id))?;
    let ok = session.undo();
    Ok(Json(json!({
        "ok": ok,
        "can_undo": session.can_undo(),
        "can_redo": session.can_redo(),
        "undo_depth": session.undo_depth(),
        "redo_depth": session.redo_depth(),
    })))
}

/// `POST /sessions/{id}/redo`：重做一步（返回步数对账）。
async fn session_redo(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let mut sessions = state.sessions.lock().expect("会话表锁中毒");
    let session = sessions
        .get_mut(&id)
        .ok_or_else(|| session_not_found(&id))?;
    let ok = session.redo();
    Ok(Json(json!({
        "ok": ok,
        "can_undo": session.can_undo(),
        "can_redo": session.can_redo(),
        "undo_depth": session.undo_depth(),
        "redo_depth": session.redo_depth(),
    })))
}

/// `DELETE /sessions/{id}`：销毁会话（不存在 → 404）。
async fn destroy_session(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let removed = state.sessions.lock().expect("会话表锁中毒").remove(&id);
    match removed {
        Some(_) => Ok(Json(json!({ "removed": true }))),
        None => Err(session_not_found(&id)),
    }
}
