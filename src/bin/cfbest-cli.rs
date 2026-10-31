//! cfbest-cli：与图形界面共用同一套内核的无窗口模式。
//!
//! 适合放在常开机器上用计划任务 / launchd / systemd 定时跑，也便于放进容器；
//! 段池、每日状态、订阅缓存与图形界面共用同一份数据目录，两个入口可以交替使用。
//! `cfbest-cli serve` 常驻运行局域网服务：按时自动优选，并提供网页管理界面与订阅地址。

// 无窗口模式只用到内核的一部分；其余函数留给图形界面，这里不报 dead_code。
#[allow(dead_code)]
#[path = "../core/mod.rs"]
mod core;
#[path = "../server/mod.rs"]
mod server;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use core::daily::{self, DailyConfig, DailyEvent};
use core::model::{DailyEntry, DailyVerification, Settings, VlessNode};
use core::region::RegionLock;
use core::storage::{self, AppPaths};
use core::verify;

const USAGE: &str = "\
cfbest-cli — CF Best 的无窗口模式（与图形界面共用配置与段池）

用法
  cfbest-cli [选项]          运行一轮自动优选后退出
  cfbest-cli serve [选项]    常驻运行局域网服务（网页管理 + 定时优选 + 订阅地址），见 serve --help

选项
  --segments <N>      本轮扫描的 /24 段数（默认取设置）
  --test <N>          进入测速的 IP 数（默认取设置）
  --top <N>           最终保留的 Top N（默认取设置）
  --concurrency <N>   扫描并发（默认取设置）
  --region <列表>     锁定出口区域：国家 / 地区代码或机房代码，逗号分隔，如 HK,JP,SJC；
                      all 表示不锁定（默认取设置）
  --out <PATH>        把结果另存到指定文件
  --json              以 JSON 输出到标准输出
  --quiet             不打印过程日志
  --reset             忽略上次进度，从段池第一段重新开始
  -h, --help          显示本帮助

输出
  Top 结果写到标准输出，过程日志写到标准错误，便于重定向与管道。

退出码
  0 有结果；3 本轮无可用 IP；1 运行出错；2 参数错误。
";

#[derive(Default)]
struct Args {
    segments: Option<usize>,
    test: Option<usize>,
    top: Option<usize>,
    concurrency: Option<usize>,
    region: Option<String>,
    out: Option<PathBuf>,
    json: bool,
    quiet: bool,
    reset: bool,
    help: bool,
}

fn take_value(
    argv: &mut impl Iterator<Item = String>,
    name: &str,
    inline: Option<String>,
) -> Result<String, String> {
    match inline {
        Some(value) => Ok(value),
        None => argv.next().ok_or_else(|| format!("{name} 缺少参数")),
    }
}

fn take_number(
    argv: &mut impl Iterator<Item = String>,
    name: &str,
    inline: Option<String>,
) -> Result<usize, String> {
    let text = take_value(argv, name, inline)?;
    text.parse::<usize>()
        .map_err(|_| format!("{name} 需要正整数：{text}"))
}

impl Args {
    fn parse(mut argv: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut args = Args::default();
        while let Some(raw) = argv.next() {
            let (key, inline) = match raw.split_once('=') {
                Some((key, value)) => (key.to_string(), Some(value.to_string())),
                None => (raw, None),
            };
            match key.as_str() {
                "--segments" => args.segments = Some(take_number(&mut argv, "--segments", inline)?),
                "--test" => args.test = Some(take_number(&mut argv, "--test", inline)?),
                "--top" => args.top = Some(take_number(&mut argv, "--top", inline)?),
                "--concurrency" => {
                    args.concurrency = Some(take_number(&mut argv, "--concurrency", inline)?);
                }
                "--region" => args.region = Some(take_value(&mut argv, "--region", inline)?),
                "--out" => {
                    args.out = Some(PathBuf::from(take_value(&mut argv, "--out", inline)?));
                }
                "--json" => args.json = true,
                "--quiet" | "-q" => args.quiet = true,
                "--reset" => args.reset = true,
                "-h" | "--help" => args.help = true,
                other => return Err(format!("未知参数：{other}")),
            }
        }
        Ok(args)
    }
}

const SERVE_USAGE: &str = "cfbest-cli serve — 常驻运行的局域网服务

