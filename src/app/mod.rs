mod actions;
mod columns;
mod daily_page;
mod events;
mod history_page;
mod pool_actions;
mod pool_page;
mod proxy_import;
mod region_lock;
mod scan_page;
mod sidebar;
mod speed_page;
mod theme_panel;
mod toast;
mod update_panel;
mod verify_page;
mod window_chrome;

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use chrono::{Local, Timelike};
use gpui::{
    App, ClickEvent, Context, Entity, MouseButton, MouseDownEvent, Render, SharedString, Window,
    actions, div, prelude::*,
};

use crate::theme::px;
use tokio::sync::mpsc::UnboundedSender;

use crate::core::daily::auto_daily_due;
use crate::core::model::{
    DailyEntry, DailyState, DailySummary, History, ParamGroup, ScanHit, SegmentScores, Settings,
    SpeedRecord, SubscriptionCache, VerifyRecord, VlessNode,
};
use crate::core::storage::{self, AppPaths};
use crate::core::verify::VerifyTarget;
use crate::core::{ip, pool};
use crate::jobs::{self, JobEvent, JobKind};
use crate::theme;
use crate::ui::input::{TextArea, TextInput};
use crate::ui::widgets::{self, Tone};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Scan,
    Speed,
    Verify,
    Daily,
    Pool,
    History,
}

impl Page {
    fn title(self) -> &'static str {
        match self {
            Page::Scan => "扫描 IP",
            Page::Speed => "测速排行",
            Page::Verify => "可用性验证",
            Page::Daily => "自动优选",
            Page::Pool => "段池优化",
            Page::History => "历史记录",
        }
    }

    fn subtitle(self) -> &'static str {
        match self {
            Page::Scan => "从 IP 段中找出可以连通的 Cloudflare 地址",
            Page::Speed => "测出下载速度最快的 IP",
            Page::Verify => "确认 IP 能通过代理节点正常访问网络",
            Page::Daily => "自动扫描、测速、验证，并整理可用的 IP",
            Page::Pool => "调整段池顺序与起点，并粗测各段在本机网络下的表现",
            Page::History => "查看之前的运行结果",
        }
    }

    fn step(self) -> Option<&'static str> {
        match self {
            Page::Scan => Some("步骤 1 / 3"),
            Page::Speed => Some("步骤 2 / 3"),
            Page::Verify => Some("步骤 3 / 3"),
            Page::Daily | Page::Pool | Page::History => None,
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Page::Scan => "search",
            Page::Speed => "gauge",
            Page::Verify => "shield-check",
            Page::Daily => "calendar",
            Page::Pool => "layers",
            Page::History => "history",
        }
    }
}

actions!(
    cf_app,
    [
        SwitchScan,
        SwitchSpeed,
        SwitchVerify,
        SwitchDaily,
        SwitchPool,
        SwitchHistory,
        StartCurrent,
        StopCurrent,
        ToggleAdvanced,
        ToggleSidebar,
        FocusNext,
        FocusPrev,
        ZoomIn,
        ZoomOut,
        ZoomReset,
        Quit
    ]
);

#[derive(Clone, Debug, Default)]
pub struct ProgressState {
    pub done: usize,
    pub total: usize,
    pub label: String,
}

impl ProgressState {
    fn fraction(&self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            (self.done as f32 / self.total as f32).clamp(0., 1.)
        }
    }
}

#[derive(Clone, Debug)]
struct StatusLine {
    text: String,
    tone: Tone,
    at: Instant,
    time: String,
    faded: bool,
}

impl StatusLine {
    fn new(text: impl Into<String>, tone: Tone) -> Self {
        Self {
            text: text.into(),
            tone,
            at: Instant::now(),
            time: Local::now().format("%H:%M:%S").to_string(),
            faded: false,
        }
    }

    /// 普通消息 8 秒后淡出，错误与警告保留 30 秒。
    fn fade_after(&self) -> Duration {
        match self.tone {
            Tone::Red | Tone::Amber => Duration::from_secs(30),
            _ => Duration::from_secs(8),
        }
    }
}

const STATUS_HISTORY_LIMIT: usize = 30;

/// 工作区面板与窗口边缘的间距。和窗口圆角配套，不随界面缩放。
const WORKBENCH_INSET: f32 = 4.0;
/// 面板右侧两角取与窗口圆角相同的弧度。同心圆角（窗口圆角 − 间距，3px）能让间隙沿弧线等宽，
/// 但面板角显得过尖；相同弧度下转角处的间隙只比边上宽约 1.7px。
const WORKBENCH_OUTER_RADIUS: f32 = theme::radius::WINDOW;

pub struct AppState {
    page: Page,
    root_focus: gpui::FocusHandle,
    main_scroll: gpui::ScrollHandle,
    theme_panel_open: bool,
    /// 每次切换页面递增，让侧栏选中指示重新播放弹入动画。
    nav_anim_seq: u64,
    /// 侧栏宽度动画：每次收起 / 展开递增。
    sidebar_anim_seq: u64,
    /// 弹层因点击外部而关闭的时刻；用于忽略同一次点击落在「主题」行上的再次切换。
    theme_panel_closed_at: Option<Instant>,
    paths: AppPaths,
    settings: Settings,
    history: History,
    daily_state: DailyState,
    events_tx: UnboundedSender<JobEvent>,
    job: Option<JobKind>,
    aux_job: Option<JobKind>,
    cancel: Option<Arc<AtomicBool>>,
    status: StatusLine,
    /// 最近的状态消息（新的在前），点击状态栏查看。
    status_history: VecDeque<StatusLine>,
    status_history_open: bool,
    status_history_closed_at: Option<Instant>,
    column_drag: Option<columns::ColumnDrag>,
    /// 调试覆盖生效时为 true：不写入设置。
    settings_frozen: bool,
    /// 历史对比的基准运行（`history.speed` 的下标，0 为最近一次）。
    history_compare_base: usize,
    toast: Option<toast::Toast>,
    toast_seq: u64,
    taskbar: Option<crate::taskbar::Taskbar>,
    tray: Option<crate::tray::Tray>,
    /// 本次运行中是否已提示过「仍在后台运行」。
    tray_hint_shown: bool,

