use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScanHit {
    pub ip: String,
    pub port: u16,
    pub colo: String,
    pub country: String,
    pub latency_ms: f64,
}

impl ScanHit {
    pub fn new(ip: String, port: u16, colo: String, latency_ms: f64) -> Self {
        let country = crate::core::ip::country_for_colo(&colo).to_string();
        Self {
            ip,
            port,
            colo,
            country,
            latency_ms,
        }
    }

    pub fn passed_line(&self) -> String {
        format!(
            "{}:{}#{}\t{:.0}ms",
            self.ip, self.port, self.colo, self.latency_ms
        )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SpeedRecord {
    pub ip: String,
    pub port: u16,
    pub colo: String,
    pub median: f64,
    pub best: f64,
    pub min: f64,
    pub rounds: Vec<f64>,
}

impl SpeedRecord {
    pub fn is_stable(&self) -> bool {
        self.max() > 0.0 && self.min > self.max() * 0.7
    }

    pub fn max(&self) -> f64 {
        self.rounds.iter().copied().fold(0.0, f64::max)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VerifyRecord {
    pub ip: String,
    pub port: u16,
    pub transport: String,
    pub ok: bool,
    pub stage: String,
    pub detail: String,
    pub colo: Option<String>,
    pub ms: Option<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VlessNode {
    pub name: String,
    pub uuid: String,
    pub addr: String,
    pub port: u16,
    pub transport: String,
    pub path: String,
    pub host: String,
    pub sni: String,
    /// 分享链接里的全部查询参数（解码后，按原顺序）。生成优选节点时原样带上 fp、alpn 等字段。
    #[serde(default)]
    pub params: Vec<(String, String)>,
}

impl VlessNode {
    pub fn summary(&self) -> String {
        format!(
            "{} · {}:{} · {} · SNI {}",
            if self.name.is_empty() {
                "(未命名)"
            } else {
                &self.name
            },
            self.addr,
            self.port,
            if self.transport.is_empty() {
                "tcp"
            } else {
                &self.transport
            },
            self.sni
        )
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DailyVerification {
    #[default]
    Unknown,
    Tls,
    Tunnel,
}

impl DailyVerification {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unknown => "未记录验证类型",
            Self::Tls => "TLS 基础验证",
            Self::Tunnel => "隧道验证",
        }
    }

    /// 统计卡上通过数的标签。
    pub fn passed_label(self) -> &'static str {
        match self {
            Self::Unknown => "验证通过",
            Self::Tls => "TLS 基础验证通过",
            Self::Tunnel => "隧道验证通过",
        }
    }

    /// 旧版本保存的结果没有验证类型，界面上不为它们显示标签。
    pub fn known(self) -> Option<Self> {
        (self != Self::Unknown).then_some(self)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DailyEntry {
    pub ip: String,
    pub port: u16,
    pub colo: String,
    pub best_mbps: f64,
    pub avg_mbps: f64,
    pub latency_ms: f64,
    #[serde(default)]
    pub verification: DailyVerification,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DailySummary {
    pub segments: Vec<String>,
    pub targets: usize,
    pub scanned: usize,
    pub hits: usize,
    pub tested: usize,
    pub verified: usize,
    #[serde(default)]
    pub verification: DailyVerification,
    pub cursor: usize,
    pub round: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct DailyState {
    pub cursor: usize,
    pub round: u32,
    pub strategy: String,
    pub last_run: String,
    pub last_top: Vec<DailyEntry>,
    pub pool_count: usize,
    /// 锁定出口区域时按锁定区域单独轮转：`region_key` 为锁定内容，换了锁定就从头开始。
    pub region_key: String,
    pub region_cursor: usize,
    pub region_round: u32,
    /// 锁定区域内的段数。
    pub region_count: usize,
}

impl Default for DailyState {
    fn default() -> Self {
        Self {
            cursor: 0,
            round: 1,
            strategy: String::new(),
            last_run: String::new(),
            last_top: Vec::new(),
            pool_count: 0,
            region_key: String::new(),
            region_cursor: 0,
            region_round: 1,
            region_count: 0,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct History {
    pub daily: Vec<DailyRun>,
    pub verify: Vec<VerifyRun>,
    pub speed: Vec<SpeedRun>,
}

/// 最近一次手动扫描的命中，重启后恢复到「扫描 IP」页。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LastScan {
    pub time: String,
    pub total: usize,
    pub hits: Vec<ScanHit>,
}

/// 最近一次成功拉取的订阅节点。订阅地址时常连不上，拉取失败时沿用这份结果，
/// 隧道验证与「导入代理软件」不至于整轮退化为 TLS 基础验证。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SubscriptionCache {
    /// 拉取时的订阅地址；换了订阅就不再沿用。
    pub url: String,
    pub time: String,
    pub nodes: Vec<VlessNode>,
}

impl SubscriptionCache {
    /// `url` 对应的上次结果；地址不同或没有节点时为 None。
    pub fn nodes_for(&self, url: &str) -> Option<&[VlessNode]> {
        let url = url.trim();
        (!url.is_empty() && self.url.trim() == url && !self.nodes.is_empty())
            .then_some(self.nodes.as_slice())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DailyRun {
    pub time: String,
    pub top: Vec<DailyEntry>,
    pub scanned: usize,
    pub hits: usize,
    pub tested: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VerifyRun {
    pub time: String,
    pub total: usize,
    pub passed: usize,
    pub records: Vec<VerifyRecord>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SpeedRun {
    pub time: String,
    pub throttled: u32,
    pub results: Vec<SpeedRecord>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub scan_ports: String,
    pub scan_concurrency: u32,
    pub scan_timeout_secs: f64,
    pub scan_sample_pct: f64,
    pub speed_coarse_secs: u64,
    pub speed_fine_secs: u64,
    pub speed_fine_rounds: u32,
    pub speed_fine_topn: usize,
    pub sub_url: String,
    pub daily_segments: usize,
    pub daily_test: usize,
    pub daily_top: usize,
    pub daily_concurrency: u32,
    pub auto_daily: bool,
    pub auto_hour: u8,
    pub auto_minute: u8,
    pub last_auto_date: String,
    pub theme: String,
    /// 明暗模式跟随 Windows 系统设置（风格仍取自 `theme`）。
    pub follow_system_theme: bool,
    /// 侧栏收起为 64px 图标栏。
    pub sidebar_collapsed: bool,
    /// 界面缩放（0.9 / 1.0 / 1.1 / 1.25）。
    pub ui_scale: f32,
    /// 表格列宽（表名 → 可调整列的宽度）。
    pub column_widths: HashMap<String, Vec<f32>>,
    /// 托盘常驻：最小化 / 关闭时隐藏到系统托盘。
    pub tray_enabled: bool,
    /// 开机自启（写入当前用户的 Run 注册表项）。
    pub launch_at_login: bool,
    pub advanced_mode: bool,
    pub pool_strategy: String,
    pub probe_samples: usize,
    pub probe_concurrency: u32,
    pub probe_top: usize,
    pub probe_seconds: u64,
    /// 导入过代理软件后为 true：此后每次启动都开启本机订阅服务，客户端可以按时更新。
    pub proxy_feed_enabled: bool,
    /// 本机订阅服务的端口，0 表示尚未分配。客户端保存的订阅地址含端口，分配后尽量不变。
    pub proxy_feed_port: u16,
    /// 本机订阅地址中的随机令牌，其他程序与网页猜不到地址。
    pub proxy_feed_token: String,
    /// 启动时与每天自动检查一次新版本（只向 GitHub 请求最新版本信息，不上传任何数据）。
    pub update_check: bool,
    /// 锁定的出口区域：国家 / 地区代码（HK）或机房代码（SJC），为空表示不锁定。
    pub region_lock: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            scan_ports: "443".into(),
            scan_concurrency: 300,
            scan_timeout_secs: 5.0,
            scan_sample_pct: 0.0,
            speed_coarse_secs: 2,
            speed_fine_secs: 6,
            speed_fine_rounds: 3,
            speed_fine_topn: 16,
            sub_url: String::new(),
            daily_segments: 20,
            daily_test: 20,
            daily_top: 5,
            daily_concurrency: 300,
            auto_daily: false,
            auto_hour: 9,
            auto_minute: 0,
            last_auto_date: String::new(),
            theme: "light".into(),
            follow_system_theme: false,
            sidebar_collapsed: false,
            ui_scale: 1.0,
            column_widths: HashMap::new(),
            tray_enabled: false,
            launch_at_login: false,
            advanced_mode: false,
            pool_strategy: "balanced".into(),
            probe_samples: 2,
            probe_concurrency: 60,
            probe_top: 8,
            probe_seconds: 2,
            proxy_feed_enabled: false,
            proxy_feed_port: 0,
            proxy_feed_token: String::new(),
            update_check: true,
            region_lock: Vec::new(),
        }
    }
}

/// 扫描 / 测速 / 每日优选的运行参数。简单模式与高级模式共用同一份设置，默认值即推荐值。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamGroup {
    Scan,
    Speed,
    Daily,
}

impl Settings {
    pub fn params_recommended(&self, group: ParamGroup) -> bool {
        let mut reset = self.clone();
        reset.reset_params(group);
        match group {
            ParamGroup::Scan => {
                self.scan_ports == reset.scan_ports
                    && self.scan_concurrency == reset.scan_concurrency
                    && self.scan_timeout_secs == reset.scan_timeout_secs
                    && self.scan_sample_pct == reset.scan_sample_pct
            }
            ParamGroup::Speed => {
                self.speed_coarse_secs == reset.speed_coarse_secs
                    && self.speed_fine_secs == reset.speed_fine_secs
                    && self.speed_fine_rounds == reset.speed_fine_rounds
                    && self.speed_fine_topn == reset.speed_fine_topn
            }
            ParamGroup::Daily => {
                self.daily_segments == reset.daily_segments
                    && self.daily_test == reset.daily_test
                    && self.daily_top == reset.daily_top
                    && self.daily_concurrency == reset.daily_concurrency
            }
        }
    }

    pub fn reset_params(&mut self, group: ParamGroup) {
        let recommended = Settings::default();
        match group {
            ParamGroup::Scan => {
                self.scan_ports = recommended.scan_ports;
                self.scan_concurrency = recommended.scan_concurrency;
                self.scan_timeout_secs = recommended.scan_timeout_secs;
                self.scan_sample_pct = recommended.scan_sample_pct;
            }
            ParamGroup::Speed => {
                self.speed_coarse_secs = recommended.speed_coarse_secs;
                self.speed_fine_secs = recommended.speed_fine_secs;
                self.speed_fine_rounds = recommended.speed_fine_rounds;
                self.speed_fine_topn = recommended.speed_fine_topn;
            }
            ParamGroup::Daily => {
                self.daily_segments = recommended.daily_segments;
                self.daily_test = recommended.daily_test;
                self.daily_top = recommended.daily_top;
                self.daily_concurrency = recommended.daily_concurrency;
            }
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SegmentScore {
    pub segment: String,
    pub group: String,
    pub samples: usize,
    pub hits: usize,
    pub colo: String,
    pub best_latency_ms: f64,
    pub avg_latency_ms: f64,
    pub mbps: Option<f64>,
    pub score: f64,
}

/// 段池中各 /24 段在本机网络下落到的机房（出口区域划分），见 `core::region`。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SegmentRegions {
    /// 最近一次完整划分完成的时间。
    pub classified: String,
    pub records: Vec<SegmentRegion>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SegmentRegion {
    pub segment: String,
    /// 各机房的命中次数，多的在前；为空表示探测过但没有响应。
    pub colos: Vec<(String, u32)>,
    /// 完整探测过的地址数；0 表示只从零散的扫描结果中学到，还没有专门划分过。
    pub probed: u32,
    pub updated: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SegmentScores {
    pub time: String,
    pub records: Vec<SegmentScore>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daily_history_preserves_legacy_results_and_verification_types() {
        let legacy = r#"{
            "daily": [{
                "time": "2026-10-26 09:00:00",
                "scanned": 256, "hits": 12, "tested": 5,
                "top": [{
                    "ip": "192.0.2.1", "port": 443, "colo": "HKG",
                    "best_mbps": 80.0, "avg_mbps": 60.0, "latency_ms": 30.0
                }]
            }]
        }"#;
        let mut history: History = serde_json::from_str(legacy).unwrap();
        assert_eq!(history.daily.len(), 1);
        assert_eq!(history.daily[0].top.len(), 1);
        assert_eq!(
            history.daily[0].top[0].verification,
            DailyVerification::Unknown
        );
        assert_eq!(history.daily[0].top[0].verification.known(), None);

        for verification in [DailyVerification::Tls, DailyVerification::Tunnel] {
            history.daily[0].top[0].verification = verification;
            let json = serde_json::to_string(&history).unwrap();
            let restored: History = serde_json::from_str(&json).unwrap();
            let entry = &restored.daily[0].top[0];
            assert_eq!(entry.ip, "192.0.2.1");
            assert_eq!(entry.best_mbps, 80.0);
            assert_eq!(entry.verification, verification);
            assert_eq!(entry.verification.known(), Some(verification));
            assert!(entry.verification.passed_label().ends_with("通过"));
        }
    }

    #[test]
    fn subscription_cache_only_serves_the_same_url() {
        let node = VlessNode {
            name: "HK".into(),
            uuid: "b831381d-6324-4d53-ad4f-8cda48b30811".into(),
            addr: "edge.example.com".into(),
            port: 443,
            transport: "ws".into(),
            path: "/?ed=2048".into(),
            host: "edge.example.com".into(),
            sni: "edge.example.com".into(),
            params: vec![("fp".into(), "chrome".into())],
        };
        let cache = SubscriptionCache {
            url: "https://sub.example.com/u".into(),
            time: "2026-10-29 08:10".into(),
            nodes: vec![node],
        };
        assert_eq!(
            cache
                .nodes_for(" https://sub.example.com/u ")
                .map(<[_]>::len),
            Some(1)
        );
        assert!(cache.nodes_for("https://sub.example.com/other").is_none());
        assert!(cache.nodes_for("").is_none());
        assert!(SubscriptionCache::default().nodes_for("").is_none());
        let empty = SubscriptionCache {
            nodes: Vec::new(),
            ..cache.clone()
        };
        assert!(empty.nodes_for("https://sub.example.com/u").is_none());

        // 落盘再读回，分享链接参数也要保留，生成的节点才与原订阅一致。
        let restored: SubscriptionCache =
            serde_json::from_str(&serde_json::to_string(&cache).unwrap()).unwrap();
        let node = &restored.nodes_for("https://sub.example.com/u").unwrap()[0];
        assert_eq!(node.path, "/?ed=2048");
        assert_eq!(node.params, vec![("fp".to_string(), "chrome".to_string())]);
        // 旧版本写下的节点没有 params 字段。
        let legacy: VlessNode = serde_json::from_str(
            r#"{"name":"a","uuid":"u","addr":"h","port":443,"transport":"ws","path":"/","host":"h","sni":"h"}"#,
        )
        .unwrap();
        assert!(legacy.params.is_empty());
    }

    #[test]
    fn param_groups_reset_independently() {
        let mut settings = Settings::default();
        assert!(settings.params_recommended(ParamGroup::Scan));
        settings.scan_concurrency = 50;
        settings.daily_top = 9;
        assert!(!settings.params_recommended(ParamGroup::Scan));
        assert!(settings.params_recommended(ParamGroup::Speed));
        settings.reset_params(ParamGroup::Scan);
        assert!(settings.params_recommended(ParamGroup::Scan));
        assert!(!settings.params_recommended(ParamGroup::Daily));
    }
}