在浏览器打开 http://<本机 IP>:27208 管理：立即运行、每日定时、设置订阅、查看结果与日志；
代理软件可直接添加页面上的订阅地址。设置与数据和图形界面共用同一个数据目录。

用法
  cfbest-cli serve [选项]

选项（也可用环境变量设置，选项优先）
  --listen <地址:端口>   监听地址，默认 0.0.0.0:27208            CFBEST_LISTEN
  --password <密码>      网页访问密码，默认不设                  CFBEST_PASSWORD
  --allow-host <域名>    未设密码时额外允许的访问域名，可重复     CFBEST_ALLOWED_HOSTS（逗号分隔）
  -h, --help             显示本帮助

其他环境变量
  CFBEST_DATA_DIR        数据目录（Docker 镜像中为 /data）
  CFBEST_SUB_URL         启动时写入的 VLESS 订阅链接（之后也可在网页上修改）
  TZ                     时区，决定每日定时的时间（Docker 镜像默认 Asia/Shanghai）
";

fn env_value(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn serve_main(argv: impl Iterator<Item = String>) -> ExitCode {
    let mut listen = env_value("CFBEST_LISTEN");
    let mut password = env_value("CFBEST_PASSWORD");
    let mut allowed_hosts: Vec<String> = env_value("CFBEST_ALLOWED_HOSTS")
        .map(|hosts| {
            hosts
                .split(',')
                .map(|host| host.trim().to_string())
                .filter(|host| !host.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let mut argv = argv;
    while let Some(raw) = argv.next() {
        let (key, inline) = match raw.split_once('=') {
            Some((key, value)) => (key.to_string(), Some(value.to_string())),
            None => (raw, None),
        };
        let result = match key.as_str() {
            "--listen" => {
                take_value(&mut argv, "--listen", inline).map(|value| listen = Some(value))
            }
            "--password" => {
                take_value(&mut argv, "--password", inline).map(|value| password = Some(value))
            }
            "--allow-host" => {
                take_value(&mut argv, "--allow-host", inline).map(|value| allowed_hosts.push(value))
            }
            "-h" | "--help" => {
                print!("{SERVE_USAGE}");
                return ExitCode::SUCCESS;
            }
            other => Err(format!("未知参数：{other}")),
        };
        if let Err(err) = result {
            eprintln!(
                "{err}
"
            );
            eprint!("{SERVE_USAGE}");
            return ExitCode::from(2);
        }
    }
    let listen = listen.unwrap_or_else(|| format!("0.0.0.0:{}", server::DEFAULT_PORT));
    let Ok(listen) = listen.parse::<SocketAddr>() else {
        eprintln!("监听地址无效：{listen}（应为 地址:端口，例如 0.0.0.0:27208）");
        return ExitCode::from(2);
    };

    let paths = AppPaths::discover();
    storage::init_logging(&paths);
    storage::install_panic_hook();
    core::sys::raise_fd_limit();
    if let Some(url) = env_value("CFBEST_SUB_URL") {
        let mut settings = storage::load_settings(&paths);
        if settings.sub_url != url {
            settings.sub_url = url;
            storage::save_settings(&paths, &settings);
            storage::log_line("已按 CFBEST_SUB_URL 设置订阅链接");
        }
    }
    let options = server::ServeOptions {
        listen,
        password: password.filter(|password| !password.is_empty()),
        allowed_hosts,
    };
    match server::run(paths, options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            storage::log_line(&format!("局域网服务启动失败：{err:#}"));
            ExitCode::FAILURE
        }
    }
}

fn main() -> ExitCode {
    let mut argv = std::env::args().skip(1).peekable();
    if argv.peek().is_some_and(|first| first == "serve") {
        argv.next();
        return serve_main(argv);
    }
    let args = match Args::parse(argv) {
        Ok(args) => args,
        Err(err) => {
            eprintln!("{err}\n");
            eprint!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    if args.help {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }

    let paths = AppPaths::discover();
    // 日志只写文件：控制台输出统一由下面的事件回调负责，避免同一条信息打印两遍。
    storage::set_console_logging(false);
    storage::init_logging(&paths);
    core::sys::raise_fd_limit();

    let settings = storage::load_settings(&paths);
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("无法启动异步运行时：{err}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(run(&paths, &settings, &args)) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(3),
        Err(err) => {
            eprintln!("运行失败：{err:#}");
            ExitCode::FAILURE
        }
    }
}

/// 返回 `Ok(true)` 表示本轮有结果，`Ok(false)` 表示扫到了但没有可用 IP。
async fn run(paths: &AppPaths, settings: &Settings, args: &Args) -> anyhow::Result<bool> {
    let mut state = storage::load_daily_state(paths);
    if args.reset {
        state.cursor = 0;
        state.round = 1;
    }

    let nodes = load_nodes(paths, &settings.sub_url);
    if nodes.is_empty() {
        eprintln!("提示：没有加载到 VLESS 订阅节点，本轮只做 TLS 基础验证（结果不代表节点可用）");
    }
    let node = verify::node_for_ips(&nodes);

    let config = DailyConfig {
        paths: paths.clone(),
        state,
        segments: args
            .segments
            .unwrap_or(settings.daily_segments)
            .clamp(1, 200),
        test: args.test.unwrap_or(settings.daily_test).clamp(1, 500),
        top: args.top.unwrap_or(settings.daily_top).clamp(1, 100),
        concurrency: args
            .concurrency
            .unwrap_or(settings.daily_concurrency as usize)
            .clamp(1, 1000),
        scan_timeout: Duration::from_secs_f64(settings.scan_timeout_secs.clamp(1.0, 30.0)),
        node,
        strategy: settings.pool_strategy.clone(),
        scores: storage::load_segment_scores(paths).records,
        region_lock: match args.region.as_deref().map(str::trim) {
            Some(text) if text.eq_ignore_ascii_case("all") => RegionLock::default(),
            Some(text) => RegionLock::parse_text(text),
            None => RegionLock::parse(&settings.region_lock),
        },
    };

    let quiet = args.quiet;
    let started = Instant::now();
    // 同一阶段同一进度会从上下游各报一次，这里记住上次打印的内容去重。
    let mut last_progress = String::new();
    let result = daily::execute(config, Arc::new(AtomicBool::new(false)), move |event| {
        if quiet {
            return;
        }
        match event {
            DailyEvent::Stage { stage, detail } => eprintln!("[{stage}] {detail}"),
            DailyEvent::Log(message) => eprintln!("{message}"),
            DailyEvent::Progress {
                stage,
                done,
                total,
                label,
            } => {
                if total == 0 || !(done == total || done % 500 == 0) {
                    return;
                }
                let line = format!("[{stage}] {done}/{total} {label}");
                if line != last_progress {
                    eprintln!("{line}");
                    last_progress = line;
                }
            }
        }
    })
    .await?;

    if args.json {
        let payload = serde_json::json!({
            "elapsed_secs": result.elapsed.as_secs_f64(),
            "scanned": result.summary.scanned,
            "hits": result.summary.hits,
            "verified": result.summary.verified,
            "top": result.top,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        print!("{}", render_text(&result.top));
    }

    if let Some(path) = &args.out {
        std::fs::write(path, render_text(&result.top))?;
        eprintln!("结果已另存到 {}", path.display());
    }
    if !quiet {
        eprintln!(
            "完成：扫描 {} 命中 {} 验证 {} 用时 {:.1}s",
            result.summary.scanned,
            result.summary.hits,
            result.summary.verified,
            started.elapsed().as_secs_f64()
        );
    }
    Ok(!result.top.is_empty())
}

fn load_nodes(paths: &AppPaths, sub_url: &str) -> Vec<VlessNode> {
    storage::load_subscription_cache(paths)
        .nodes_for(sub_url)
        .map(|nodes| nodes.to_vec())
        .unwrap_or_default()
}

fn render_text(entries: &[DailyEntry]) -> String {
    if entries.is_empty() {
        return "# 本轮无可用 IP\n".to_string();
    }
    let mut out = format!("# Top {}\n", entries.len());
    for entry in entries {
        out.push_str(&format!(
            "{}:{}#{}\t{:.1}Mbps\t{:.0}ms\t{}\t{}\n",
            entry.ip,
            entry.port,
            entry.colo,
            entry.best_mbps,
            entry.latency_ms,
            verification_label(&entry.verification),
            core::colo::place(&entry.colo),
        ));
    }
    out
}

fn verification_label(verification: &DailyVerification) -> &'static str {
    match verification {
        DailyVerification::Tunnel => "隧道验证",
        DailyVerification::Tls => "TLS 验证",
        DailyVerification::Unknown => "未验证",
    }
}