    scan_input: Entity<TextArea>,
    scan_ports: Entity<TextInput>,
    scan_concurrency: Entity<TextInput>,
    scan_timeout: Entity<TextInput>,
    scan_sample: Entity<TextInput>,
    scan_asn: Entity<TextInput>,
    scan_hits: Vec<ScanHit>,
    scan_progress: ProgressState,
    scan_note: String,

    speed_input: Entity<TextArea>,
    speed_coarse: Entity<TextInput>,
    speed_fine: Entity<TextInput>,
    speed_rounds: Entity<TextInput>,
    speed_topn: Entity<TextInput>,
    speed_records: Vec<SpeedRecord>,
    speed_phase: String,
    speed_current: String,
    speed_throttled: u32,
    speed_warnings: Vec<String>,
    speed_progress: ProgressState,
    speed_sort: (speed_page::SpeedSortKey, bool),
    speed_view_order: Vec<usize>,

    sub_url: Entity<TextInput>,
    verify_input: Entity<TextArea>,
    verify_records: Vec<VerifyRecord>,
    verify_nodes: Vec<VlessNode>,
    verify_node_source: String,
    verify_progress: ProgressState,
    /// 本机已安装、支持一键导入的代理软件；进入验证 / 自动优选页时重新检测。
    proxy_clients: Vec<crate::proxy_clients::ProxyClient>,
    /// 订阅正在后台拉取（含失败重试）。不占任务槽，扫描、测速照常可用。
    sub_loading: bool,
    /// 最近一次成功拉取的订阅节点（`subscription.json`），拉取失败时沿用。
    sub_cache: SubscriptionCache,
    /// 检查更新的状态与弹层（见 update_panel.rs）。
    update: update_panel::UpdateState,
    update_cancel: Option<Arc<AtomicBool>>,
    update_popover_open: bool,
    update_popover_closed_at: Option<Instant>,
    /// 上次检查更新的时刻；自动检查每 24 小时一次。
    last_update_check: Option<Instant>,

    daily_segments: Entity<TextInput>,
    daily_test: Entity<TextInput>,
    daily_top: Entity<TextInput>,
    daily_concurrency: Entity<TextInput>,
    daily_auto_time: Entity<TextInput>,
    daily_stage: String,
    daily_detail: String,
    daily_progress: ProgressState,
    daily_logs: VecDeque<String>,
    daily_top_rows: Vec<DailyEntry>,
    daily_summary: Option<DailySummary>,
    pool_segments: Vec<String>,
    pool_order: Vec<String>,
    score_map: HashMap<String, f64>,
    segment_scores: SegmentScores,
    /// 段池各段的出口区域划分（`segment_regions.json`）与按国家 / 地区的统计。
    segment_regions: crate::core::model::SegmentRegions,
    region_countries: Vec<crate::core::region::CountryCount>,
    /// 锁定出口区域：标签之外的国家 / 地区或机房代码。
    region_extra: Entity<TextInput>,
    classify_progress: ProgressState,
    classify_detail: String,
    /// 扫描结果按国家 / 地区筛选（国家代码）；None 显示全部。
    scan_region: Option<String>,
    pool_cursor: Entity<TextInput>,
    probe_samples: Entity<TextInput>,
    probe_concurrency: Entity<TextInput>,
    probe_top: Entity<TextInput>,
    probe_seconds: Entity<TextInput>,
    probe_stage: String,
    probe_detail: String,
    probe_progress: ProgressState,
    reset_armed_at: Option<Instant>,
    /// 已到每日自动优选时间，但有任务在运行，正等待空闲。
    auto_daily_waiting: bool,
    /// 自动运行时间输入框失焦时提交修改；首次渲染时注册（需要 Window）。
    daily_time_blur: Option<gpui::Subscription>,
}

fn text_input(
    cx: &mut Context<AppState>,
    value: impl Into<SharedString>,
    placeholder: impl Into<SharedString>,
) -> Entity<TextInput> {
    cx.new(|cx| TextInput::new(cx, value, placeholder))
}

fn text_area(
    cx: &mut Context<AppState>,
    value: impl Into<SharedString>,
    placeholder: impl Into<SharedString>,
) -> Entity<TextArea> {
    cx.new(|cx| TextArea::new(cx, value, placeholder))
}

fn float_text(value: f64) -> String {
    if (value.fract()).abs() < f64::EPSILON {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    }
}

impl AppState {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let _ = jobs::runtime();
        let paths = AppPaths::discover();
        let mut settings = storage::load_settings(&paths);
        settings.pool_strategy = pool::normalize_strategy(&settings.pool_strategy).to_string();
        let initial_theme = theme::Theme::from_id(&settings.theme);
        theme::set_theme(initial_theme);
        theme::set_ui_scale(settings.ui_scale);
        settings.theme = initial_theme.id().to_string();
        let history = storage::load_history(&paths);
        // 恢复上次的手动结果：扫描命中单独保存，测速与验证取历史记录中最近的一次。
        let last_scan = storage::load_last_scan(&paths);
        let last_speed = history.speed.first().cloned();
        let last_verify = history.verify.first().cloned();
        let pool_segments = storage::load_pool(&paths);
        let segment_scores = storage::load_segment_scores(&paths);
        let sub_cache = storage::load_subscription_cache(&paths);
        let mut daily_state = storage::load_daily_state(&paths);
        if daily_state.round == 0 {
            daily_state.round = 1;
        }
        // 旧版游标按原顺序分配；升级后回到通用推荐起点，避免从半途开始。
        if daily_state.strategy.is_empty() || daily_state.strategy != settings.pool_strategy {
            daily_state.cursor = 0;
            daily_state.round = 1;
        }
        daily_state.strategy = settings.pool_strategy.clone();
        daily_state.pool_count = pool_segments.len();
        if daily_state.pool_count > 0 && daily_state.cursor >= daily_state.pool_count {
            daily_state.cursor %= daily_state.pool_count;
        }
        let pool_order = pool::ordered_pool(
            &pool_segments,
            &settings.pool_strategy,
            &segment_scores.records,
        );
        let score_map: HashMap<String, f64> = segment_scores
            .records
            .iter()
            .map(|record| (record.segment.clone(), record.score))
            .collect();
        let (events_tx, events_rx) = tokio::sync::mpsc::unbounded_channel();

