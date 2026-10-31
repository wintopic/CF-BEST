//! 局域网服务的 HTTP 层：管理网页、JSON 接口与订阅地址共用一个端口。
//!
//! - 订阅地址 `/<令牌>/daily[/clash|/v2ray]` 交给 `sub_server::lan_response`，令牌即凭证。
//! - 设置了密码时，网页与接口要先登录（Cookie），也接受 `Authorization: Bearer <密码>`。
//! - 没设密码时只接受 IP、localhost、单段主机名与 `.local` / `.lan` 等内网域名的 Host，
//!   挡住借 DNS 重绑定从外部网页读写接口；其他域名用 `CFBEST_ALLOWED_HOSTS` 放行。
//! - 修改类接口（POST）要求 `X-CFBest` 请求头：跨站网页带不上自定义头。

use std::io;
use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;

use super::Service;
use crate::core::storage;
use crate::core::sub_server;

const INDEX_HTML: &str = include_str!("index.html");
const MAX_HEAD: usize = 16 * 1024;
const MAX_BODY: usize = 64 * 1024;
const SESSION_COOKIE: &str = "cfbest_session";
/// 登录 Cookie 的有效期：30 天。
const SESSION_MAX_AGE: u32 = 30 * 24 * 3600;

pub async fn serve(listener: TcpListener, service: Arc<Service>) {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                tokio::spawn(handle(stream, service.clone()));
            }
            Err(err) => {
                storage::log_line(&format!("局域网服务接受连接失败：{err}"));
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    }
}

async fn handle(mut stream: TcpStream, service: Arc<Service>) {
    let Ok(Ok(Some(request))) = timeout(Duration::from_secs(10), read_request(&mut stream)).await
    else {
        return;
    };
    let bytes = respond(&service, &request).await;
    let _ = timeout(Duration::from_secs(10), stream.write_all(&bytes)).await;
    let _ = stream.shutdown().await;
}

struct Request {
    /// 原始请求头，订阅请求原样交给 `sub_server`。
    head: String,
    method: String,
    path: String,
    query: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Request {
    fn parse(head: &str) -> Option<Self> {
        let mut lines = head.split("\r\n");
        let mut request_line = lines.next()?.split_whitespace();
        let method = request_line.next()?.to_string();
        let target = request_line.next()?;
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        let headers = lines
            .filter_map(|line| {
                let (name, value) = line.split_once(':')?;
                Some((name.trim().to_ascii_lowercase(), value.trim().to_string()))
            })
            .collect();
        Some(Self {
            head: head.to_string(),
            method,
            path: path.to_string(),
            query: query.to_string(),
            headers,
            body: Vec::new(),
        })
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    fn cookie(&self, name: &str) -> Option<&str> {
        self.header("cookie")?.split(';').find_map(|pair| {
            let (key, value) = pair.trim().split_once('=')?;
            (key == name).then_some(value)
        })
    }

    fn query_value(&self, name: &str) -> Option<&str> {
        self.query.split('&').find_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            (key == name).then_some(value)
        })
    }

    fn json(&self) -> Result<serde_json::Value, String> {
        serde_json::from_slice(&self.body).map_err(|_| "请求内容不是有效的 JSON".to_string())
    }
}

/// 读请求头与正文（按 Content-Length，最多 64 KB）。连接提前关闭或格式不对时返回 None。
async fn read_request(stream: &mut TcpStream) -> io::Result<Option<Request>> {
    let mut buffer = Vec::with_capacity(2048);
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(pos) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break pos;
        }
        if buffer.len() > MAX_HEAD {
            return Ok(None);
        }
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Ok(None);
        }
        buffer.extend_from_slice(&chunk[..read]);
    };
    let head = String::from_utf8_lossy(&buffer[..head_end + 4]).into_owned();
    let Some(mut request) = Request::parse(&head) else {
        return Ok(None);
    };
    let length = request
        .header("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    if length > MAX_BODY {
        return Ok(None);
    }
    let mut body = buffer.split_off(head_end + 4);
    while body.len() < length {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Ok(None);
        }
        body.extend_from_slice(&chunk[..read]);
    }
    body.truncate(length);
    request.body = body;
    Ok(Some(request))
}

struct Response {
    status: u16,
    content_type: &'static str,
    headers: Vec<(&'static str, String)>,
    body: Vec<u8>,
}

impl Response {
    fn json(status: u16, value: serde_json::Value) -> Self {
        Self {
            status,
            content_type: "application/json; charset=utf-8",
            headers: Vec::new(),
            body: value.to_string().into_bytes(),
        }
    }

    fn ok(message: impl Into<String>) -> Self {
        Self::json(200, json!({ "ok": true, "message": message.into() }))
    }

    fn error(status: u16, message: impl Into<String>) -> Self {
        Self::json(status, json!({ "ok": false, "error": message.into() }))
    }

    fn text(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            content_type: "text/plain; charset=utf-8",
            headers: Vec::new(),
            body: body.into().into_bytes(),
        }
    }

