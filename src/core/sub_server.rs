//! 本机订阅服务：只监听 127.0.0.1，把最新的优选节点提供给代理软件导入与定时更新。
//!
//! 地址形如 `http://127.0.0.1:端口/令牌/daily/clash`。来源是 `daily`（自动优选）或 `verify`
//! （可用性验证）；格式是 `clash`（YAML）或 `v2ray`（Base64 分享链接），省略时按 User-Agent
//! 判断。令牌随机生成，Host 只接受本机地址，网页即使借 DNS 重绑定连到这个端口也读不到节点。
//!
//! 无窗口的局域网服务（`cfbest-cli serve`）在自己的端口上用 [`lan_response`] 提供同样的订阅，
//! 那里不限制 Host：局域网里的手机、电脑要用 NAS 的地址来更新，令牌就是凭证。

use std::io;
use std::net::{Ipv4Addr, TcpListener as StdTcpListener};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use base64::Engine as _;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::runtime::Runtime;
use tokio::time::timeout;

use crate::core::proxy::{FeedFormat, ProxyFeed, percent_encode};
use crate::core::storage;

/// 默认端口，避开常见代理端口与 Windows 的动态端口段（49152 起）。
pub const DEFAULT_PORT: u16 = 27207;
/// 首选端口被占用时，依次尝试其后的端口。
const PORT_ATTEMPTS: u16 = 20;
/// 建议客户端自动更新订阅的间隔（小时），通过 `profile-update-interval` 响应头告知。
const UPDATE_INTERVAL_HOURS: u32 = 6;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeedSource {
    Daily,
    Verify,
}

impl FeedSource {
    pub fn slug(self) -> &'static str {
        match self {
            FeedSource::Daily => "daily",
            FeedSource::Verify => "verify",
        }
    }

    /// 导入后在代理软件里显示的订阅名。
    pub fn title(self) -> &'static str {
        match self {
            FeedSource::Daily => "CF Best 自动优选",
            FeedSource::Verify => "CF Best 可用性验证",
        }
    }

    fn from_slug(slug: &str) -> Option<Self> {
        match slug {
            "daily" => Some(FeedSource::Daily),
            "verify" => Some(FeedSource::Verify),
            _ => None,
        }
    }
}

#[derive(Default)]
struct Shared {
    port: Option<u16>,
    token: String,
    daily: Option<Arc<ProxyFeed>>,
    verify: Option<Arc<ProxyFeed>>,
}

fn shared() -> MutexGuard<'static, Shared> {
    static SHARED: OnceLock<Mutex<Shared>> = OnceLock::new();
    SHARED
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 更新某个来源的节点；`None` 表示暂无可导入的结果，客户端更新时收到 503，保留原有节点。
pub fn publish(source: FeedSource, feed: Option<ProxyFeed>) {
    let feed = feed.map(Arc::new);
    let mut shared = shared();
    match source {
        FeedSource::Daily => shared.daily = feed,
        FeedSource::Verify => shared.verify = feed,
    }
}

/// 设置订阅地址中的令牌（局域网服务不经 [`start`] 启动本机服务，单独设置）。
#[allow(dead_code)]
pub fn set_token(token: &str) {
    shared().token = token.to_string();
}

/// 订阅地址；`format` 为 None 时由服务按 User-Agent 选择格式，适合手动粘贴到任意客户端。
pub fn feed_url(port: u16, token: &str, source: FeedSource, format: Option<FeedFormat>) -> String {
    let mut url = format!("http://127.0.0.1:{port}/{token}/{}", source.slug());
    if let Some(format) = format {
        url.push('/');
        url.push_str(format.slug());
    }
    url
}

/// 启动服务并返回实际端口，已在运行时直接返回。优先用 `preferred`（0 表示默认端口），
/// 被占用时依次尝试其后的端口；端口变了，客户端里保存的旧地址就要重新导入。
pub fn start(runtime: &Runtime, preferred: u16, token: &str) -> Result<u16, String> {
    {
        let mut shared = shared();
        shared.token = token.to_string();
        if let Some(port) = shared.port {
            return Ok(port);
        }
    }
    let base = if preferred == 0 {
        DEFAULT_PORT
    } else {
        preferred
    };
    let mut last_error = None;
    for port in (0..PORT_ATTEMPTS).filter_map(|offset| base.checked_add(offset)) {
        let listener = match StdTcpListener::bind((Ipv4Addr::LOCALHOST, port)) {
            Ok(listener) => listener,
            Err(err) => {
                last_error = Some(err);
                continue;
            }
        };
        let listener = listener.set_nonblocking(true).and_then(|()| {
            let _guard = runtime.enter();
            TcpListener::from_std(listener)
        });
        let listener = listener.map_err(|err| format!("本机订阅服务启动失败：{err}"))?;
        shared().port = Some(port);
        runtime.spawn(serve(listener, port));
        storage::log_line(&format!("本机订阅服务已启动：127.0.0.1:{port}"));
        return Ok(port);
    }
    Err(format!(
        "本机端口 {base} 起的 {PORT_ATTEMPTS} 个端口都无法使用：{}",
        last_error.map_or_else(|| "端口超出范围".to_string(), |err| err.to_string())
    ))
}

