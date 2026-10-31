//! 局域网服务（`cfbest-cli serve`）：常驻运行，按时自动优选，并在同一个端口上提供网页管理界面、
//! JSON 接口与订阅地址。给 NAS（Docker）、Linux 服务器这类没有图形界面的机器用。
//!
//! 与图形界面共用数据目录里的设置、段池、每日进度与订阅缓存；同一时间只允许一个实例使用该目录。

mod web;

use std::collections::VecDeque;
use std::fs::File;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use chrono::{Local, Timelike};
use serde::Serialize;
use tokio::net::TcpListener;

use crate::core::daily::{self, DailyConfig, DailyEvent, DailyResult, auto_daily_due};
use crate::core::model::{
    DailyEntry, DailyRun, DailyState, DailyVerification, History, Settings, SubscriptionCache,
    VlessNode,
};
use crate::core::proxy::{Endpoint, ProxyFeed};
use crate::core::region::{self, ClassifyConfig, CountryCount, RegionLock};
use crate::core::storage::{self, AppPaths};
use crate::core::sub_server::{self, FeedSource};
use crate::core::{colo, http, verify};

/// 默认端口：与图形界面的本机订阅服务（27207）错开，两者可以在同一台机器上共存。
pub const DEFAULT_PORT: u16 = 27208;
/// 网页上保留的日志行数。
const LOG_CAPACITY: usize = 400;
/// 检查是否到了每日自动优选时间的间隔。
const SCHEDULE_TICK: Duration = Duration::from_secs(20);