        let scan_input =
            cx.new(|cx| TextArea::new(cx, "", "每行一个 IP 段，例如 104.16.0.0/24").frameless());
        let scan_ports = text_input(cx, settings.scan_ports.clone(), "443, 8443");
        let scan_concurrency = text_input(cx, settings.scan_concurrency.to_string(), "300");
        let scan_timeout = text_input(cx, float_text(settings.scan_timeout_secs), "5");
        let scan_sample = text_input(cx, float_text(settings.scan_sample_pct), "0");
        let scan_asn = text_input(cx, "", "AS906");

        let speed_input = text_area(
            cx,
            "",
            "每行一个 IP 或 IP:端口，可从扫描结果或官方候选池载入",
        );
        let speed_coarse = text_input(cx, settings.speed_coarse_secs.to_string(), "2");
        let speed_fine = text_input(cx, settings.speed_fine_secs.to_string(), "6");
        let speed_rounds = text_input(cx, settings.speed_fine_rounds.to_string(), "3");
        let speed_topn = text_input(cx, settings.speed_fine_topn.to_string(), "16");

        let sub_url = text_input(cx, settings.sub_url.clone(), "粘贴订阅链接，例如 https://…");
        let verify_input = text_area(
            cx,
            "",
            "待验证的 IP，每行一条（留空则验证已拉取的订阅节点）",
        );

        let segment_regions = storage::load_segment_regions(&paths);
        let region_countries = segment_regions.countries(&pool_segments);
        let region_extra = {
            let known: Vec<&str> = region_countries
                .iter()
                .map(|country| country.code.as_str())
                .collect();
            let extra: Vec<&str> = settings
                .region_lock
                .iter()
                .map(String::as_str)
                .filter(|code| !known.contains(code))
                .collect();
            text_input(cx, extra.join(", "), "例如 SG, NRT, SJC")
        };
        let daily_segments = text_input(cx, settings.daily_segments.to_string(), "20");
        let daily_test = text_input(cx, settings.daily_test.to_string(), "20");
        let daily_top = text_input(cx, settings.daily_top.to_string(), "5");
        let daily_concurrency = text_input(cx, settings.daily_concurrency.to_string(), "300");
        let daily_auto_time = text_input(
            cx,
            format!("{:02}:{:02}", settings.auto_hour, settings.auto_minute),
            "09:00",
        );
        let pool_cursor = text_input(cx, (daily_state.cursor + 1).to_string(), "1");
        let probe_samples = text_input(cx, settings.probe_samples.to_string(), "2");
        let probe_concurrency = text_input(cx, settings.probe_concurrency.to_string(), "60");
        let probe_top = text_input(cx, settings.probe_top.to_string(), "8");
        let probe_seconds = text_input(cx, settings.probe_seconds.to_string(), "2");

        let mut daily_logs = VecDeque::new();
        daily_logs.push_front(format!("数据目录：{}", paths.root.display()));
        if !daily_state.last_top.is_empty() {
            daily_logs.push_front(format!(
                "上次优选：{} · Top {}",
                daily_state.last_run,
                daily_state.last_top.len()
            ));
        }

        let root_focus = cx.focus_handle();
        let mut app = Self {
            page: Page::Daily,
            root_focus,
            main_scroll: gpui::ScrollHandle::new(),
            theme_panel_open: false,
            nav_anim_seq: 0,
            sidebar_anim_seq: 0,
            theme_panel_closed_at: None,
            paths,
            settings,
            history,
            daily_state: daily_state.clone(),
            events_tx,
            job: None,
            aux_job: None,
            cancel: None,
            status: StatusLine::new("就绪", Tone::Green),
            toast: None,
            status_history: VecDeque::new(),
            status_history_open: false,
            status_history_closed_at: None,
            column_drag: None,
            settings_frozen: false,
            history_compare_base: 1,
            toast_seq: 0,
            taskbar: crate::taskbar::Taskbar::new(),
            tray: None,
            tray_hint_shown: false,
            scan_input,
            scan_ports,
            scan_concurrency,
            scan_timeout,
            scan_sample,
            scan_asn,
            scan_progress: if last_scan.hits.is_empty() {
                ProgressState::default()
            } else {
                ProgressState {
                    done: last_scan.total,
                    total: last_scan.total,
                    label: format!(
                        "上次扫描 {}：命中 {} 个",
                        last_scan.time,
                        last_scan.hits.len()
                    ),
                }
            },
            scan_note: if last_scan.hits.is_empty() {
                "尚未开始扫描".into()
            } else {
                format!(
                    "上次扫描 {}：命中 {} 个",
                    last_scan.time,
                    last_scan.hits.len()
                )
            },
            scan_hits: last_scan.hits,
            speed_input,
            speed_coarse,
            speed_fine,
            speed_rounds,
            speed_topn,
            speed_progress: match &last_speed {
                Some(run) => ProgressState {
                    done: run.results.len(),
                    total: run.results.len(),
                    label: format!("上次测速 {}", run.time),
                },
                None => ProgressState::default(),
            },
            speed_records: last_speed.map(|run| run.results).unwrap_or_default(),
            speed_phase: "空闲".into(),
            speed_current: String::new(),
            speed_throttled: 0,
            speed_warnings: Vec::new(),
            speed_sort: (speed_page::SpeedSortKey::Median, true),
            speed_view_order: Vec::new(),
            sub_url,
            verify_input,
            verify_progress: match &last_verify {
                Some(run) => ProgressState {
                    done: run.total,
                    total: run.total,
                    label: format!("上次验证 {}：{}/{} 通过", run.time, run.passed, run.total),
                },
                None => ProgressState::default(),
            },
            verify_records: last_verify.map(|run| run.records).unwrap_or_default(),
            verify_nodes: Vec::new(),
            verify_node_source: "尚未配置订阅".into(),
            proxy_clients: crate::proxy_clients::detect(),
            sub_loading: false,
            sub_cache,
            update: Default::default(),
            update_cancel: None,
            update_popover_open: false,
            update_popover_closed_at: None,
            last_update_check: None,
            daily_segments,
            daily_test,
            daily_top,
            daily_concurrency,
            daily_auto_time,
            daily_stage: if daily_state.last_top.is_empty() {
                "空闲".into()
            } else {
                "上次结果".into()
            },
            daily_detail: if daily_state.last_run.is_empty() {
                "尚未运行".into()
            } else {
                format!("上次运行 {}", daily_state.last_run)
            },
            daily_progress: ProgressState::default(),
            daily_logs,
            daily_top_rows: daily_state.last_top.clone(),
            daily_summary: None,
            pool_segments,
            pool_order,
            score_map,
            segment_scores: segment_scores.clone(),
            classify_detail: if segment_regions.classified.is_empty() {
                String::new()
            } else {
                format!("上次划分 {}", segment_regions.classified)
            },
            segment_regions,
            region_countries,
            region_extra,
            classify_progress: ProgressState::default(),
            scan_region: None,
            pool_cursor,
            probe_samples,
            probe_concurrency,
            probe_top,
            probe_seconds,
            probe_stage: if segment_scores.time.is_empty() {
                "未粗测".into()
            } else {
                "已有结果".into()
            },
            probe_detail: if segment_scores.time.is_empty() {
                "尚未做过本机段池粗测".into()
            } else {
                format!("上次粗测 {}", segment_scores.time)
            },
            probe_progress: ProgressState::default(),
            reset_armed_at: None,
            auto_daily_waiting: false,
            daily_time_blur: None,
        };