async fn serve(listener: TcpListener, port: u16) {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                tokio::spawn(handle(stream, port));
            }
            Err(err) => {
                storage::log_line(&format!("本机订阅服务接受连接失败：{err}"));
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    }
}

async fn handle(mut stream: TcpStream, port: u16) {
    let Ok(Ok(head)) = timeout(Duration::from_secs(5), read_head(&mut stream)).await else {
        return;
    };
    let response = answer(&head, Some(port));
    let _ = timeout(Duration::from_secs(10), stream.write_all(&response)).await;
    let _ = stream.shutdown().await;
}

/// 局域网服务收到的订阅请求：按令牌返回节点，不检查 Host。
#[allow(dead_code)]
pub fn lan_response(head: &str) -> Vec<u8> {
    answer(head, None)
}

/// 用当前令牌与节点回答一个请求并记日志；`local_port` 为 Some 时只接受本机 Host。
fn answer(head: &str, local_port: Option<u16>) -> Vec<u8> {
    let (token, daily, verify) = {
        let shared = shared();
        (
            shared.token.clone(),
            shared.daily.clone(),
            shared.verify.clone(),
        )
    };
    let response = respond(head, local_port, &token, |source| match source {
        FeedSource::Daily => daily.clone(),
        FeedSource::Verify => verify.clone(),
    });
    // 日志里不记令牌。
    let request = head.lines().next().unwrap_or("");
    let request = if token.is_empty() {
        request.to_string()
    } else {
        request.replace(token.as_str(), "***")
    };
    let scope = if local_port.is_some() {
        "本机"
    } else {
        "局域网"
    };
    storage::log_line(&format!("{scope}订阅 {} ← {request}", response.status));
    response.to_bytes()
}

/// 读到请求头结束（空行）为止，最多 16 KB；订阅请求都是不带正文的 GET。
async fn read_head(stream: &mut TcpStream) -> io::Result<String> {
    let mut head = Vec::with_capacity(1024);
    let mut chunk = [0u8; 2048];
    loop {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        head.extend_from_slice(&chunk[..read]);
        if head.windows(4).any(|window| window == b"\r\n\r\n") || head.len() > 16 * 1024 {
            break;
        }
    }
    Ok(String::from_utf8_lossy(&head).into_owned())
}

struct Response {
    status: u16,
    content_type: &'static str,
    headers: Vec<(&'static str, String)>,
    body: Vec<u8>,
    head_only: bool,
}

impl Response {
    fn text(status: u16, message: &str) -> Self {
        Self {
            status,
            content_type: "text/plain; charset=utf-8",
            headers: Vec::new(),
            body: format!("CF Best：{message}\n").into_bytes(),
            head_only: false,
        }
    }

    fn reason(&self) -> &'static str {
        match self.status {
            200 => "OK",
            403 => "Forbidden",
            404 => "Not Found",
            405 => "Method Not Allowed",
            _ => "Service Unavailable",
        }
    }

    fn to_bytes(&self) -> Vec<u8> {
        let mut head = format!(
            "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n",
            self.status,
            self.reason(),
            self.content_type,
            self.body.len()
        );
        for (name, value) in &self.headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        head.push_str("\r\n");
        let mut bytes = head.into_bytes();
        if !self.head_only {
            bytes.extend_from_slice(&self.body);
        }
        bytes
    }
}

/// `local_port` 为 Some 时只接受指向本机该端口的 Host（本机订阅服务）；None 不检查（局域网服务）。
fn respond(
    head: &str,
    local_port: Option<u16>,
    token: &str,
    feed_for: impl Fn(FeedSource) -> Option<Arc<ProxyFeed>>,
) -> Response {
    let mut lines = head.split("\r\n");
    let mut request_line = lines.next().unwrap_or("").split_whitespace();
    let method = request_line.next().unwrap_or("");
    let target = request_line.next().unwrap_or("");
    let header = |name: &str| {
        lines.clone().find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.trim().eq_ignore_ascii_case(name).then(|| value.trim())
        })
    };
    let head_only = method == "HEAD";
    let mut response = if method != "GET" && !head_only {
        Response::text(405, "只支持 GET 请求")
    } else if let Some(port) = local_port
        && header("host").is_some_and(|host| !host_allowed(host, port))
    {
        Response::text(403, "只接受本机访问")
    } else {
        route(target, token, header("user-agent").unwrap_or(""), feed_for)
    };
    response.head_only = head_only;
    response
}