pub struct ServeOptions {
    pub listen: SocketAddr,
    /// 网页与接口的访问密码；订阅地址不受影响（令牌即凭证）。
    pub password: Option<String>,
    /// 未设密码时，除 IP 地址与 localhost 之外还允许的 Host（如 NAS 的域名）。
    pub allowed_hosts: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
struct LogLine {
    seq: u64,
    time: String,
    text: String,
}

#[derive(Default)]
struct LogRing {
    lines: VecDeque<LogLine>,
    next_seq: u64,
}

/// 正在运行的每日优选，网页上显示的进度。
#[derive(Clone, Debug, Serialize)]
struct JobView {
    trigger: &'static str,
    started: String,
    stage: String,
    detail: String,
    done: usize,
    total: usize,
    label: String,
}

#[derive(Default)]
struct SubscriptionView {
    /// `nodes` 对应的订阅地址；设置里换了地址后不再沿用。
    url: String,
    nodes: Vec<VlessNode>,
    time: String,
    loading: bool,
    error: String,
}

/// 段池的出口区域划分概况，在划分或优选结束后重新计算（段池可能很大，不在每次轮询时算）。
#[derive(Default)]
struct RegionView {
    pool: usize,
    /// 有划分记录的段数，以及其中有响应的段数。
    known: usize,
    alive: usize,
    classified: String,
    countries: Vec<CountryCount>,
}

impl RegionView {
    fn load(paths: &AppPaths) -> Self {
        let pool = storage::load_pool(paths);
        let regions = storage::load_segment_regions(paths);
        let (known, alive) = regions.coverage(&pool);
        Self {
            pool: pool.len(),
            known,
            alive,
            classified: regions.classified.clone(),
            countries: regions.countries(&pool),
        }
    }
}

struct Inner {
    settings: Settings,
    daily_state: DailyState,
    history: History,
    subscription: SubscriptionView,
    job: Option<JobView>,
    cancel: Option<Arc<AtomicBool>>,
    /// 上一次运行结束时的提示。
    last_result: String,
    regions: RegionView,
}

pub struct Service {
    paths: AppPaths,
    options: ServeOptions,
    inner: Mutex<Inner>,
    /// 与 `inner` 分开加锁：持有 `inner` 时也可以写日志。
    logs: Mutex<LogRing>,
}

/// 启动服务并一直运行。
pub fn run(paths: AppPaths, options: ServeOptions) -> Result<()> {
    let Some(lock) = storage::acquire_instance_lock(&paths, Duration::ZERO) else {
        bail!(
            "数据目录 {} 正被另一个 CF Best 使用（图形界面或另一个服务），请先退出它",
            paths.root.display()
        );
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("无法启动异步运行时")?;
    runtime.block_on(serve(paths, options, lock))
}

async fn serve(paths: AppPaths, options: ServeOptions, _lock: File) -> Result<()> {
    let listener = TcpListener::bind(options.listen)
        .await
        .with_context(|| format!("无法监听 {}", options.listen))?;
    let service = Arc::new(Service::load(paths, options));
    let sink = service.clone();
    storage::set_log_sink(move |time, text| sink.push_log(time, text));

    storage::log_line(&format!(
        "局域网服务已启动：http://{}（数据目录 {}）",
        service.options.listen,
        service.paths.root.display()
    ));
    if service.options.password.is_none() {
        storage::log_line(
            "未设置访问密码：局域网内的设备都能打开管理页面；可用 CFBEST_PASSWORD 设置密码",
        );
    }
    service.publish_feeds();
    tokio::spawn(service.clone().refresh_subscription());
    tokio::spawn(service.clone().schedule());
    web::serve(listener, service).await;
    Ok(())
}

impl Service {
    fn load(paths: AppPaths, options: ServeOptions) -> Self {
        let mut settings = storage::load_settings(&paths);
        if settings.proxy_feed_token.is_empty() {
            settings.proxy_feed_token = new_token();
            storage::save_settings(&paths, &settings);
        }
        sub_server::set_token(&settings.proxy_feed_token);
        let cache = storage::load_subscription_cache(&paths);
        let subscription = match cache.nodes_for(&settings.sub_url) {
            Some(nodes) => SubscriptionView {
                url: cache.url.trim().to_string(),
                nodes: nodes.to_vec(),
                time: cache.time.clone(),
                ..Default::default()
            },
            None => SubscriptionView::default(),
        };
        let inner = Inner {
            daily_state: storage::load_daily_state(&paths),
            history: storage::load_history(&paths),
            settings,
            subscription,
            job: None,
            cancel: None,
            last_result: String::new(),
            regions: RegionView::load(&paths),
        };
        Self {
            paths,
            options,
            inner: Mutex::new(inner),
            logs: Mutex::new(LogRing::default()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn push_log(&self, time: &str, text: &str) {
        let mut logs = self
            .logs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let seq = logs.next_seq;
        logs.next_seq += 1;
        if logs.lines.len() >= LOG_CAPACITY {
            logs.lines.pop_front();
        }
        logs.lines.push_back(LogLine {
            seq,
            time: time.to_string(),
            text: text.to_string(),
        });
    }

    /// 只记到网页日志（不写文件），且与上一行相同时跳过：每日优选的大部分日志
    /// 已经由 `storage::log_line` 记过，事件回调里会再收到一次。
    fn push_event_log(&self, text: &str) {
        let duplicate = self
            .logs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .lines
            .back()
            .is_some_and(|line| line.text == text);
        if !duplicate {
            let time = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
            self.push_log(&time, text);
        }
    }

    fn logs_since(&self, since: Option<u64>) -> Vec<LogLine> {
        let logs = self
            .logs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        logs.lines
            .iter()
            .filter(|line| since.is_none_or(|since| line.seq > since))
            .cloned()
            .collect()
    }

    fn save_settings(&self, settings: &Settings) {
        storage::save_settings(&self.paths, settings);
    }

    /// 拉取订阅（失败会自动重试），成功后写入缓存；失败时沿用同一地址上次成功的节点。
    async fn refresh_subscription(self: Arc<Self>) {
        let url = {
            let mut inner = self.lock();
            let url = inner.settings.sub_url.trim().to_string();
            if url.is_empty() {
                inner.subscription = SubscriptionView::default();
                drop(inner);
                self.publish_feeds();
                return;
            }
            if inner.subscription.loading {
                return;
            }
            inner.subscription.loading = true;
            inner.subscription.error.clear();
            url
        };
        storage::log_line("正在拉取订阅…");
        let result = http::fetch_subscription(&url, Duration::from_secs(20)).await;
        let message = {
            let mut inner = self.lock();
            inner.subscription.loading = false;
            if inner.settings.sub_url.trim() != url {
                // 拉取期间换了订阅地址，这份结果作废。
                return;
            }
            match result {
                Ok(nodes) => {
                    let time = Local::now().format("%Y-%m-%d %H:%M").to_string();
                    storage::save_subscription_cache(
                        &self.paths,
                        &SubscriptionCache {
                            url: url.clone(),
                            time: time.clone(),
                            nodes: nodes.clone(),
                        },
                    );
                    let message = format!(
                        "订阅已载入：{} 个节点，{}",
                        nodes.len(),
                        if template_node(&nodes).is_some() {
                            "使用 VLESS 隧道验证"
                        } else {
                            "没有 VLESS + WS 节点，只能做 TLS 基础验证"
                        }
                    );
                    inner.subscription = SubscriptionView {
                        url,
                        nodes,
                        time,
                        ..Default::default()
                    };
                    message
                }
                Err(err) => {
                    let error = format!("{err:#}");
                    inner.subscription.error = error.clone();
                    if inner.subscription.url != url {
                        inner.subscription.nodes.clear();
                    }
                    if inner.subscription.nodes.is_empty() {
                        format!("订阅拉取失败：{error}")
                    } else {
                        format!(
                            "订阅拉取失败，沿用 {} 载入的 {} 个节点：{error}",
                            inner.subscription.time,
                            inner.subscription.nodes.len()
                        )
                    }
                }
            }
        };
        storage::log_line(&message);
        self.publish_feeds();
    }

    /// 把最新的优选结果交给订阅服务；没有隧道验证通过的结果或 VLESS + WS 节点时，
    /// 客户端更新订阅会收到 503 并保留原有节点。
    fn publish_feeds(&self) {
        let feed = {
            let inner = self.lock();
            template_node(&inner.subscription.nodes).and_then(|template| {
                ProxyFeed::new(
                    FeedSource::Daily.title(),
                    inner.daily_state.last_run.clone(),
                    template,
                    tunnel_endpoints(&inner.daily_state.last_top),
                )
            })
        };
        sub_server::publish(FeedSource::Daily, feed);
        sub_server::publish(FeedSource::Verify, None);
    }

    /// 每隔一会儿检查一次：开启了每日自动优选、到点且当天还没运行过就开始，错过时间之后补跑。
    async fn schedule(self: Arc<Self>) {
        let mut ticker = tokio::time::interval(SCHEDULE_TICK);
        loop {
            ticker.tick().await;
            self.maybe_auto_daily();
        }
    }

    fn maybe_auto_daily(self: &Arc<Self>) {
        let now = Local::now();
        let today = now.format("%Y-%m-%d").to_string();
        let now_hm = (now.hour() as u8, now.minute() as u8);
        let note = {
            let mut inner = self.lock();
            let at = (inner.settings.auto_hour, inner.settings.auto_minute);
            // 有任务在运行、订阅还在载入时等它们结束。
            if !inner.settings.auto_daily
                || inner.job.is_some()
                || inner.subscription.loading
                || !auto_daily_due(&today, now_hm, &inner.settings.last_auto_date, at)
            {
                return;
            }
            inner.settings.last_auto_date = today;
            self.save_settings(&inner.settings);
            if now_hm > at {
                format!("补跑 {:02}:{:02} 的每日优选", at.0, at.1)
            } else {
                "按计划开始每日优选".to_string()
            }
        };
        storage::log_line(&note);
        if let Err(err) = self.start_daily("定时") {
            storage::log_line(&format!("自动优选启动失败：{err}"));
        }
    }

    /// 占用任务槽：每日优选与区域划分同一时间只运行一个。
    fn begin_job(&self, trigger: &'static str) -> Result<Arc<AtomicBool>, String> {
        let cancel = Arc::new(AtomicBool::new(false));
        let mut inner = self.lock();
        if let Some(job) = &inner.job {
            return Err(format!("正在{}，请等它结束或先停止", job.trigger_label()));
        }
        inner.job = Some(JobView {
            trigger,
            started: Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            stage: "准备".into(),
            detail: String::new(),
            done: 0,
            total: 0,
            label: String::new(),
        });
        inner.cancel = Some(cancel.clone());
        Ok(cancel)
    }

    /// 在后台开始一轮每日优选；已有任务在运行时返回错误。
    fn start_daily(self: &Arc<Self>, trigger: &'static str) -> Result<(), String> {
        let cancel = self.begin_job(trigger)?;
        tokio::spawn(self.clone().run_daily(trigger, cancel));
        Ok(())
    }

    /// 在后台划分段池的出口区域；`force` 为 true 时重新划分全部段，否则只划分新段。
    fn start_classify(self: &Arc<Self>, force: bool) -> Result<(), String> {
        if storage::load_pool(&self.paths).is_empty() {
            return Err("段池为空：先运行一轮优选，会自动生成段池".into());
        }
        let cancel = self.begin_job(CLASSIFY_TRIGGER)?;
        tokio::spawn(self.clone().run_classify(force, cancel));
        Ok(())
    }

    async fn run_classify(self: Arc<Self>, force: bool, cancel: Arc<AtomicBool>) {
        let pool = storage::load_pool(&self.paths);
        let mut regions = storage::load_segment_regions(&self.paths);
        let segments = regions.pending(&pool, force);
        let message = if segments.is_empty() {
            "段池里的段都已划分过；网络环境变了可以选「重新划分全部」".to_string()
        } else {
            storage::log_line(&format!(
                "开始划分出口区域：{} 段（{}）",
                segments.len(),
                if force {
                    "全部重新划分"
                } else {
                    "只划分新段"
                }
            ));
            self.set_stage(
                "区域划分",
                &format!("探测 {} 个段在本机网络下落到的机房", segments.len()),
            );
            let config = {
                let inner = self.lock();
                ClassifyConfig {
                    segments,
                    concurrency: inner.settings.daily_concurrency.clamp(1, 1000) as usize,
                    timeout: daily::classify_timeout(Duration::from_secs_f64(
                        inner.settings.scan_timeout_secs.clamp(0.5, 30.0),
                    )),
                }
            };
            let progress = self.clone();
            let outcome = region::classify(&config, cancel, move |step| {
                if let Some(job) = &mut progress.lock().job {
                    job.done = step.probed;
                    job.total = step.total;
                    job.label = format!("已划分 {} / {} 段", step.resolved, step.segments);
                }
            })
            .await;
            let now = Local::now().format("%Y-%m-%d %H:%M").to_string();
            region::apply_classification(&mut regions, &outcome, &now);
            storage::save_segment_regions(&self.paths, &regions);
            let countries = regions.countries(&pool);
            let spread = if countries.is_empty() {
                "都没有响应".to_string()
            } else {
                countries
                    .iter()
                    .take(6)
                    .map(|country| format!("{} {} 段", country.name, country.segments))
                    .collect::<Vec<_>>()
                    .join("、")
            };
            format!(
                "区域划分{}：{} 段中 {} 段有响应，{:.0}s；段池分布：{spread}",
                if outcome.stopped {
                    "已停止"
                } else {
                    "完成"
                },
                outcome.segments.len(),
                outcome.responded,
                outcome.elapsed.as_secs_f64()
            )
        };
        {
            let mut inner = self.lock();
            inner.job = None;
            inner.cancel = None;
            inner.last_result = message.clone();
            inner.regions = RegionView::load(&self.paths);
        }
        storage::log_line(&message);
    }

    fn stop_daily(&self) -> bool {
        match &self.lock().cancel {
            Some(cancel) => {
                cancel.store(true, Ordering::Relaxed);
                true
            }
            None => false,
        }
    }

    async fn run_daily(self: Arc<Self>, trigger: &'static str, cancel: Arc<AtomicBool>) {
        storage::log_line(&format!("开始每日优选（{trigger}）"));
        // 节点可能有变：先更新订阅，拉取失败时沿用上次的节点。
        if !self.lock().settings.sub_url.trim().is_empty() {
            self.set_stage("订阅", "更新订阅节点");
            self.clone().refresh_subscription().await;
            // 启动时或改设置后的拉取还没结束：等它的结果，而不是带着旧节点开跑。
            while self.lock().subscription.loading {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
        let config = {
            let inner = self.lock();
            let settings = &inner.settings;
            DailyConfig {
                paths: self.paths.clone(),
                state: inner.daily_state.clone(),
                segments: settings.daily_segments.clamp(1, 200),
                test: settings.daily_test.clamp(1, 500),
                top: settings.daily_top.clamp(1, 100),
                concurrency: settings.daily_concurrency.clamp(1, 1000) as usize,
                scan_timeout: Duration::from_secs_f64(settings.scan_timeout_secs.clamp(0.5, 30.0)),
                node: verify::node_for_ips(&inner.subscription.nodes),
                strategy: settings.pool_strategy.clone(),
                scores: storage::load_segment_scores(&self.paths).records,
                region_lock: RegionLock::parse(&settings.region_lock),
            }
        };
        let result = if cancel.load(Ordering::Relaxed) {
            Ok(DailyResult {
                stopped: true,
                ..Default::default()
            })
        } else {
            let events = self.clone();
            daily::execute(config, cancel, move |event| events.on_daily_event(event)).await
        };
        self.finish_daily(result);
    }

    fn set_stage(&self, stage: &str, detail: &str) {
        if let Some(job) = &mut self.lock().job {
            job.stage = stage.to_string();
            job.detail = detail.to_string();
            job.done = 0;
            job.total = 0;
            job.label.clear();
        }
    }

    fn on_daily_event(&self, event: DailyEvent) {
        match event {
            DailyEvent::Stage { stage, detail } => {
                self.set_stage(&stage, &detail);
                self.push_event_log(&format!("[{stage}] {detail}"));
            }
            DailyEvent::Progress {
                stage,
                done,
                total,
                label,
            } => {
                if let Some(job) = &mut self.lock().job {
                    job.stage = stage;
                    job.done = done;
                    job.total = total;
                    job.label = label;
                }
            }
            DailyEvent::Log(text) => self.push_event_log(&text),
        }
    }

    fn finish_daily(&self, result: Result<DailyResult>) {
        let message = {
            let mut inner = self.lock();
            inner.job = None;
            inner.cancel = None;
            // 优选会生成段池、更新扫描过的段的划分。
            inner.regions = RegionView::load(&self.paths);
            let message = match result {
                Ok(result) if result.stopped && result.summary.scanned == 0 => {
                    "每日优选已停止".to_string()
                }
                Ok(result) => {
                    inner.daily_state = result.state.clone();
                    if result.summary.scanned > 0 {
                        inner.history.daily.insert(
                            0,
                            DailyRun {
                                time: Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
                                top: result.top.clone(),
                                scanned: result.summary.scanned,
                                hits: result.summary.hits,
                                tested: result.summary.tested,
                            },
                        );
                        storage::trim_history(&mut inner.history);
                        storage::save_history(&self.paths, &inner.history);
                    }
                    let mut message = if result.top.is_empty() {
                        format!(
                            "每日优选完成：本轮无可用 IP（命中 {}，测速 {}）",
                            result.summary.hits, result.summary.tested
                        )
                    } else {
                        format!(
                            "每日优选完成：Top {}，扫描命中 {}",
                            result.top.len(),
                            result.summary.hits
                        )
                    };
                    message.push_str(&format!(
                        " · {} · {:.0}s",
                        result.summary.verification.label(),
                        result.elapsed.as_secs_f64()
                    ));
                    if result.stopped {
                        message.push_str("（已停止）");
                    }
                    message
                }
                Err(err) => format!("每日优选失败：{err:#}"),
            };
            inner.last_result = message.clone();
            message
        };
        storage::log_line(&message);
        self.publish_feeds();
    }

    /// 网页修改设置。只接受这里列出的字段，范围与图形界面一致；返回改动说明。
    fn update_settings(self: &Arc<Self>, patch: &serde_json::Value) -> Result<String, String> {
        let mut inner = self.lock();
        let mut settings = inner.settings.clone();
        let number = |key: &str, min: u64, max: u64| -> Result<Option<u64>, String> {
            match patch.get(key) {
                None => Ok(None),
                Some(value) => value
                    .as_u64()
                    .filter(|value| (min..=max).contains(value))
                    .map(Some)
                    .ok_or_else(|| format!("{key} 应为 {min}–{max} 的整数")),
            }
        };
        if let Some(value) = number("daily_segments", 1, 200)? {
            settings.daily_segments = value as usize;
        }
        if let Some(value) = number("daily_test", 1, 500)? {
            settings.daily_test = value as usize;
        }
        if let Some(value) = number("daily_top", 1, 100)? {
            settings.daily_top = value as usize;
        }
        if let Some(value) = number("daily_concurrency", 1, 1000)? {
            settings.daily_concurrency = value as u32;
        }
        if let Some(value) = patch.get("scan_timeout_secs") {
            settings.scan_timeout_secs = value
                .as_f64()
                .filter(|value| (0.5..=30.0).contains(value))
                .ok_or("scan_timeout_secs 应在 0.5–30 秒之间")?;
        }
        if let Some(value) = patch.get("pool_strategy") {
            let value = value.as_str().unwrap_or("");
            if !["balanced", "sequential", "network"].contains(&value) {
                return Err("pool_strategy 应为 balanced、sequential 或 network".into());
            }
            settings.pool_strategy = value.to_string();
        }
        if let Some(value) = patch.get("sub_url") {
            let url = value.as_str().ok_or("sub_url 应为字符串")?.trim();
            if !url.is_empty() && !url.starts_with("http://") && !url.starts_with("https://") {
                return Err("订阅链接应以 http:// 或 https:// 开头".into());
            }
            settings.sub_url = url.to_string();
        }
        if let Some(value) = patch.get("region_lock") {
            let entries: Vec<&str> = value
                .as_array()
                .ok_or("region_lock 应为字符串数组")?
                .iter()
                .filter_map(serde_json::Value::as_str)
                .collect();
            settings.region_lock = RegionLock::parse(&entries).entries();
        }
        if let Some(value) = patch.get("auto_daily") {
            settings.auto_daily = value.as_bool().ok_or("auto_daily 应为 true 或 false")?;
        }
        if let Some(value) = patch.get("auto_time") {
            let (hour, minute) = parse_hhmm(value.as_str().unwrap_or(""))?;
            settings.auto_hour = hour;
            settings.auto_minute = minute;
        }
        // 刚开启或改了时间：今天的时间已过就从明天开始，避免一保存就立即运行。
        let schedule_changed = settings.auto_daily
            && (!inner.settings.auto_daily
                || (settings.auto_hour, settings.auto_minute)
                    != (inner.settings.auto_hour, inner.settings.auto_minute));
        let mut deferred = false;
        if schedule_changed {
            let now = Local::now();
            let today = now.format("%Y-%m-%d").to_string();
            deferred = auto_daily_due(
                &today,
                (now.hour() as u8, now.minute() as u8),
                &settings.last_auto_date,
                (settings.auto_hour, settings.auto_minute),
            );
            if deferred {
                settings.last_auto_date = today;
            }
        }
        let sub_changed = settings.sub_url != inner.settings.sub_url;
        let lock_changed = settings.region_lock != inner.settings.region_lock;
        inner.settings = settings;
        self.save_settings(&inner.settings);
        let mut message = "设置已保存".to_string();
        if lock_changed {
            message.push_str(&format!(
                "；出口区域：{}",
                RegionLock::parse(&inner.settings.region_lock).label()
            ));
        }
        if deferred {
            message.push_str(&format!(
                "；今天的 {:02}:{:02} 已过，明天开始自动优选",
                inner.settings.auto_hour, inner.settings.auto_minute
            ));
        }
        drop(inner);
        storage::log_line(&message);
        if sub_changed {
            tokio::spawn(self.clone().refresh_subscription());
        }
        Ok(message)
    }

    /// 换一个订阅令牌：旧的订阅地址随即失效，客户端需要重新添加。
    fn reset_token(&self) {
        let mut inner = self.lock();
        inner.settings.proxy_feed_token = new_token();
        sub_server::set_token(&inner.settings.proxy_feed_token);
        self.save_settings(&inner.settings);
        drop(inner);
        storage::log_line("订阅令牌已更换，旧的订阅地址已失效");
    }

    /// 网页轮询的状态。`since` 为上次收到的最后一条日志序号。
    fn snapshot(&self, since: Option<u64>) -> serde_json::Value {
        let inner = self.lock();
        let settings = &inner.settings;
        let template = template_node(&inner.subscription.nodes);
        let feed_count = tunnel_endpoints(&inner.daily_state.last_top).len();
        let lock = RegionLock::parse(&settings.region_lock);
        let state = &inner.daily_state;
        let locked_progress = !lock.is_empty() && state.region_key == lock.key();
        let top: Vec<_> = state
            .last_top
            .iter()
            .map(|entry| {
                serde_json::json!({
                    "ip": entry.ip,
                    "port": entry.port,
                    "colo": entry.colo,
                    "place": colo::place(&entry.colo),
                    "best_mbps": entry.best_mbps,
                    "avg_mbps": entry.avg_mbps,
                    "latency_ms": entry.latency_ms,
                    "verification": entry.verification,
                })
            })
            .collect();
        let countries: Vec<_> = inner
            .regions
            .countries
            .iter()
            .map(|country| {
                serde_json::json!({
                    "code": country.code,
                    "name": country.name,
                    "region": colo::region_name(country.region),
                    "segments": country.segments,
                    "colos": country.colos,
                })
            })
            .collect();
        let history: Vec<_> = inner
            .history
            .daily
            .iter()
            .take(30)
            .map(|run| {
                serde_json::json!({
                    "time": run.time,
                    "scanned": run.scanned,
                    "hits": run.hits,
                    "tested": run.tested,
                    "top": run.top.len(),
                    "best_mbps": run.top.iter().map(|entry| entry.best_mbps).fold(0.0, f64::max),
                })
            })
            .collect();
        serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "data_dir": self.paths.root.display().to_string(),
            "password": self.options.password.is_some(),
            "job": inner.job,
            "last_result": inner.last_result,
            "next_run": next_run(settings),
            "settings": {
                "sub_url": settings.sub_url,
                "auto_daily": settings.auto_daily,
                "auto_time": format!("{:02}:{:02}", settings.auto_hour, settings.auto_minute),
                "daily_segments": settings.daily_segments,
                "daily_test": settings.daily_test,
                "daily_top": settings.daily_top,
                "daily_concurrency": settings.daily_concurrency,
                "scan_timeout_secs": settings.scan_timeout_secs,
                "pool_strategy": crate::core::pool::normalize_strategy(&settings.pool_strategy),
                "region_lock": lock.entries(),
            },
            "region_lock": {
                "label": lock.label(),
                "countries": lock.countries(),
                "colos": lock.colos(),
            },
            "regions": {
                "pool": inner.regions.pool,
                "known": inner.regions.known,
                "alive": inner.regions.alive,
                "classified": inner.regions.classified,
                "countries": countries,
            },
            "subscription": {
                "count": inner.subscription.nodes.len(),
                "time": inner.subscription.time,
                "loading": inner.subscription.loading,
                "error": inner.subscription.error,
                "node": template.as_ref().map(VlessNode::summary),
            },
            "daily": {
                "last_run": state.last_run,
                "top": top,
                "verification": state.last_top.first()
                    .map_or(DailyVerification::Unknown, |entry| entry.verification),
                "locked": locked_progress,
                "cursor": if locked_progress { state.region_cursor } else { state.cursor },
                "round": if locked_progress { state.region_round } else { state.round },
                "pool_count": if locked_progress { state.region_count } else { state.pool_count },
            },
            "feed": {
                "token": settings.proxy_feed_token,
                "count": if template.is_some() { feed_count } else { 0 },
            },
            "history": history,
            "logs": self.logs_since(since),
        })
    }

    /// 最新 Top 结果，每行 `IP:端口#机房`，便于其他工具直接读取。
    fn top_text(&self) -> String {
        let inner = self.lock();
        inner
            .daily_state
            .last_top
            .iter()
            .map(|entry| format!("{}:{}#{}\n", entry.ip, entry.port, entry.colo))
            .collect()
    }
}

/// 隧道验证与订阅生成用的节点模板：第一个 VLESS + WS 节点（与 `verify::node_for_ips` 一致）。
fn template_node(nodes: &[VlessNode]) -> Option<VlessNode> {
    let node = verify::node_for_ips(nodes);
    node.transport.eq_ignore_ascii_case("ws").then_some(node)
}

fn tunnel_endpoints(rows: &[DailyEntry]) -> Vec<Endpoint> {
    rows.iter()
        .filter(|entry| entry.verification == DailyVerification::Tunnel)
        .map(|entry| Endpoint {
            ip: entry.ip.clone(),
            port: entry.port,
            colo: entry.colo.clone(),
        })
        .collect()
}

/// 区域划分任务在任务槽里的触发方式，网页据此显示「区域划分」而不是「手动 / 定时」。
const CLASSIFY_TRIGGER: &str = "区域划分";

impl JobView {
    fn trigger_label(&self) -> &'static str {
        if self.trigger == CLASSIFY_TRIGGER {
            "划分出口区域"
        } else {
            "运行优选"
        }
    }
}

fn new_token() -> String {
    format!("{:032x}", rand::random::<u128>())
}

/// 下次自动运行的时间说明；未开启时为 None。
fn next_run(settings: &Settings) -> Option<String> {
    if !settings.auto_daily {
        return None;
    }
    let now = Local::now();
    let today = now.format("%Y-%m-%d").to_string();
    let at = (settings.auto_hour, settings.auto_minute);
    let time = format!("{:02}:{:02}", at.0, at.1);
    Some(if settings.last_auto_date == today {
        format!("明天 {time}")
    } else if (now.hour() as u8, now.minute() as u8) < at {
        format!("今天 {time}")
    } else {
        "即将补跑".to_string()
    })
}

fn parse_hhmm(text: &str) -> Result<(u8, u8), String> {
    let invalid = || "时间格式应为 HH:MM（00:00–23:59）".to_string();
    let (hour, minute) = text.trim().split_once(':').ok_or_else(invalid)?;
    let hour = hour.parse::<u8>().map_err(|_| invalid())?;
    let minute = minute.parse::<u8>().map_err(|_| invalid())?;
    if hour > 23 || minute > 59 {
        return Err(invalid());
    }
    Ok((hour, minute))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hhmm_accepts_only_valid_times() {
        assert_eq!(parse_hhmm(" 06:30 "), Ok((6, 30)));
        assert_eq!(parse_hhmm("0:0"), Ok((0, 0)));
        assert!(parse_hhmm("24:00").is_err());
        assert!(parse_hhmm("12:60").is_err());
        assert!(parse_hhmm("1230").is_err());
    }

    #[test]
    fn only_tunnel_verified_rows_become_feed_endpoints() {
        let entry = |ip: &str, verification| DailyEntry {
            ip: ip.into(),
            port: 443,
            colo: "HKG".into(),
            verification,
            ..Default::default()
        };
        let rows = [
            entry("104.16.1.1", DailyVerification::Tunnel),
            entry("104.16.1.2", DailyVerification::Tls),
        ];
        let endpoints = tunnel_endpoints(&rows);
        assert_eq!(endpoints.len(), 1);
        assert_eq!(endpoints[0].ip, "104.16.1.1");
    }

    #[test]
    fn next_run_is_none_when_auto_daily_is_off() {
        let settings = Settings::default();
        assert!(next_run(&settings).is_none());
        let today = Local::now().format("%Y-%m-%d").to_string();
        let settings = Settings {
            auto_daily: true,
            last_auto_date: today,
            ..Settings::default()
        };
        assert!(next_run(&settings).unwrap().starts_with("明天"));
    }
}