        storage::save_settings(&app.paths, &app.settings);
        storage::save_daily_state(&app.paths, &app.daily_state);

        let mut events_rx = events_rx;
        cx.spawn(async move |this, cx| {
            loop {
                let Some(first) = events_rx.recv().await else {
                    break;
                };
                let mut batch = vec![first];
                while batch.len() < 500 {
                    match events_rx.try_recv() {
                        Ok(event) => batch.push(event),
                        Err(_) => break,
                    }
                }
                let result = this.update(cx, |app, cx| {
                    for event in batch {
                        app.handle_event(event, cx);
                    }
                    cx.notify();
                });
                if result.is_err() {
                    break;
                }
            }
        })
        .detach();

        let sub_url = app.settings.sub_url.trim().to_string();
        if !sub_url.is_empty() {
            // 先用上次成功载入的节点，后台刷新失败也不会退回 TLS 基础验证。
            if let Some(nodes) = app.sub_cache.nodes_for(&sub_url).map(<[_]>::to_vec) {
                let note = format!("沿用 {} 载入的结果", app.sub_cache.time);
                app.apply_subscription_nodes(nodes, Some(&note));
            }
            app.start_subscription_load(sub_url);
        }

        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_secs(10))
                    .await;
                if this.update(cx, |app, cx| app.on_tick(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();

        app.resume_feed_server();

        #[cfg(debug_assertions)]
        app.apply_debug_overrides();

        app
    }

    /// 截图 / 调试用的环境变量覆盖（仅 debug 构建）。只要使用了任一覆盖，本次运行就不再写入设置，
    /// 避免临时状态（如收起侧栏、托盘常驻）被保存下来。
    ///
    /// `CFBEST_THEME=bee-light` · `CFBEST_PAGE=speed` · `CFBEST_SCALE=1.25` ·
    /// `CFBEST_SIDEBAR=collapsed` · `CFBEST_TRAY=1` · `CFBEST_DEMO=1`（内存中的演示数据）·
    /// `CFBEST_UPDATE_POPOVER=1`（打开检查更新弹层）。另见 update_panel.rs 的 `CFBEST_FAKE_VERSION`
    /// 与 `CFBEST_AUTO_UPDATE`：冒充旧版本、发现新版本后自动下载安装，用来测试更新流程。
    #[cfg(debug_assertions)]
    fn apply_debug_overrides(&mut self) {
        let var = |name: &str| std::env::var(name).ok();
        let mut used = false;
        if let Some(id) = var("CFBEST_THEME") {
            theme::set_theme(theme::Theme::from_id(&id));
            used = true;
        }
        if let Some(scale) = var("CFBEST_SCALE").and_then(|value| value.parse::<f32>().ok()) {
            theme::set_ui_scale(scale);
            used = true;
        }
        if var("CFBEST_TRAY").is_some() {
            self.settings.tray_enabled = true;
            used = true;
        }
        if var("CFBEST_SIDEBAR").is_some_and(|value| value == "collapsed") {
            self.settings.sidebar_collapsed = true;
            used = true;
        }
        if let Some(name) = var("CFBEST_PAGE") {
            self.page = match name.trim() {
                "speed" => Page::Speed,
                "verify" => Page::Verify,
                "daily" => Page::Daily,
                "pool" => Page::Pool,
                "history" => Page::History,
                _ => Page::Scan,
            };
            used = true;
        }
        if var("CFBEST_DEMO").is_some() {
            self.load_demo_data();
            used = true;
        }
        if var("CFBEST_UPDATE_POPOVER").is_some() {
            self.update_popover_open = true;
            used = true;
        }
        if used {
            self.settings_frozen = true;
            storage::log_line("调试覆盖生效：本次运行不写入设置");
        }
    }

    /// 保存设置；调试覆盖生效时跳过（见 `apply_debug_overrides`）。
    fn save_settings(&self) {
        if !self.settings_frozen {
            storage::save_settings(&self.paths, &self.settings);
        }
    }

    /// 生成可重复的演示数据：扫描命中、测速结果与两次测速历史。
    #[cfg(debug_assertions)]
    fn load_demo_data(&mut self) {
        let colos = ["HKG", "LAX", "SJC", "NRT", "SIN", "FRA"];
        let mut seed = 7u64;
        let mut next = move || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((seed >> 33) % 1000) as f64 / 1000.0
        };
        let mut records = Vec::new();
        for index in 0..24 {
            let ip = format!("104.16.{}.{}", 10 + index / 8, 20 + index * 3);
            let colo = colos[index % colos.len()].to_string();
            let latency = 40.0 + next() * 220.0;
            let median = (180.0 - latency * 0.45 + next() * 40.0).max(4.0);
            self.scan_hits
                .push(ScanHit::new(ip.clone(), 443, colo.clone(), latency));
            records.push(SpeedRecord {
                ip,
                port: 443,
                colo,
                median,
                best: median * 1.12,
                min: median * 0.86,
                rounds: vec![median * 0.9, median, median * 1.1],
            });
        }
        records.sort_by(|a, b| b.median.total_cmp(&a.median));
        let mut previous = records.clone();
        for record in previous.iter_mut() {
            record.median *= 0.7 + next() * 0.6;
        }
        previous.sort_by(|a, b| b.median.total_cmp(&a.median));
        previous.truncate(18);
        self.history.speed = vec![
            crate::core::model::SpeedRun {
                time: "2026-10-27 09:30:00".into(),
                throttled: 0,
                results: records.clone(),
            },
            crate::core::model::SpeedRun {
                time: "2026-10-26 21:10:00".into(),
                throttled: 1,
                results: previous,
            },
        ];
        self.speed_records = records;
    }

    pub fn initial_focus(&self, _: &gpui::App) -> gpui::FocusHandle {
        // 启动页是自动优选，没有需要立即输入的框。
        self.root_focus.clone()
    }

    fn notify_status(&mut self, text: impl Into<String>, tone: Tone, cx: &mut Context<Self>) {
        self.status = StatusLine::new(text, tone);
        if self
            .status_history
            .front()
            .is_none_or(|last| last.text != self.status.text)
        {
            self.status_history.push_front(self.status.clone());
            self.status_history.truncate(STATUS_HISTORY_LIMIT);
        }
        cx.notify();
    }

    fn fail(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        let text = text.into();
        storage::log_line(&format!("操作失败：{text}"));
        self.notify_status(text, Tone::Red, cx);
    }

    fn busy(&self) -> bool {
        self.job.is_some() || self.aux_job.is_some()
    }

    /// 订阅还在载入，手头又没有可用的 WS 节点：这时开始验证只能退化为 TLS 基础验证，先等一等。
    /// 有上次成功载入的节点时不用等，后台刷新不影响任何任务。
    fn awaiting_nodes(&self) -> bool {
        self.sub_loading
            && !self
                .verify_nodes
                .iter()
                .any(|node| node.transport.eq_ignore_ascii_case("ws"))
    }

    fn on_tick(&mut self, cx: &mut Context<Self>) {
        if self
            .reset_armed_at
            .is_some_and(|at| at.elapsed() >= Duration::from_secs(5))
        {
            self.reset_armed_at = None;
            cx.notify();
        }
        if !self.status.faded && self.status.at.elapsed() >= self.status.fade_after() {
            self.status.faded = true;
            cx.notify();
        }
        self.maybe_auto_daily(cx);
        self.maybe_check_update(cx);
    }

    /// 每日自动优选：到点后当天还没运行过就开始；错过时间（睡眠、关机）会在之后补跑，
    /// 有任务在运行时等它结束。后台启动，不切换当前页面。
    fn maybe_auto_daily(&mut self, cx: &mut Context<Self>) {
        if !self.settings.auto_daily {
            self.auto_daily_waiting = false;
            return;
        }
        let now = Local::now();
        let today = now.format("%Y-%m-%d").to_string();
        let at = (self.settings.auto_hour, self.settings.auto_minute);
        let now_hm = (now.hour() as u8, now.minute() as u8);
        if !auto_daily_due(&today, now_hm, &self.settings.last_auto_date, at) {
            return;
        }
        if self.busy() || self.awaiting_nodes() {
            if !self.auto_daily_waiting {
                self.auto_daily_waiting = true;
                self.notify_status(
                    format!(
                        "已到每日优选时间 {:02}:{:02}，{}",
                        at.0,
                        at.1,
                        if self.busy() {
                            "当前任务结束后开始"
                        } else {
                            "订阅节点载入后开始"
                        }
                    ),
                    Tone::Muted,
                    cx,
                );
            }
            return;
        }
        self.auto_daily_waiting = false;
        self.settings.last_auto_date = today;
        self.save_settings();
        let note = if now_hm > at {
            format!("补跑 {:02}:{:02} 的每日优选", at.0, at.1)
        } else {
            "按计划开始每日优选".to_string()
        };
        storage::log_line(&note);
        match self.launch_daily(false, cx) {
            Ok(()) => {
                self.daily_logs
                    .push_front(format!("{} {note}", now.format("%H:%M")));
                self.notify_status(note, Tone::Accent, cx);
            }
            Err(err) => self.fail(format!("自动优选启动失败：{err}"), cx),
        }
    }

    /// 设定每日自动运行时间。今天的时间已过时从明天开始，避免一设置就立即运行；返回是否顺延到明天。
    pub(super) fn schedule_auto_daily(&mut self, hour: u8, minute: u8) -> bool {
        self.settings.auto_hour = hour;
        self.settings.auto_minute = minute;
        let now = Local::now();
        let today = now.format("%Y-%m-%d").to_string();
        let deferred = auto_daily_due(
            &today,
            (now.hour() as u8, now.minute() as u8),
            &self.settings.last_auto_date,
            (hour, minute),
        );
        if deferred {
            self.settings.last_auto_date = today;
        }
        self.save_settings();
        deferred
    }

    /// 自动运行已开启时，时间输入框失焦即生效；输入无效则恢复为当前计划时间。
    fn commit_auto_time(&mut self, cx: &mut Context<Self>) {
        if !self.settings.auto_daily {
            return;
        }
        let current = (self.settings.auto_hour, self.settings.auto_minute);
        let parsed = parse_hhmm(self.daily_auto_time.read(cx).content());
        let (hour, minute) = parsed.clone().unwrap_or(current);
        self.daily_auto_time.update(cx, |input, cx| {
            input.set_content(format!("{hour:02}:{minute:02}"));
            cx.notify();
        });
        match parsed {
            Err(err) => self.fail(
                format!("{err}，已恢复为 {:02}:{:02}", current.0, current.1),
                cx,
            ),
            Ok(time) if time == current => {}
            Ok((hour, minute)) => {
                let deferred = self.schedule_auto_daily(hour, minute);
                self.notify_status(
                    if deferred {
                        format!("每日自动运行时间已改为 {hour:02}:{minute:02}；今天已过该时间，从明天开始")
                    } else {
                        format!("每日自动运行时间已改为 {hour:02}:{minute:02}")
                    },
                    Tone::Green,
                    cx,
                );
            }
        }
    }

    fn render_content(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        match self.page {
            Page::Scan => self.render_scan_page(cx),
            Page::Speed => self.render_speed_page(cx),
            Page::Verify => self.render_verify_page(cx),
            Page::Daily => self.render_daily_page(cx),
            Page::Pool => self.render_pool_page(cx),
            Page::History => self.render_history_page(cx),
        }
    }

    fn toggle_status_history(&mut self, cx: &mut Context<Self>) {
        let just_closed = self
            .status_history_closed_at
            .is_some_and(|at| at.elapsed() < Duration::from_millis(250));
        self.status_history_open = !self.status_history_open && !just_closed;
        cx.notify();
    }

    /// 状态栏上方的消息记录弹层（新的在前，最多 30 条）。
    fn render_status_history(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let rows = self.status_history.iter().map(|line| {
            let color = match line.tone {
                Tone::Red => theme::red(),
                Tone::Amber => theme::amber(),
                _ => theme::muted(),
            };
            div()
                .flex()
                .items_start()
                .gap_2()
                .px_3()
                .py_1p5()
                .rounded(px(theme::radius::SM))
                .hover(|style| style.bg(theme::row_hover()))
                .child(
                    div()
                        .flex_shrink_0()
                        .w(px(58.))
                        .font_family(theme::FONT_MONO)
                        .text_size(px(theme::font::MICRO))
                        .line_height(px(18.))
                        .text_color(theme::faint())
                        .child(line.time.clone()),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .mt(px(6.))
                        .child(widgets::badge_dot(line.tone)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(theme::font::SMALL))
                        .line_height(px(18.))
                        .text_color(color)
                        .child(line.text.clone()),
                )
        });
        let count = self.status_history.len();
        let panel = div()
            .id("status-history")
            .w(px(460.))
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .rounded(px(theme::radius::XL))
            .bg(theme::panel())
            .border_1()
            .border_color(theme::border())
            .shadow(theme::shadow::popover())
            .occlude()
            .on_mouse_down_out(cx.listener(|this, _: &gpui::MouseDownEvent, _, cx| {
                this.status_history_open = false;
                this.status_history_closed_at = Some(Instant::now());
                cx.notify();
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_2()
                    .pb_1()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_size(px(theme::font::LABEL))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(theme::text())
                            .child("最近消息")
                            .child(
                                div()
                                    .font_family(theme::FONT_MONO)
                                    .text_size(px(theme::font::MICRO))
                                    .text_color(theme::faint())
                                    .child(format!("{count} / {STATUS_HISTORY_LIMIT}")),
                            ),
                    )
                    .child(
                        div()
                            .id("clear-status-history")
                            .px_2()
                            .py_0p5()
                            .rounded(px(theme::radius::SM))
                            .cursor_pointer()
                            .text_size(px(theme::font::CAPTION))
                            .text_color(theme::faint())
                            .hover(|style| {
                                style.bg(theme::fill_secondary()).text_color(theme::text())
                            })
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.status_history.clear();
                                cx.notify();
                            }))
                            .child("清空"),
                    ),
            )
            .child(
                div()
                    .id("status-history-list")
                    .flex()
                    .flex_col()
                    .max_h(px(320.))
                    .overflow_y_scroll()
                    .when(count == 0, |this| {
                        this.child(
                            div()
                                .px_3()
                                .py_4()
                                .text_size(px(theme::font::SMALL))
                                .text_color(theme::faint())
                                .child("暂无消息"),
                        )
                    })
                    .children(rows),
            );
        div()
            .absolute()
            .top_0()
            .left_0()
            .child(
                gpui::deferred(
                    gpui::anchored()
                        .anchor(gpui::Corner::BottomLeft)
                        .offset(gpui::point(px(-8.), px(-6.)))
                        .snap_to_window_with_margin(px(8.))
                        .child(panel),
                )
                .with_priority(1),
            )
            .into_any_element()
    }

    fn render_status(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        let status = self.status.clone();
        let history_popover = self
            .status_history_open
            .then(|| self.render_status_history(cx));
        let status_color = match status.tone {
            _ if status.faded => theme::faint(),
            Tone::Red => theme::red(),
            Tone::Amber => theme::amber(),
            _ => theme::muted(),
        };
        div()
            .flex()
            .items_center()
            .justify_between()
            .h(px(30.))
            .px_5()
            .flex_shrink_0()
            .border_t_1()
            .border_color(theme::border_subtle())
            .child(
                div()
                    .id("status-line")
                    .relative()
                    .flex()
                    .items_center()
                    .gap_2()
                    .min_w_0()
                    .h(px(22.))
                    .px_1p5()
                    .ml(px(-6.))
                    .rounded(px(theme::radius::SM))
                    .cursor_pointer()
                    .hover(|style| style.bg(theme::row_hover()))
                    .tooltip(crate::ui::tooltip::tooltip("查看最近消息", None))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.toggle_status_history(cx);
                    }))
                    .child(widgets::badge_dot(if status.faded {
                        Tone::Muted
                    } else {
                        status.tone
                    }))
                    .child(
                        div()
                            .text_size(px(theme::font::SMALL))
                            .text_color(status_color)
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(status.text),
                    )
                    .children(history_popover),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .text_size(px(theme::font::CAPTION))
                            .text_color(theme::faint())
                            .child(widgets::kbd(theme::shortcut("Enter")))
                            .child("开始")
                            .child(widgets::kbd(theme::shortcut(".")))
                            .child("停止"),
                    )
                    .child(div().w(px(1.)).h(px(12.)).bg(theme::border_subtle()))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .child(div().size(px(6.)).rounded(px(3.)).bg(theme::accent()))
                            .child(
                                div()
                                    .text_size(px(theme::font::CAPTION))
                                    .text_color(theme::faint())
                                    .child(format!(
                                        "{} · {}",
                                        theme::current().style().label(),
                                        theme::current().mode_label()
                                    )),
                            ),
                    )
                    .child(div().w(px(1.)).h(px(12.)).bg(theme::border_subtle()))
                    .child(self.render_version_label(cx)),
            )
    }
}

