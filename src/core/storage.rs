use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use chrono::Local;
use serde::{Serialize, de::DeserializeOwned};

use crate::core::model::{
    DailyEntry, DailyState, DailyVerification, History, LastScan, ScanHit, SegmentScores, Settings,
    SpeedRecord, SubscriptionCache, VerifyRecord,
};

static LOG_FILE: OnceLock<Mutex<File>> = OnceLock::new();
/// 日志是否同时写到控制台；无窗口模式用 `--quiet` 关掉。
static CONSOLE_LOG: AtomicBool = AtomicBool::new(true);

#[derive(Clone, Debug)]
pub struct AppPaths {
    pub root: PathBuf,
    pub exports: PathBuf,
    pub settings: PathBuf,
    pub history: PathBuf,
    pub daily_state: PathBuf,
    pub pool: PathBuf,
    pub segment_scores: PathBuf,
    pub log: PathBuf,
}

impl AppPaths {
    pub fn discover() -> Self {
        let root = platform_data_dir()
            .unwrap_or_else(|| {
                std::env::current_exe()
                    .ok()
                    .and_then(|exe| exe.parent().map(Path::to_path_buf))
                    .unwrap_or_else(|| PathBuf::from("."))
            })
            .join("CFBest");
        let exports = root.join("exports");
        let _ = fs::create_dir_all(&exports);
        Self {
            settings: root.join("settings.json"),
            history: root.join("history.json"),
            daily_state: root.join("daily_state.json"),
            pool: root.join("pool.txt"),
            segment_scores: root.join("segment_scores.json"),
            log: root.join("runtime.log"),
            root,
            exports,
        }
    }
}

/// 各平台存放应用数据的目录：Windows `%APPDATA%`，macOS `~/Library/Application Support`，
/// 其他平台 `$XDG_DATA_HOME` 或 `~/.local/share`。
fn platform_data_dir() -> Option<PathBuf> {
    let var = |name: &str| std::env::var_os(name).map(PathBuf::from);
    if cfg!(target_os = "windows") {
        var("APPDATA")
    } else if cfg!(target_os = "macos") {
        var("HOME").map(|home| home.join("Library").join("Application Support"))
    } else {
        var("XDG_DATA_HOME").or_else(|| var("HOME").map(|home| home.join(".local").join("share")))
    }
}

/// 单实例锁。另一个实例占着锁时最多等 `wait`：更新后由旧版本拉起的新版本要等旧进程退出。
pub fn acquire_instance_lock(paths: &AppPaths, wait: std::time::Duration) -> Option<File> {
    let path = paths.root.join("instance.lock");
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .ok()?;
    let deadline = std::time::Instant::now() + wait;
    loop {
        match file.try_lock() {
            Ok(()) => return Some(file),
            Err(_) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            Err(_) => return None,
        }
    }
}

pub fn init_logging(paths: &AppPaths) {
    let _ = fs::create_dir_all(&paths.root);
    if fs::metadata(&paths.log).is_ok_and(|meta| meta.len() > 2 * 1024 * 1024) {
        let _ = fs::rename(&paths.log, paths.log.with_extension("log.old"));
    }
    if let Ok(file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.log)
    {
        let _ = LOG_FILE.set(Mutex::new(file));
    }
    log_line("──────── 应用启动 ────────");
}

pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log_line(&format!("PANIC: {info}"));
        previous(info);
    }));
}

/// 是否把日志同时写到控制台。图形界面保持默认（写），无窗口模式可用 `--quiet` 关掉。
/// 只有 `cfbest-cli` 会调用，图形界面固定用默认值。
#[allow(dead_code)]
pub fn set_console_logging(enabled: bool) {
    CONSOLE_LOG.store(enabled, Ordering::Relaxed);
}

pub fn log_line(message: &str) {
    let stamp = Local::now().format("%Y-%m-%d %H:%M:%S");
    if let Some(file) = LOG_FILE.get()
        && let Ok(mut file) = file.lock()
    {
        let _ = writeln!(file, "[{stamp}] {message}");
        let _ = file.flush();
    }
    if CONSOLE_LOG.load(Ordering::Relaxed) {
        eprintln!("[{stamp}] {message}");
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension(format!(
        "{}.tmp",
        path.extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("data")
    ));
    fs::write(&tmp, bytes)?;
    if let Err(err) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(path);
        fs::rename(&tmp, path)?;
        log_line(&format!("rename fallback for {}: {err}", path.display()));
    }
    Ok(())
}