fn route(
    target: &str,
    token: &str,
    user_agent: &str,
    feed_for: impl Fn(FeedSource) -> Option<Arc<ProxyFeed>>,
) -> Response {
    let not_found = || Response::text(404, "订阅地址无效，请在 CF Best 中重新导入");
    // 有的客户端会把导入链接里 url 后面的 `&name=…` 也拼进订阅地址，按查询参数一样忽略。
    let path = target.split(['?', '#', '&']).next().unwrap_or("");
    let mut segments = path.trim_matches('/').split('/');
    let (Some(given), Some(source)) = (segments.next(), segments.next()) else {
        return not_found();
    };
    if token.is_empty() || given != token {
        return not_found();
    }
    let Some(source) = FeedSource::from_slug(source) else {
        return not_found();
    };
    let format = match segments.next() {
        None | Some("") => sniff_format(user_agent),
        Some("clash") => FeedFormat::Clash,
        Some("v2ray") => FeedFormat::V2ray,
        Some(_) => return not_found(),
    };
    if segments.next().is_some() {
        return not_found();
    }
    let Some(feed) = feed_for(source) else {
        return Response::text(
            503,
            "暂无隧道验证通过的结果，或订阅节点尚未载入；请在 CF Best 中完成验证后再更新",
        );
    };
    let (content_type, extension) = match format {
        FeedFormat::Clash => ("text/yaml; charset=utf-8", "yaml"),
        FeedFormat::V2ray => ("text/plain; charset=utf-8", "txt"),
    };
    Response {
        status: 200,
        content_type,
        headers: vec![
            (
                "Content-Disposition",
                format!(
                    "attachment; filename*=UTF-8''{}.{extension}",
                    percent_encode(&feed.title)
                ),
            ),
            (
                "profile-title",
                format!(
                    "base64:{}",
                    base64::engine::general_purpose::STANDARD.encode(&feed.title)
                ),
            ),
            ("profile-update-interval", UPDATE_INTERVAL_HOURS.to_string()),
        ],
        body: feed.render(format).into_bytes(),
        head_only: false,
    }
}

/// 只接受本机地址的 Host，挡住 DNS 重绑定。
fn host_allowed(host: &str, port: u16) -> bool {
    let host = host.to_ascii_lowercase();
    ["127.0.0.1", "localhost", "[::1]"]
        .iter()
        .any(|name| host == *name || host == format!("{name}:{port}"))
}