    fn html() -> Self {
        Self {
            status: 200,
            content_type: "text/html; charset=utf-8",
            headers: vec![
                (
                    "Content-Security-Policy",
                    "default-src 'self'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; \
                     img-src 'self' data:; frame-ancestors 'none'"
                        .into(),
                ),
                ("X-Frame-Options", "DENY".into()),
            ],
            body: INDEX_HTML.as_bytes().to_vec(),
        }
    }

    fn with_cookie(mut self, value: &str, max_age: u32) -> Self {
        self.headers.push((
            "Set-Cookie",
            format!(
                "{SESSION_COOKIE}={value}; Path=/; HttpOnly; SameSite=Strict; Max-Age={max_age}"
            ),
        ));
        self
    }

    fn to_bytes(&self) -> Vec<u8> {
        let reason = match self.status {
            200 => "OK",
            204 => "No Content",
            400 => "Bad Request",
            401 => "Unauthorized",
            403 => "Forbidden",
            404 => "Not Found",
            405 => "Method Not Allowed",
            409 => "Conflict",
            _ => "Error",
        };
        let mut head = format!(
            "HTTP/1.1 {} {reason}\r\nContent-Type: {}\r\nContent-Length: {}\r\n\
             Cache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n\
             Referrer-Policy: no-referrer\r\nConnection: close\r\n",
            self.status,
            self.content_type,
            self.body.len()
        );
        for (name, value) in &self.headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        head.push_str("\r\n");
        let mut bytes = head.into_bytes();
        bytes.extend_from_slice(&self.body);
        bytes
    }
}

async fn respond(service: &Arc<Service>, request: &Request) -> Vec<u8> {
    let path = request.path.as_str();
    match path {
        "/healthz" => return Response::text(200, "ok\n").to_bytes(),
        "/favicon.ico" => return Response::text(204, "").to_bytes(),
        "/" | "/index.html" | "/top.txt" => {}
        _ if path.starts_with("/api/") => {}
        // 其余路径都按订阅地址处理：令牌不对时 sub_server 返回 404。
        _ => return sub_server::lan_response(&request.head),
    }
    if service.options.password.is_none()
        && request
            .header("host")
            .is_some_and(|host| !host_allowed(host, &service.options.allowed_hosts))
    {
        return Response::error(
            403,
            "不认识的访问域名：设置访问密码（CFBEST_PASSWORD），或把域名加入 CFBEST_ALLOWED_HOSTS",
        )
        .to_bytes();
    }
    route(service, request).await.to_bytes()
}

async fn route(service: &Arc<Service>, request: &Request) -> Response {
    let method = request.method.as_str();
    let path = request.path.as_str();
    if method == "GET" && (path == "/" || path == "/index.html") {
        return Response::html();
    }
    if method == "POST" && request.header("x-cfbest").is_none() {
        return Response::error(403, "缺少 X-CFBest 请求头");
    }
    if (method, path) == ("POST", "/api/login") {
        return login(service, request).await;
    }
    if (method, path) == ("POST", "/api/logout") {
        return Response::ok("已退出登录").with_cookie("", 0);
    }
    if !authorized(service, request) {
        return Response::json(
            401,
            json!({ "ok": false, "error": "请先登录", "login": true }),
        );
    }
    match (method, path) {
        ("GET", "/api/state") => {
            let since = request
                .query_value("since")
                .and_then(|value| value.parse().ok());
            Response::json(200, service.snapshot(since))
        }
        ("GET", "/top.txt") => Response::text(200, service.top_text()),
        ("POST", "/api/run") => match service.start_daily("手动") {
            Ok(()) => Response::ok("已开始一轮优选"),
            Err(err) => Response::error(409, err),
        },
        ("POST", "/api/regions/classify") => {
            let force = request
                .json()
                .ok()
                .and_then(|body| body.get("force")?.as_bool())
                .unwrap_or(false);
            match service.start_classify(force) {
                Ok(()) => Response::ok("已开始划分出口区域"),
                Err(err) => Response::error(409, err),
            }
        }
        ("POST", "/api/stop") => {
            if service.stop_daily() {
                Response::ok("正在停止…")
            } else {
                Response::error(409, "当前没有运行中的优选")
            }
        }
        ("POST", "/api/settings") => {
            let patch = match request.json() {
                Ok(patch) => patch,
                Err(err) => return Response::error(400, err),
            };
            match service.update_settings(&patch) {
                Ok(message) => Response::ok(message),
                Err(err) => Response::error(400, err),
            }
        }
        ("POST", "/api/subscription/refresh") => {
            if service.lock().settings.sub_url.trim().is_empty() {
                return Response::error(400, "还没有设置订阅链接");
            }
            tokio::spawn(service.clone().refresh_subscription());
            Response::ok("正在拉取订阅…")
        }
        ("POST", "/api/token/reset") => {
            service.reset_token();
            let response = Response::ok("订阅令牌已更换：请在代理软件中重新添加订阅地址");
            // 登录 Cookie 由令牌加盐，换令牌后给当前浏览器发一个新的。
            match session_value(service) {
                Some(value) => response.with_cookie(&value, SESSION_MAX_AGE),
                None => response,
            }
        }
        (_, "/api/state" | "/top.txt") => Response::error(405, "只支持 GET 请求"),
        (_, path) if path.starts_with("/api/") && method != "POST" && method != "GET" => {
            Response::error(405, "不支持的请求方法")
        }
        _ => Response::error(404, "没有这个接口"),
    }
}