pub fn save_json<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    write_atomic(path, &bytes)
}

pub fn load_json<T: DeserializeOwned + Default>(path: &Path) -> T {
    match fs::read(path) {
        Ok(bytes) => match serde_json::from_slice(&bytes) {
            Ok(value) => value,
            Err(err) => {
                log_line(&format!("读取 {} 失败：{err}，使用默认值", path.display()));
                T::default()
            }
        },
        Err(_) => T::default(),
    }
}

pub fn load_settings(paths: &AppPaths) -> Settings {
    load_json(&paths.settings)
}

pub fn save_settings(paths: &AppPaths, settings: &Settings) {
    if let Err(err) = save_json(&paths.settings, settings) {
        log_line(&format!("保存设置失败：{err}"));
    }
}

pub fn load_history(paths: &AppPaths) -> History {
    load_json(&paths.history)
}

pub fn save_history(paths: &AppPaths, history: &History) {
    if let Err(err) = save_json(&paths.history, history) {
        log_line(&format!("保存历史失败：{err}"));
    }
}

fn last_scan_path(paths: &AppPaths) -> PathBuf {
    paths.root.join("last_scan.json")
}

pub fn load_last_scan(paths: &AppPaths) -> LastScan {
    load_json(&last_scan_path(paths))
}

pub fn save_last_scan(paths: &AppPaths, scan: &LastScan) {
    if let Err(err) = save_json(&last_scan_path(paths), scan) {
        log_line(&format!("保存扫描结果失败：{err}"));
    }
}

fn subscription_cache_path(paths: &AppPaths) -> PathBuf {
    paths.root.join("subscription.json")
}

pub fn load_subscription_cache(paths: &AppPaths) -> SubscriptionCache {
    load_json(&subscription_cache_path(paths))
}

pub fn save_subscription_cache(paths: &AppPaths, cache: &SubscriptionCache) {
    if let Err(err) = save_json(&subscription_cache_path(paths), cache) {
        log_line(&format!("保存订阅节点失败：{err}"));
    }
}

pub fn load_daily_state(paths: &AppPaths) -> DailyState {
    load_json(&paths.daily_state)
}

pub fn save_daily_state(paths: &AppPaths, state: &DailyState) {
    if let Err(err) = save_json(&paths.daily_state, state) {
        log_line(&format!("保存进度失败：{err}"));
    }
}

pub fn load_segment_scores(paths: &AppPaths) -> SegmentScores {
    load_json(&paths.segment_scores)
}

pub fn save_segment_scores(paths: &AppPaths, scores: &SegmentScores) {
    if let Err(err) = save_json(&paths.segment_scores, scores) {
        log_line(&format!("保存段池粗测结果失败：{err}"));
    }
}