impl Render for AppState {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.daily_time_blur.is_none() {
            let handle = gpui::Focusable::focus_handle(self.daily_auto_time.read(cx), cx);
            self.daily_time_blur =
                Some(cx.on_blur(&handle, window, |this, _, cx| this.commit_auto_time(cx)));
        }
        let sidebar = self.render_sidebar(cx);
        let topbar = self.render_topbar(window.is_maximized(), cx);
        let content = self.render_content(cx);
        let status = self.render_status(cx);
        let toast = self.render_toast(cx);
        let taskbar_progress =
            crate::taskbar::TaskbarProgress::from_fraction(self.job_progress(), self.job.is_some());
        crate::taskbar::update(&mut self.taskbar, window, taskbar_progress);
        let tray_tip = self.job_summary();
        if let Some(tray) = self.tray.as_mut() {
            tray.set_tip(&format!("CF Best · {tray_tip}"));
        }
        div()
            .relative()
            .flex()
            .size_full()
            .bg(theme::canvas_background())
            .text_color(theme::text())
            .font_family(theme::FONT_UI)
            .text_size(px(theme::font::TITLE))
            .track_focus(&self.root_focus)
            .key_context("CfBestRoot")
            .on_action(cx.listener(Self::on_switch_scan))
            .on_action(cx.listener(Self::on_switch_speed))
            .on_action(cx.listener(Self::on_switch_verify))
            .on_action(cx.listener(Self::on_switch_daily))
            .on_action(cx.listener(Self::on_switch_pool))
            .on_action(cx.listener(Self::on_switch_history))
            .on_action(cx.listener(Self::on_start_current))
            .on_action(cx.listener(Self::on_stop_current))
            .on_action(cx.listener(Self::on_toggle_advanced))
            .on_action(cx.listener(Self::on_toggle_sidebar))
            .on_action(cx.listener(Self::on_focus_next))
            .on_action(cx.listener(Self::on_focus_prev))
            .on_action(cx.listener(|this, _: &ZoomIn, _, cx| this.step_ui_scale(1, cx)))
            .on_action(cx.listener(|this, _: &ZoomOut, _, cx| this.step_ui_scale(-1, cx)))
            .on_action(cx.listener(|this, _: &ZoomReset, _, cx| this.set_ui_scale(1.0, cx)))
            .on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _, cx| {
                this.on_column_drag_move(event, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &gpui::MouseUpEvent, _, cx| this.end_column_drag(cx)),
            )
            // 任意鼠标点击结束键盘导航，焦点外框随之隐藏（`:focus-visible` 语义）。
            .on_any_mouse_down(cx.listener(|_, _: &gpui::MouseDownEvent, _, cx| {
                if widgets::set_keyboard_nav(false) {
                    cx.notify();
                }
            }))
            .child(sidebar)
            // Qoder WorkbenchFrame 的内容区：内缩 4px、1px border-tertiary、16px 圆角、bg-layout，
            // 左侧 `-4px 0 12px -8px` 阴影。标题栏、页面与状态栏都在面板内。
            // Windows 的窗口圆角只有 8px，右侧两角改用与窗口相同的圆角，左侧挨着侧栏仍为 16px。
            .child(
                div()
                    .id("workbench")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .m(gpui::px(WORKBENCH_INSET))
                    .rounded_l(px(theme::radius::XXL))
                    .rounded_r(gpui::px(WORKBENCH_OUTER_RADIUS))
                    .border_1()
                    .border_color(theme::border())
                    .bg(theme::content())
                    .shadow(theme::shadow::workbench())
                    .overflow_hidden()
                    .child(topbar)
                    .child(
                        div()
                            .id("main-page-scroll")
                            .flex_1()
                            .w_full()
                            .overflow_y_scroll()
                            .track_scroll(&self.main_scroll)
                            .px_5()
                            .py_4()
                            .child(content),
                    )
                    .child(status),
            )
            .children(toast)
    }
}

