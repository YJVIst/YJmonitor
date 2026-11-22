use crate::{auth, core::*, deploy, nodes, realtime, telegram, terminal};
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::{ConnectInfo, State},
    http::Request,
    response::{IntoResponse, Response},
    routing::any,
};
use hyper::service::service_fn;
use hyper_util::rt::{TokioIo, TokioTimer};
use serde_json::json;
use std::{convert::Infallible, net::SocketAddr, sync::Arc, time::Duration};
use tokio::{net::TcpListener, sync::Semaphore, task::JoinSet, time::timeout};
use tower::ServiceExt;
async fn api(
    State(app): State<App>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    req: Request<Body>,
) -> Response {
    if !app.0.php_gateway
        && !req.uri().path().starts_with("/api/")
        && req.uri().path() != "/_internal/health"
    {
        let response = if !crate::web::host_matches(&req, &app.0.origin) {
            ApiError::new(421, "请使用配置的站点域名").into_response()
        } else if !matches!(req.method().as_str(), "GET" | "HEAD") {
            ApiError::new(405, "请求方法不正确").into_response()
        } else {
            crate::web::response(
                req.uri().path(),
                req.method() == "HEAD",
                &app.lock().data.site.name,
            )
        };
        return crate::web::secure(response);
    }
    if !app.0.php_gateway
        && req.uri().path() != "/_internal/health"
        && !crate::web::host_matches(&req, &app.0.origin)
    {
        return crate::web::secure(ApiError::new(421, "请使用配置的面板地址").into_response());
    }
    let c = Context::new(
        req.method().as_str(),
        req.uri().path(),
        req.headers().clone(),
        remote,
        &app.0.origin,
    );
    let request_timeout = if c.path == "/api/enroll/claim" {
        180
    } else {
        20
    };
    let result = async {
        if c.path == "/_internal/health" {
            if c.method != "GET" || !app.gateway(&c) {
                return Err(ApiError::new(403, "gateway authorization required"));
            }
            return Ok(ApiReply::ok(
                json!({"ok":true,"service":"vistart-probe","version":VERSION,"runtime":"rust","origin":app.0.origin}),
            ));
        }
        if app.0.php_gateway && !app.gateway(&c) {
            return Err(ApiError::new(403, "gateway authorization required"));
        }
        if c.header("Sec-Fetch-Site") == "cross-site" {
            return Err(ApiError::new(403, "不允许跨站请求"));
        }
        {
            let mut i = app.lock();
            i.request(&c.ip, 120)?;
            i.expire_nodes();
        }
        if !["GET", "POST", "PUT", "PATCH", "DELETE"].contains(&c.method.as_str()) {
            return Err(ApiError::new(405, "请求方法不正确"));
        }
        if matches!(c.method.as_str(), "POST" | "PUT" | "PATCH")
            && c.header("Content-Type")
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                != "application/json"
        {
            return Err(ApiError::new(415, "请使用 JSON 请求"));
        }
        if app.0.origin.starts_with("http://")
            && (c.path.starts_with("/api/enroll/")
                || [
                    "/api/admin/deploy",
                    "/api/admin/enrollment",
                    "/api/admin/terminal-ticket",
                ]
                .contains(&c.path.as_str()))
        {
            return Err(ApiError::new(
                409,
                "请先配置 HTTPS 反向代理并设置面板地址，再接入节点或使用终端",
            ));
        }
        if c.path.starts_with("/api/admin/") {
            let mut inner = app.lock();
            app.guard(&mut inner, &c, true, c.method != "GET")?;
        }
        let max_body = if [
            "/api/admin/ops/agents/batch-update",
            "/api/admin/node-order",
        ]
        .contains(&c.path.as_str())
        {
            96 * 1024
        } else if c.path == "/api/admin/ops/restore" {
            24 * 1024 * 1024
        } else if [
            "/api/admin/theme",
            "/api/admin/theme/preview",
            "/api/admin/theme/import",
        ]
        .contains(&c.path.as_str())
        {
            crate::theme::BODY_LIMIT
        } else {
            16384
        };
        let bytes = timeout(Duration::from_secs(10), to_bytes(req.into_body(), max_body))
            .await
            .map_err(|_| ApiError::new(408, "请求超时"))?
            .map_err(|_| ApiError::new(413, "请求内容过大"))?
            .to_vec();
        if c.path == "/api/enroll/claim" {
            return crate::enrollment::claim(app.clone(), c, bytes).await;
        }
        if c.path == "/api/enroll/script" && c.method == "GET" {
            return Ok(crate::enrollment::script(&app));
        }
        if c.path == "/api/login" && c.method == "POST" {
            return auth::login(app.clone(), c, bytes).await;
        }
        if c.path.starts_with("/api/admin/telegram/group") {
            return telegram::group_api(app.clone(), c, bytes).await;
        }
        if c.path == "/api/admin/username" && c.method == "POST" {
            return auth::change_username(app.clone(), c, bytes).await;
        }
        if c.path == "/api/admin/password" && c.method == "POST" {
            return auth::password(app.clone(), c, bytes).await;
        }
        if c.path == "/api/admin/reauth" {
            return auth::elevate(app.clone(), c, bytes).await;
        }
        if c.path.starts_with("/api/admin/ops/") || c.path == "/api/migrate" {
            return crate::operations::api(app.clone(), c, bytes).await;
        }
        if c.path.starts_with("/api/admin/mfa/") {
            return auth::mfa(app.clone(), c, bytes).await;
        }
        if c.path.starts_with("/api/admin/theme") && c.method != "GET" {
            return crate::theme::change(app.clone(), c, bytes).await;
        }
        if c.path == "/api/admin/terminal-route" && c.method == "POST" {
            return crate::diagnostics::api(app.clone(), c, bytes);
        }
        dispatch(&app, &c, &bytes)
    };
    crate::web::secure(
        match timeout(Duration::from_secs(request_timeout), result).await {
            Ok(Ok(reply)) => crate::web::reply(reply, &app.0.origin),
            Ok(Err(e)) => e.into_response(),
            Err(_) => ApiError::new(504, "请求处理超时，请重试").into_response(),
        },
    )
}
fn dispatch(app: &App, c: &Context, body: &[u8]) -> ApiResult<ApiReply> {
    let mut i = app.lock();
    let p = c.path.as_str();
    let m = c.method.as_str();
    match (p, m) {
        ("/api/session", "GET") => {
            if let Some(x) = i.session(&c.sid) {
                Ok(ApiReply::ok(i.info(&x)))
            } else {
                let x = i.new_session(&c.sid, false)?;
                Ok(ApiReply::session(i.info(&x), &x.id))
            }
        }
        ("/api/logout", "POST") => {
            let x = app.guard(&mut i, c, true, true)?;
            let name = i.auth.username.clone();
            app.record(&mut i, "logout", &name);
            let x = i.new_session(&x.id, false)?;
            crate::file_sessions::save(app, &i)?;
            Ok(ApiReply::session(i.info(&x), &x.id))
        }
        ("/api/admin/theme/export", "GET") => crate::theme::export(app, &mut i, c),
        ("/api/public/theme", "GET") | ("/api/admin/theme", "GET") => crate::theme::read(&mut i),
        ("/api/public/nodes", "GET") => {
            let admin = i.session(&c.sid).is_some_and(|s| s.auth);
            if !i.data.site.public && !admin {
                return Err(ApiError::new(403, "此看板暂未公开"));
            }
            if i.cache.as_ref().is_none_or(|(at, _)| *at != now()) {
                #[derive(serde::Serialize)]
                struct Board<'a> {
                    nodes: Vec<&'a crate::model::PublicNode>,
                    site: &'a crate::model::Site,
                    preview: bool,
                    #[serde(rename = "themeRevision")]
                    theme_revision: &'a str,
                }
                let board = Board {
                    nodes: i
                        .data
                        .nodes
                        .iter()
                        .filter(|n| n.visible && !n.removing)
                        .map(|n| &n.public)
                        .collect(),
                    site: &i.data.site,
                    preview: i.data.preview,
                    theme_revision: &i.data.theme.revision,
                };
                let raw = serde_json::to_vec(&board).map_err(|_| ApiError::internal())?;
                i.cache = Some((now(), Arc::from(raw)));
            }
            let mut reply = ApiReply::ok(serde_json::Value::Null);
            reply.raw = Some(i.cache.as_ref().unwrap().1.clone());
            Ok(reply)
        }
        ("/api/admin/billing", "GET") => {
            app.guard(&mut i, c, true, false)?;
            Ok(ApiReply::ok(crate::billing::summary(&i.data.nodes, now())))
        }
        ("/api/admin/nodes", "GET") => {
            app.guard(&mut i, c, true, false)?;
            if i.admin_cache.as_ref().is_none_or(|(at, _)| *at != now()) {
                let list = crate::server_management::list(&i);
                let raw=serde_json::to_vec(&json!({"nodes":list,"site":i.data.site,"preview":i.data.preview,"themeRevision":i.data.theme.revision})).map_err(|_|ApiError::internal())?;
                i.admin_cache = Some((now(), Arc::from(raw)));
            }
            let mut reply = ApiReply::ok(serde_json::Value::Null);
            reply.raw = Some(i.admin_cache.as_ref().unwrap().1.clone());
            Ok(reply)
        }
        ("/api/admin/nodes", "POST") => nodes::save_node(app, &mut i, c, "", body),
        ("/api/admin/site", "PUT") => nodes::site(app, &mut i, c, body),
        ("/api/admin/audit", "GET") => {
            app.guard(&mut i, c, true, false)?;
            Ok(ApiReply::ok(json!({"events":i.audit})))
        }
        ("/api/admin/enrollment", "POST") => crate::enrollment::issue(app, &mut i, c, body),
        ("/api/admin/terminal-ticket", "POST") => realtime::ticket(app, &mut i, c, body),
        ("/api/admin/terminal-close", "POST") => crate::retained::api(app, &mut i, c, body),
        ("/api/admin/deploy", "POST") => deploy::begin(app, &mut i, c, body),
        ("/api/admin/go-live", "POST") => {
            app.guard(&mut i, c, true, true)?;
            if !i.data.nodes.iter().any(|n| !n.demo && n.public.online) {
                return Err(ApiError::new(409, "请先接入一台真实服务器"));
            }
            let mut data = i.data.clone();
            data.nodes.retain(|n| !n.demo);
            data.preview = false;
            app.save_data(&mut i, data)?;
            app.record(&mut i, "go_live", "真实监控");
            Ok(ApiReply::ok(json!({"ok":true})))
        }
        _ => {
            if p == "/api/admin/node-order" && c.method == "POST" {
                return nodes::reorder(app, &mut i, c, body);
            }
            if p == "/api/admin/telegram"
                || p == "/api/admin/telegram/test"
                || p == "/api/admin/telegram/preview"
            {
                return telegram::api(app, &mut i, c, body);
            }
            if p == "/api/admin/commands" || p.starts_with("/api/admin/commands/") {
                return nodes::commands(app, &mut i, c, body);
            }
            if let Some(id) = p.strip_prefix("/api/admin/nodes/") {
                if !valid_id(id) {
                    return Err(ApiError::new(404, "服务器不存在"));
                }
                if m == "PATCH" {
                    return nodes::save_node(app, &mut i, c, id, body);
                }
                if m == "DELETE" {
                    return nodes::delete_node(app, &mut i, c, id, body);
                }
            }
            if let Some(id) = p.strip_prefix("/api/admin/renewals/")
                && m == "POST"
                && valid_id(id)
            {
                return nodes::renew(app, &mut i, c, id, body);
            }
            if let Some(id) = p.strip_prefix("/api/admin/deploy/")
                && m == "GET"
                && valid_id(id)
            {
                return deploy::status(app, &mut i, c, id);
            }
            Err(ApiError::new(404, "接口不存在"))
        }
    }
}
fn valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
pub async fn serve(app: App, listen: SocketAddr) -> Result<(), &'static str> {
    let listener = TcpListener::bind(listen)
        .await
        .map_err(|_| "panel port unavailable")?;
    tokio::spawn(crate::ssh_address::monitor(app.clone()));
    let router = Router::new()
        .route("/api/agent", any(realtime::agent_handler))
        .route("/api/terminal", any(terminal::handler))
        .fallback(api)
        .with_state(app.clone());
    let slots = Arc::new(Semaphore::new(256));
    let mut tasks = JoinSet::new();
    telegram::start(&app);
    crate::file_sessions::start(&app);
    crate::retained::start(&app);
    crate::legacy_terminals::start(&app);
    crate::operations::start(&app);
    eprintln!("Rust probe controller started");
    loop {
        tokio::select! {_=app.0.stop.cancelled()=>break,Some(_)=tasks.join_next()=>{},incoming=listener.accept()=>{let(socket,remote)=incoming.map_err(|_|"listener failed")?;let Ok(slot)=slots.clone().try_acquire_owned()else{drop(socket);continue};let router=router.clone();let stop=app.0.stop.clone();tasks.spawn(async move{let _slot=slot;let _=socket.set_nodelay(true);let service=service_fn(move|req:Request<hyper::body::Incoming>|{let router=router.clone();async move{let mut req=req.map(Body::new);req.extensions_mut().insert(ConnectInfo(remote));let size=req.headers().iter().map(|(k,v)|k.as_str().len()+v.len()+4).sum::<usize>();if size>8192||req.uri().to_string().len()>2048{return Ok::<_,Infallible>(ApiError::new(431,"请求头过大").into_response())}router.oneshot(req).await}});let mut builder=hyper::server::conn::http1::Builder::new();builder.timer(TokioTimer::new()).header_read_timeout(Duration::from_secs(5)).max_headers(48).max_buf_size(16384).keep_alive(false);let conn=builder.serve_connection(TokioIo::new(socket),service).with_upgrades();tokio::select!{_=stop.cancelled()=>{},_=conn=>{}}});}}
    }
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    Ok(())
}