/// 未指定格式时：Clash 系客户端拿 YAML，其余（v2rayN、Karing、Shadowrocket 等）拿 Base64 分享链接。
fn sniff_format(user_agent: &str) -> FeedFormat {
    let agent = user_agent.to_ascii_lowercase();
    if ["clash", "mihomo", "meta", "stash", "verge", "nyanpasu"]
        .iter()
        .any(|name| agent.contains(name))
    {
        FeedFormat::Clash
    } else {
        FeedFormat::V2ray
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::http::parse_vless_link;
    use crate::core::proxy::Endpoint;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";
    const PORT: u16 = 27207;

    fn feed() -> Arc<ProxyFeed> {
        let node = parse_vless_link(
            "vless://b831381d-6324-4d53-ad4f-8cda48b30811@edge.example.com:443?security=tls&type=ws&path=%2F#HK",
        )
        .unwrap();
        Arc::new(
            ProxyFeed::new(
                FeedSource::Daily.title(),
                "2026-10-29 09:00",
                node,
                vec![Endpoint {
                    ip: "104.16.1.2".into(),
                    port: 443,
                    colo: "HKG".into(),
                }],
            )
            .unwrap(),
        )
    }

    fn get(path: &str, host: &str, agent: &str) -> Response {
        let head = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: {agent}\r\n\r\n");
        respond(&head, Some(PORT), TOKEN, |source| {
            (source == FeedSource::Daily).then(feed)
        })
    }

    fn body(response: &Response) -> String {
        String::from_utf8(response.body.clone()).unwrap()
    }

    #[test]
    fn serves_clash_or_v2ray_by_path_and_user_agent() {
        let host = format!("127.0.0.1:{PORT}");
        let clash = get(&format!("/{TOKEN}/daily/clash"), &host, "v2rayN/7.0");
        assert_eq!(clash.status, 200);
        assert!(body(&clash).contains("type: vless"));
        assert_eq!(clash.content_type, "text/yaml; charset=utf-8");

        let v2ray = get(&format!("/{TOKEN}/daily/v2ray"), &host, "clash-verge/v2.3");
        assert_eq!(v2ray.status, 200);
        let links = base64::engine::general_purpose::STANDARD
            .decode(body(&v2ray))
            .unwrap();
        assert!(String::from_utf8(links).unwrap().starts_with("vless://"));

        let sniffed = get(&format!("/{TOKEN}/daily?x=1"), &host, "clash-verge/v2.3");
        assert_eq!(sniffed.content_type, "text/yaml; charset=utf-8");
        let sniffed = get(&format!("/{TOKEN}/daily/"), &host, "v2rayN/7.0");
        assert_eq!(sniffed.content_type, "text/plain; charset=utf-8");
        // Clash Verge Rev 会把导入链接中 url 之后的参数并入订阅地址。
        let glued = get(
            &format!("/{TOKEN}/daily/v2ray&name=CF%20Best"),
            &host,
            "clash-verge/v2.3",
        );
        assert_eq!(glued.status, 200);
        assert_eq!(glued.content_type, "text/plain; charset=utf-8");

        let headers = &clash.headers;
        assert!(
            headers
                .iter()
                .any(|(name, value)| *name == "profile-update-interval" && value == "6")
        );
        assert!(
            headers
                .iter()
                .any(|(name, value)| *name == "Content-Disposition"
                    && value.ends_with("''CF%20Best%20%E8%87%AA%E5%8A%A8%E4%BC%98%E9%80%89.yaml"))
        );
    }

    #[test]
    fn rejects_wrong_token_host_method_and_missing_feed() {
        let host = format!("localhost:{PORT}");
        assert_eq!(get(&format!("/{TOKEN}/daily/clash"), &host, "").status, 200);
        assert_eq!(get("/wrong/daily/clash", &host, "").status, 404);
        assert_eq!(get(&format!("/{TOKEN}"), &host, "").status, 404);
        assert_eq!(get(&format!("/{TOKEN}/weekly"), &host, "").status, 404);
        assert_eq!(get(&format!("/{TOKEN}/daily/json"), &host, "").status, 404);
        assert_eq!(
            get(&format!("/{TOKEN}/daily/clash/x"), &host, "").status,
            404
        );
        assert_eq!(
            get(&format!("/{TOKEN}/verify/clash"), &host, "").status,
            503
        );
        assert_eq!(
            get(
                &format!("/{TOKEN}/daily/clash"),
                &format!("evil.example:{PORT}"),
                ""
            )
            .status,
            403
        );
        let empty_token = respond("GET //daily HTTP/1.1\r\n\r\n", Some(PORT), "", |_| {
            Some(feed())
        });
        assert_eq!(empty_token.status, 404);
        let post = respond(
            &format!("POST /{TOKEN}/daily HTTP/1.1\r\n\r\n"),
            Some(PORT),
            TOKEN,
            |_| Some(feed()),
        );
        assert_eq!(post.status, 405);
    }

    #[test]
    fn lan_requests_skip_the_host_check_but_still_need_the_token() {
        let lan = |path: &str| {
            respond(
                &format!(
                    "GET {path} HTTP/1.1
Host: nas.lan:27208

"
                ),
                None,
                TOKEN,
                |_| Some(feed()),
            )
            .status
        };
        assert_eq!(lan(&format!("/{TOKEN}/daily/clash")), 200);
        assert_eq!(lan("/wrong/daily/clash"), 404);
    }

    #[test]
    fn head_request_sends_headers_only() {
        let response = respond(
            &format!("HEAD /{TOKEN}/daily/clash HTTP/1.1\r\nHost: 127.0.0.1:{PORT}\r\n\r\n"),
            Some(PORT),
            TOKEN,
            |_| Some(feed()),
        );
        let bytes = String::from_utf8(response.to_bytes()).unwrap();
        assert!(bytes.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(bytes.contains(&format!("Content-Length: {}\r\n", response.body.len())));
        assert!(bytes.ends_with("\r\n\r\n"));
    }

    #[tokio::test]
    async fn serves_published_feed_over_loopback() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        shared().token = TOKEN.to_string();
        publish(FeedSource::Daily, Some((*feed()).clone()));
        tokio::spawn(serve(listener, port));

        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port))
            .await
            .unwrap();
        let request = format!(
            "GET {} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUser-Agent: clash-verge/v2.3\r\n\r\n",
            feed_url(port, TOKEN, FeedSource::Daily, None)
                .trim_start_matches(&format!("http://127.0.0.1:{port}"))
        );
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).await.unwrap();
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
        assert!(response.contains("\r\n\r\n# CF Best 自动优选 · 2026-10-29 09:00 · 1 个节点\n"));
        assert!(response.contains("    server: \"104.16.1.2\"\n"));
    }
}