impl AppState {
    fn focus_page_input(&mut self, page: Page, window: &mut Window, cx: &App) {
        let handle = match page {
            Page::Scan => Some(self.scan_input.read(cx).focus_handle()),
            Page::Speed => Some(self.speed_input.read(cx).focus_handle()),
            Page::Verify if self.settings.advanced_mode => {
                Some(self.verify_input.read(cx).focus_handle())
            }
            Page::Verify | Page::Daily | Page::Pool | Page::History => None,
        };
        window.focus(&handle.unwrap_or_else(|| self.root_focus.clone()));
    }

    fn reset_page_scroll(&self) {
        self.main_scroll.set_offset(gpui::point(px(0.), px(0.)));
    }

    fn switch_page(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
        if self.page != page {
            self.nav_anim_seq += 1;
            // 离开页面时输入框随之卸载，不一定触发失焦，这里补一次提交。
            if self.page == Page::Daily {
                self.commit_auto_time(cx);
            }
        }
        self.page = page;
        if matches!(page, Page::Verify | Page::Daily) {
            // 用户可能刚装好代理软件；只读注册表 / 应用目录，开销可忽略。
            self.proxy_clients = crate::proxy_clients::detect();
        }
        self.reset_page_scroll();
        self.focus_page_input(page, window, cx);
        cx.notify();
    }