pub fn load_pool(paths: &AppPaths) -> Vec<String> {
    fs::read_to_string(&paths.pool)
        .map(|text| {
            let mut seen = HashSet::new();
            text.lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .filter(|line| seen.insert(line.to_string()))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

pub fn save_pool(paths: &AppPaths, segments: &[String]) -> io::Result<()> {
    write_atomic(&paths.pool, format!("{}\n", segments.join("\n")).as_bytes())
}

fn timestamp() -> String {
    Local::now().format("%Y%m%d-%H%M%S").to_string()
}

fn csv_field(value: &str) -> String {
    if value.contains([',', '"', '\n']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

pub fn write_scan_exports(paths: &AppPaths, hits: &[ScanHit]) -> io::Result<Vec<PathBuf>> {
    let stamp = timestamp();
    let csv_path = paths.exports.join(format!("scan-{stamp}.csv"));
    let passed_path = paths.exports.join(format!("scan-{stamp}.passed.txt"));

    let mut csv = String::from("IPv4,IPv6,国家\n");
    for hit in hits {
        csv.push_str(&format!(
            "{},{},{}\n",
            csv_field(&hit.ip),
            csv_field(&hit.ip),
            csv_field(&hit.country)
        ));
    }
    write_atomic(&csv_path, csv.as_bytes())?;

    let mut passed = String::new();
    for hit in hits {
        passed.push_str(&hit.passed_line());
        passed.push('\n');
    }
    write_atomic(&passed_path, passed.as_bytes())?;
    Ok(vec![csv_path, passed_path])
}

#[derive(Serialize)]
struct SpeedExport<'a> {
    time: String,
    throttled: u32,
    rank: Vec<SpeedExportRow<'a>>,
}

#[derive(Serialize)]
struct SpeedExportRow<'a> {
    ip: &'a str,
    port: u16,
    colo: &'a str,
    med: f64,
    max: f64,
    min: f64,
    all: &'a [f64],
}

fn speed_csv(records: &[SpeedRecord]) -> String {
    let mut csv = String::from("排名,IP,端口,落点,中位Mbps,最快Mbps,最慢Mbps,稳定,各轮Mbps\n");
    for (index, record) in records.iter().enumerate() {
        let rounds = record
            .rounds
            .iter()
            .map(|value| format!("{value:.2}"))
            .collect::<Vec<_>>()
            .join(" ");
        csv.push_str(&format!(
            "{},{},{},{},{:.2},{:.2},{:.2},{},{}\n",
            index + 1,
            csv_field(&record.ip),
            record.port,
            csv_field(&record.colo),
            record.median,
            record.best,
            record.min,
            if record.is_stable() { "是" } else { "否" },
            csv_field(&rounds)
        ));
    }
    csv
}

fn verify_csv(records: &[VerifyRecord]) -> String {
    let mut csv = String::from("IP,端口,传输,结果,阶段,落点,耗时ms,详情\n");
    for record in records {
        csv.push_str(&format!(
            "{},{},{},{},{},{},{},{}\n",
            csv_field(&record.ip),
            record.port,
            csv_field(&record.transport),
            if record.ok { "通过" } else { "失败" },
            csv_field(&record.stage),
            csv_field(record.colo.as_deref().unwrap_or("")),
            record.ms.map(|ms| format!("{ms:.0}")).unwrap_or_default(),
            csv_field(&record.detail)
        ));
    }
    csv
}

/// Excel 需要 BOM 才能正确识别 UTF-8 中文。
fn with_bom(text: &str) -> Vec<u8> {
    let mut bytes = "\u{feff}".as_bytes().to_vec();
    bytes.extend_from_slice(text.as_bytes());
    bytes
}

/// 导出测速结果：JSON（兼容旧格式）+ CSV（便于表格软件打开）。
pub fn write_speed_export(
    paths: &AppPaths,
    records: &[SpeedRecord],
    throttled: u32,
) -> io::Result<Vec<PathBuf>> {
    let stamp = timestamp();
    let path = paths.exports.join(format!("speed-{stamp}.json"));
    let csv_path = paths.exports.join(format!("speed-{stamp}.csv"));
    let doc = SpeedExport {
        time: Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        throttled,
        rank: records
            .iter()
            .map(|record| SpeedExportRow {
                ip: &record.ip,
                port: record.port,
                colo: &record.colo,
                med: record.median,
                max: record.best,
                min: record.min,
                all: &record.rounds,
            })
            .collect(),
    };
    save_json(&path, &doc)?;
    write_atomic(&csv_path, &with_bom(&speed_csv(records)))?;
    Ok(vec![path, csv_path])
}

/// 导出验证结果：PICKS 列表 + 全部记录 CSV + 通过地址的 `ip:port#落点` 列表。
pub fn write_verify_picks(paths: &AppPaths, records: &[VerifyRecord]) -> io::Result<Vec<PathBuf>> {
    let stamp = timestamp();
    let path = paths.exports.join(format!("verify-picks-{stamp}.txt"));
    let csv_path = paths.exports.join(format!("verify-{stamp}.csv"));
    let list_path = paths.exports.join(format!("verify-ok-{stamp}.txt"));
    let mut doc = String::from("PICKS = [\n");
    for record in records.iter().filter(|record| record.ok) {
        let colo = record.colo.as_deref().unwrap_or("?");
        doc.push_str(&format!(
            "    (\"{}\", \"{} · {}\"),\n",
            record.ip,
            colo,
            crate::core::ip::colo_label(colo)
        ));
    }
    doc.push_str("]\n");
    write_atomic(&path, doc.as_bytes())?;
    write_atomic(&csv_path, &with_bom(&verify_csv(records)))?;
    let mut list = String::new();
    for record in records.iter().filter(|record| record.ok) {
        list.push_str(&format!(
            "{}:{}#{}\n",
            record.ip,
            record.port,
            record.colo.as_deref().unwrap_or("?")
        ));
    }
    write_atomic(&list_path, list.as_bytes())?;
    Ok(vec![path, csv_path, list_path])
}

pub fn write_daily_hits(paths: &AppPaths, hits: &[ScanHit]) -> io::Result<PathBuf> {
    let path = paths
        .root
        .join(format!("扫描命中-{}.txt", Local::now().format("%Y%m%d")));
    let mut doc = String::new();
    for hit in hits {
        doc.push_str(&hit.passed_line());
        doc.push('\n');
    }
    write_atomic(&path, doc.as_bytes())?;
    Ok(path)
}

pub fn write_daily_output(paths: &AppPaths, top: &[DailyEntry]) -> io::Result<PathBuf> {
    let path = paths
        .root
        .join(format!("每日优选-{}.txt", Local::now().format("%Y%m%d")));
    let mut doc = format!(
        "# {}  Top {}\n",
        Local::now().format("%Y-%m-%d %H:%M"),
        top.len()
    );
    if top
        .iter()
        .any(|entry| entry.verification == DailyVerification::Tls)
    {
        doc.push_str("# TLS 基础验证仅检查 TLS 连接与 HTTP 回源，不代表代理隧道可用。\n");
    }
    for entry in top {
        doc.push_str(&format!(
            "{}:{}#{}\t{:.1}Mbps\t{:.0}ms\t{}\n",
            entry.ip,
            entry.port,
            entry.colo,
            entry.best_mbps,
            entry.latency_ms,
            entry.verification.label()
        ));
    }
    write_atomic(&path, doc.as_bytes())?;
    Ok(path)
}

pub fn open_folder(path: &Path) {
    let opener = if cfg!(target_os = "windows") {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(opener).arg(path).spawn();
}

pub fn trim_history(history: &mut History) {
    history.daily.truncate(60);
    history.verify.truncate(60);
    history.speed.truncate(60);
    for run in &mut history.speed {
        run.results.truncate(50);
    }
    for run in &mut history.verify {
        run.records.truncate(80);
    }
}

#[cfg(test)]
mod tests {
    use super::{csv_field, speed_csv, verify_csv};
    use crate::core::model::{SpeedRecord, VerifyRecord};

    #[test]
    fn csv_field_quotes_only_when_needed() {
        assert_eq!(csv_field("1.1.1.1"), "1.1.1.1");
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(csv_field("line\nbreak"), "\"line\nbreak\"");
    }

    #[test]
    fn speed_csv_has_header_and_ranked_rows() {
        let records = vec![SpeedRecord {
            ip: "1.1.1.1".into(),
            port: 443,
            colo: "HKG".into(),
            median: 50.0,
            best: 55.0,
            min: 45.0,
            rounds: vec![45.0, 50.0, 55.0],
        }];
        let csv = speed_csv(&records);
        let mut lines = csv.lines();
        assert!(lines.next().unwrap().starts_with("排名,IP,"));
        assert_eq!(
            lines.next().unwrap(),
            "1,1.1.1.1,443,HKG,50.00,55.00,45.00,是,45.00 50.00 55.00"
        );
    }

    #[test]
    fn verify_csv_quotes_detail_with_commas() {
        let records = vec![VerifyRecord {
            ip: "1.0.0.1".into(),
            port: 443,
            transport: "ws".into(),
            ok: false,
            stage: "tls".into(),
            detail: "timeout, retry".into(),
            colo: None,
            ms: None,
        }];
        let csv = verify_csv(&records);
        assert_eq!(
            csv.lines().nth(1).unwrap(),
            "1.0.0.1,443,ws,失败,tls,,,\"timeout, retry\""
        );
    }
}