async fn login(service: &Arc<Service>, request: &Request) -> Response {
    let Some(password) = &service.options.password else {
        return Response::ok("未设置访问密码，无需登录");
    };
    let given = request
        .json()
        .ok()
        .and_then(|body| body.get("password")?.as_str().map(str::to_string))
        .unwrap_or_default();
    if !constant_time_eq(&given, password) {
        // 放慢连续猜密码。
        tokio::time::sleep(Duration::from_secs(1)).await;
        storage::log_line("网页登录失败：密码错误");
        return Response::error(401, "密码错误");
    }
    match session_value(service) {
        Some(value) => Response::ok("已登录").with_cookie(&value, SESSION_MAX_AGE),
        None => Response::ok("已登录"),
    }
}

fn authorized(service: &Service, request: &Request) -> bool {
    let Some(password) = &service.options.password else {
        return true;
    };
    if let Some(bearer) = request
        .header("authorization")
        .and_then(|value| value.strip_prefix("Bearer "))
    {
        return constant_time_eq(bearer.trim(), password);
    }
    match (request.cookie(SESSION_COOKIE), session_value(service)) {
        (Some(cookie), Some(expected)) => constant_time_eq(cookie, &expected),
        _ => false,
    }
}

/// 登录 Cookie 的值：密码与订阅令牌的 SHA-256。不必在服务端保存会话，重启后仍然有效；
/// 改密码或换令牌后旧 Cookie 失效。
fn session_value(service: &Service) -> Option<String> {
    let password = service.options.password.as_ref()?;
    let token = service.lock().settings.proxy_feed_token.clone();
    let digest = ring::digest::digest(
        &ring::digest::SHA256,
        format!("cfbest-session\0{token}\0{password}").as_bytes(),
    );
    Some(
        digest
            .as_ref()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    )
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

/// 未设密码时接受的 Host：IP 地址、localhost、单段主机名（NAS 的机器名）、内网专用后缀，
/// 以及 `extra` 中列出的域名。这些名字在公网上解析不到，DNS 重绑定用不了。
fn host_allowed(host: &str, extra: &[String]) -> bool {
    let host = host.trim().to_ascii_lowercase();
    if host.starts_with('[') {
        // IPv6 地址。
        return true;
    }
    let name = match host.rsplit_once(':') {
        Some((name, port)) if port.bytes().all(|byte| byte.is_ascii_digit()) => name,
        _ => host.as_str(),
    };
    name.parse::<Ipv4Addr>().is_ok()
        || name == "localhost"
        || !name.contains('.')
        || [
            ".local",
            ".lan",
            ".home",
            ".home.arpa",
            ".internal",
            ".localdomain",
        ]
        .iter()
        .any(|suffix| name.ends_with(suffix))
        || extra
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_check_accepts_lan_names_and_rejects_public_domains() {
        let extra = vec!["cf.example.com".to_string()];
        for host in [
            "192.168.1.10:27208",
            "127.0.0.1",
            "[fd00::1]:27208",
            "localhost:27208",
            "DS920:27208",
            "nas.local",
            "fnos.lan:27208",
            "CF.example.com:27208",
        ] {
            assert!(host_allowed(host, &extra), "{host}");
        }
        for host in [
            "evil.example:27208",
            "rebind.attacker.net",
            "1.2.3.4.nip.io",
        ] {
            assert!(!host_allowed(host, &extra), "{host}");
        }
    }

    #[test]
    fn request_parsing_reads_headers_cookies_and_query() {
        let request = Request::parse(
            "GET /api/state?since=12 HTTP/1.1\r\nHost: nas\r\nCookie: a=1; cfbest_session=abc\r\n\r\n",
        )
        .unwrap();
        assert_eq!(request.method, "GET");
        assert_eq!(request.path, "/api/state");
        assert_eq!(request.query_value("since"), Some("12"));
        assert_eq!(request.header("host"), Some("nas"));
        assert_eq!(request.cookie(SESSION_COOKIE), Some("abc"));
        assert_eq!(request.cookie("missing"), None);
    }

    #[test]
    fn constant_time_eq_compares_whole_strings() {
        assert!(constant_time_eq("secret", "secret"));
        assert!(!constant_time_eq("secret", "secreT"));
        assert!(!constant_time_eq("secret", "secret2"));
        assert!(!constant_time_eq("", "x"));
    }
}