    fn on_switch_scan(&mut self, _: &SwitchScan, window: &mut Window, cx: &mut Context<Self>) {
        self.switch_page(Page::Scan, window, cx);
    }

    fn on_switch_speed(&mut self, _: &SwitchSpeed, window: &mut Window, cx: &mut Context<Self>) {
        self.switch_page(Page::Speed, window, cx);
    }

    fn on_switch_verify(&mut self, _: &SwitchVerify, window: &mut Window, cx: &mut Context<Self>) {
        self.switch_page(Page::Verify, window, cx);
    }

    fn on_switch_daily(&mut self, _: &SwitchDaily, window: &mut Window, cx: &mut Context<Self>) {
        self.switch_page(Page::Daily, window, cx);
    }

    fn on_switch_pool(&mut self, _: &SwitchPool, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.advanced_mode {
            self.switch_page(Page::Pool, window, cx);
        } else {
            self.notify_status("段池优化仅在高级模式中提供", Tone::Muted, cx);
        }
    }

    fn on_switch_history(
        &mut self,
        _: &SwitchHistory,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.settings.advanced_mode {
            self.switch_page(Page::History, window, cx);
        } else {
            self.notify_status("历史记录仅在高级模式中提供", Tone::Muted, cx);
        }
    }

    fn on_start_current(&mut self, _: &StartCurrent, _: &mut Window, cx: &mut Context<Self>) {
        match self.page {
            Page::Scan => self.start_scan_inner(cx),
            Page::Speed => self.start_speed_inner(cx),
            Page::Verify => self.start_verify_inner(cx),
            Page::Daily => {
                if let Err(err) = self.start_daily_inner(cx) {
                    self.fail(err, cx);
                }
            }
            Page::Pool => {
                if let Err(err) = self.start_probe_inner(cx) {
                    self.fail(err, cx);
                }
            }
            Page::History => {}
        }
    }

    fn on_stop_current(&mut self, _: &StopCurrent, _: &mut Window, cx: &mut Context<Self>) {
        self.stop_job_inner(cx);
    }

    fn on_toggle_advanced(
        &mut self,
        _: &ToggleAdvanced,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_advanced(&ClickEvent::default(), window, cx);
    }

    fn on_toggle_sidebar(&mut self, _: &ToggleSidebar, _: &mut Window, cx: &mut Context<Self>) {
        self.toggle_sidebar(cx);
    }

    fn on_focus_next(&mut self, _: &FocusNext, window: &mut Window, cx: &mut Context<Self>) {
        widgets::set_keyboard_nav(true);
        window.focus_next();
        cx.notify();
    }

    fn on_focus_prev(&mut self, _: &FocusPrev, window: &mut Window, cx: &mut Context<Self>) {
        widgets::set_keyboard_nav(true);
        window.focus_prev();
        cx.notify();
    }
}

fn parse_range<T>(text: &str, name: &str, min: T, max: T) -> Result<T, String>
where
    T: std::str::FromStr + PartialOrd + std::fmt::Display + Copy,
{
    let value = text
        .trim()
        .parse::<T>()
        .map_err(|_| format!("{name}必须是数字"))?;
    if value < min || value > max {
        return Err(format!("{name}需在 {min}–{max} 之间"));
    }
    Ok(value)
}

/// 到点（含错过后补跑）且今天还没自动运行过。
fn parse_hhmm(text: &str) -> Result<(u8, u8), String> {
    let (hour, minute) = text
        .trim()
        .split_once(':')
        .ok_or_else(|| "时间格式应为 HH:MM".to_string())?;
    let hour = hour.parse::<u8>().map_err(|_| "小时无效".to_string())?;
    let minute = minute.parse::<u8>().map_err(|_| "分钟无效".to_string())?;
    if hour > 23 || minute > 59 {
        return Err("时间超出范围（HH 00–23，MM 00–59）".into());
    }
    Ok((hour, minute))
}

#[cfg(test)]
mod tests {
    use super::{parse_hhmm, parse_range};

    #[test]
    fn parse_range_accepts_bounds_and_trims() {
        assert_eq!(parse_range::<u32>(" 5 ", "并发", 1, 10), Ok(5));
        assert_eq!(parse_range::<u32>("1", "并发", 1, 10), Ok(1));
        assert_eq!(parse_range::<u32>("10", "并发", 1, 10), Ok(10));
        assert_eq!(parse_range::<f64>("0.5", "比例", 0.0, 1.0), Ok(0.5));
    }

    #[test]
    fn parse_range_rejects_bad_input() {
        assert!(parse_range::<u32>("abc", "并发", 1, 10).is_err());
        assert!(parse_range::<u32>("0", "并发", 1, 10).is_err());
        assert!(parse_range::<u32>("11", "并发", 1, 10).is_err());
        assert!(parse_range::<u32>("", "并发", 1, 10).is_err());
    }

    #[test]
    fn parse_hhmm_handles_valid_and_invalid() {
        assert_eq!(parse_hhmm("06:30"), Ok((6, 30)));
        assert_eq!(parse_hhmm(" 23:59 "), Ok((23, 59)));
        assert_eq!(parse_hhmm("0:0"), Ok((0, 0)));
        assert!(parse_hhmm("24:00").is_err());
        assert!(parse_hhmm("12:60").is_err());
        assert!(parse_hhmm("1230").is_err());
        assert!(parse_hhmm("ab:cd").is_err());
    }
}
