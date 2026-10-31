use std::future::Future;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use tokio::runtime::Runtime;
use tokio::sync::mpsc::UnboundedSender;

use crate::core::daily::{self, DailyConfig, DailyResult};
use crate::core::http;
use crate::core::ip;
use crate::core::model::{ScanHit, SpeedRecord, VerifyRecord, VlessNode};
use crate::core::probe::{self, ProbeConfig, ProbeOutcome};
use crate::core::region::{self, ClassifyConfig};
use crate::core::scan::{self, ScanConfig, ScanOutcome};
use crate::core::speed::{self, SpeedConfig, SpeedOutcome};
use crate::core::storage::{self, AppPaths};
use crate::core::update;
use crate::core::verify::{self, VerifyOutcome, VerifyTarget};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobKind {
    Scan,
    Speed,
    Verify,
    Daily,
    Asn,
    Pool,
    PoolRebuild,
    Probe,
    Classify,
}

impl JobKind {
    pub fn label(self) -> &'static str {
        match self {
            JobKind::Scan => "扫描",
            JobKind::Speed => "测速",
            JobKind::Verify => "可用性验证",
            JobKind::Daily => "每日优选",
            JobKind::Asn => "ASN 查询",
            JobKind::Pool => "官方候选池",
            JobKind::PoolRebuild => "段池重建",
            JobKind::Probe => "段池粗测",
            JobKind::Classify => "出口区域划分",
        }
    }
}

#[derive(Debug)]
pub enum JobEvent {
    ScanProgress {
        done: usize,
        total: usize,
        hits: usize,
        elapsed_ms: u64,
    },
    ScanHit(ScanHit),
    ScanDone(ScanOutcome),
    SpeedPhase {
        name: &'static str,
        total: usize,
    },
    SpeedProgress {
        phase: &'static str,
        done: usize,
        total: usize,
        throttled: u32,
        current: Option<String>,
    },
    SpeedRecord(SpeedRecord),
    SpeedWarning(String),
    SpeedDone(SpeedOutcome),
    VerifyProgress {
        done: usize,
        total: usize,
        current: String,
    },
    VerifyRecord(VerifyRecord),
    VerifyDone(VerifyOutcome),
    DailyStage {
        stage: String,
        detail: String,
    },
    DailyProgress {
        stage: String,
        done: usize,
        total: usize,
        label: String,
    },
    DailyLog(String),
    DailyDone(DailyResult),
    AsnReady {
        prefixes: Vec<String>,
        source: String,
    },
    PoolReady {
        ips: Vec<String>,
        source: String,
    },
    SubscriptionReady {
        url: String,
        nodes: Vec<VlessNode>,
    },
    /// 订阅拉取（含重试）最终失败。
    SubscriptionFailed {
        url: String,
        error: String,
    },
    PoolRebuilt {
        count: usize,
        source: String,
    },
    ProbePhase {
        name: &'static str,
        total: usize,
    },
    ProbeProgress {
        phase: &'static str,
        done: usize,
        total: usize,
        current: String,
    },
    ProbeWarning(String),
    ProbeDone(ProbeOutcome),
    ClassifyProgress(region::ClassifyProgress),
    ClassifyDone(region::ClassifyOutcome),
    UpdateChecked(Result<update::Release, String>),
    UpdateProgress(update::DownloadProgress),
    /// 已下载并校验的安装包内容，或失败原因。
    UpdateDownloaded(Result<Vec<u8>, String>),
    Error(String),
    Done(JobKind),
    AuxDone,
}

pub fn runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        let workers = std::thread::available_parallelism()
            .map(|count| count.get().min(8))
            .unwrap_or(4);
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(workers)
            .thread_name("cf-best-net")
            .enable_all()
            .build()
            .expect("创建 Tokio 运行时失败")
    })
}

fn spawn_guarded<Fut>(kind: JobKind, tx: UnboundedSender<JobEvent>, future: Fut)
where
    Fut: Future<Output = ()> + Send + 'static,
{
    runtime().spawn(async move {
        let supervisor = tx.clone();
        let handle = tokio::spawn(future);
        if let Err(err) = handle.await {
            let message = format!("{}任务异常：{err}", kind.label());
            storage::log_line(&message);
            let _ = supervisor.send(JobEvent::Error(message));
        }
        let _ = supervisor.send(JobEvent::Done(kind));
    });
}

fn spawn_aux<Fut>(kind: JobKind, tx: UnboundedSender<JobEvent>, future: Fut)
where
    Fut: Future<Output = ()> + Send + 'static,
{
    runtime().spawn(async move {
        let supervisor = tx.clone();
        let handle = tokio::spawn(future);
        if let Err(err) = handle.await {
            let message = format!("{}任务异常：{err}", kind.label());
            storage::log_line(&message);
            let _ = supervisor.send(JobEvent::Error(message));
        }
        let _ = supervisor.send(JobEvent::AuxDone);
    });
}

pub fn spawn_scan(config: ScanConfig, tx: UnboundedSender<JobEvent>, cancel: Arc<AtomicBool>) {
    storage::log_line(&format!(
        "开始扫描：{} 个探测，并发 {}",
        config.targets.len(),
        config.concurrency
    ));
    spawn_guarded(JobKind::Scan, tx.clone(), async move {
        let tx = tx;
        let outcome = scan::execute(&config, cancel, |event| match event {
            scan::ScanEvent::Progress {
                done,
                total,
                hits,
                elapsed,
            } => {
                let _ = tx.send(JobEvent::ScanProgress {
                    done,
                    total,
                    hits,
                    elapsed_ms: elapsed.as_millis() as u64,
                });
            }
            scan::ScanEvent::Hit(hit) => {
                let _ = tx.send(JobEvent::ScanHit(hit));
            }
        })
        .await;
        storage::log_line(&format!(
            "扫描结束：命中 {}/{}，失败 {}，耗时 {:.1}s{}",
            outcome.hits.len(),
            outcome.total,
            outcome.failed,
            outcome.elapsed.as_secs_f64(),
            if outcome.stopped {
                "（已停止）"
            } else {
                ""
            }
        ));
        let _ = tx.send(JobEvent::ScanDone(outcome));
    });
}

pub fn spawn_speed(config: SpeedConfig, tx: UnboundedSender<JobEvent>, cancel: Arc<AtomicBool>) {
    storage::log_line(&format!(
        "开始测速：{} 个候选（串行）",
        config.targets.len()
    ));
    spawn_guarded(JobKind::Speed, tx.clone(), async move {
        let tx = tx;
        let outcome = speed::execute(&config, cancel, |event| match event {
            speed::SpeedEvent::Phase { name, total } => {
                let _ = tx.send(JobEvent::SpeedPhase { name, total });
            }
            speed::SpeedEvent::Progress {
                phase,
                done,
                total,
                throttled,
                current,
            } => {
                let _ = tx.send(JobEvent::SpeedProgress {
                    phase,
                    done,
                    total,
                    throttled,
                    current,
                });
            }
            speed::SpeedEvent::Record(record) => {
                let _ = tx.send(JobEvent::SpeedRecord(record));
            }
            speed::SpeedEvent::Warning(warning) => {
                let _ = tx.send(JobEvent::SpeedWarning(warning));
            }
        })
        .await;
        storage::log_line(&format!(
            "测速结束：{} 个有效，{} 次 429，耗时 {:.1}s{}",
            outcome.records.len(),
            outcome.throttled,
            outcome.elapsed.as_secs_f64(),
            if outcome.stopped {
                "（已停止）"
            } else {
                ""
            }
        ));
        let _ = tx.send(JobEvent::SpeedDone(outcome));
    });
}

pub fn spawn_verify(
    targets: Vec<VerifyTarget>,
    wait: Duration,
    tx: UnboundedSender<JobEvent>,
    cancel: Arc<AtomicBool>,
) {
    storage::log_line(&format!("开始可用性验证：{} 个目标", targets.len()));
    spawn_guarded(JobKind::Verify, tx.clone(), async move {
        let tx = tx;
        let outcome = verify::execute(&targets, wait, cancel, |event| match event {
            verify::VerifyEvent::Progress {
                done,
                total,
                current,
            } => {
                let _ = tx.send(JobEvent::VerifyProgress {
                    done,
                    total,
                    current,
                });
            }
            verify::VerifyEvent::Record(record) => {
                let _ = tx.send(JobEvent::VerifyRecord(record));
            }
        })
        .await;
        storage::log_line(&format!(
            "可用性验证结束：{}/{} 通过，耗时 {:.1}s{}",
            outcome.passed,
            targets.len(),
            outcome.elapsed.as_secs_f64(),
            if outcome.stopped {
                "（已停止）"
            } else {
                ""
            }
        ));
        let _ = tx.send(JobEvent::VerifyDone(outcome));
    });
}

pub fn spawn_daily(config: DailyConfig, tx: UnboundedSender<JobEvent>, cancel: Arc<AtomicBool>) {
    storage::log_line("开始每日优选");
    spawn_guarded(JobKind::Daily, tx.clone(), async move {
        let tx = tx;
        let result = daily::execute(config, cancel, |event| match event {
            daily::DailyEvent::Stage { stage, detail } => {
                let _ = tx.send(JobEvent::DailyStage { stage, detail });
            }
            daily::DailyEvent::Progress {
                stage,
                done,
                total,
                label,
            } => {
                let _ = tx.send(JobEvent::DailyProgress {
                    stage,
                    done,
                    total,
                    label,
                });
            }
            daily::DailyEvent::Log(line) => {
                let _ = tx.send(JobEvent::DailyLog(line));
            }
        })
        .await;
        match result {
            Ok(result) => {
                storage::log_line(&format!(
                    "每日优选结束：Top {}，扫描命中 {}，耗时 {:.1}s{}",
                    result.top.len(),
                    result.summary.hits,
                    result.elapsed.as_secs_f64(),
                    if result.stopped {
                        "（已停止）"
                    } else {
                        ""
                    }
                ));
                let _ = tx.send(JobEvent::DailyDone(result));
            }
            Err(err) => {
                let message = format!("每日优选失败：{err:#}");
                storage::log_line(&message);
                let _ = tx.send(JobEvent::Error(message));
            }
        }
    });
}

fn send_aux_error(tx: &UnboundedSender<JobEvent>, kind: JobKind, err: anyhow::Error) {
    let message = format!("{}失败：{err:#}", kind.label());
    storage::log_line(&message);
    let _ = tx.send(JobEvent::Error(format!("{}失败：{err:#}", kind.label())));
}

pub fn spawn_asn_fetch(asn: String, tx: UnboundedSender<JobEvent>) {
    spawn_aux(JobKind::Asn, tx.clone(), async move {
        match http::fetch_asn_prefixes(&asn, Duration::from_secs(30)).await {
            Ok((prefixes, source)) => {
                let _ = tx.send(JobEvent::AsnReady {
                    prefixes,
                    source: format!("{asn} · {source}"),
                });
            }
            Err(err) => send_aux_error(&tx, JobKind::Asn, err),
        }
    });
}

pub fn spawn_official_pool(total: usize, tx: UnboundedSender<JobEvent>) {
    spawn_aux(JobKind::Pool, tx.clone(), async move {
        let (cidrs, source) = http::fetch_official_cidrs(Duration::from_secs(20)).await;
        match ip::build_official_pool(&cidrs, total) {
            Ok(pool) => {
                let _ = tx.send(JobEvent::PoolReady {
                    ips: pool,
                    source: format!("{source} · {} 个段", cidrs.len()),
                });
            }
            Err(err) => send_aux_error(&tx, JobKind::Pool, anyhow::anyhow!(err)),
        }
    });
}

/// 在后台拉取订阅（失败会自动重试），不占用任务槽：订阅时常要等上几十秒，
/// 期间扫描、测速照常可用。结束时必定发出 `SubscriptionReady` 或 `SubscriptionFailed`。
pub fn spawn_subscription(url: String, tx: UnboundedSender<JobEvent>) {
    runtime().spawn(async move {
        let fetch_url = url.clone();
        let handle = tokio::spawn(async move {
            http::fetch_subscription(&fetch_url, Duration::from_secs(20)).await
        });
        let event = match handle.await {
            Ok(Ok(nodes)) => JobEvent::SubscriptionReady { url, nodes },
            Ok(Err(err)) => JobEvent::SubscriptionFailed {
                url,
                error: format!("{err:#}"),
            },
            Err(err) => JobEvent::SubscriptionFailed {
                url,
                error: format!("订阅任务异常：{err}"),
            },
        };
        if let JobEvent::SubscriptionFailed { error, .. } = &event {
            storage::log_line(&format!("订阅失败：{error}"));
        }
        let _ = tx.send(event);
    });
}

pub fn spawn_pool_rebuild(paths: AppPaths, tx: UnboundedSender<JobEvent>) {
    spawn_aux(JobKind::PoolRebuild, tx.clone(), async move {
        let result = async {
            let (prefixes, source) =
                http::fetch_asn_prefixes("AS906", Duration::from_secs(30)).await?;
            let segments =
                ip::split_to_24(&prefixes, 200_000).map_err(|err| anyhow::anyhow!(err))?;
            storage::save_pool(&paths, &segments)?;
            Ok::<(usize, String), anyhow::Error>((segments.len(), source))
        }
        .await;
        match result {
            Ok((count, source)) => {
                let _ = tx.send(JobEvent::PoolRebuilt { count, source });
            }
            Err(err) => send_aux_error(&tx, JobKind::PoolRebuild, err),
        }
    });
}

pub fn spawn_probe(config: ProbeConfig, tx: UnboundedSender<JobEvent>, cancel: Arc<AtomicBool>) {
    storage::log_line(&format!(
        "开始段池粗测：{} 段，每段 {} 个抽样，并发 {}",
        config.segments.len(),
        config.samples,
        config.concurrency
    ));
    spawn_guarded(JobKind::Probe, tx.clone(), async move {
        let tx = tx;
        let outcome = probe::execute(&config, cancel, |event| match event {
            probe::ProbeEvent::Phase { name, total } => {
                let _ = tx.send(JobEvent::ProbePhase { name, total });
            }
            probe::ProbeEvent::Progress {
                phase,
                done,
                total,
                current,
            } => {
                let _ = tx.send(JobEvent::ProbeProgress {
                    phase,
                    done,
                    total,
                    current,
                });
            }
            probe::ProbeEvent::Warning(line) => {
                storage::log_line(&line);
                let _ = tx.send(JobEvent::ProbeWarning(line));
            }
        })
        .await;
        storage::log_line(&format!(
            "段池粗测结束：握手命中 {} 个地址，{} 段完成吞吐，耗时 {:.1}s{}",
            outcome.handshake_hits,
            outcome.throughput_tested,
            outcome.elapsed.as_secs_f64(),
            if outcome.stopped {
                "（已停止）"
            } else {
                ""
            }
        ));
        let _ = tx.send(JobEvent::ProbeDone(outcome));
    });
}

pub fn spawn_classify(
    config: ClassifyConfig,
    tx: UnboundedSender<JobEvent>,
    cancel: Arc<AtomicBool>,
) {
    storage::log_line(&format!(
        "开始划分出口区域：{} 段，并发 {}",
        config.segments.len(),
        config.concurrency
    ));
    spawn_guarded(JobKind::Classify, tx.clone(), async move {
        let progress = tx.clone();
        let outcome = region::classify(&config, cancel, |step| {
            let _ = progress.send(JobEvent::ClassifyProgress(step));
        })
        .await;
        storage::log_line(&format!(
            "出口区域划分{}：{} 段中 {} 段有响应，耗时 {:.1}s",
            if outcome.stopped {
                "已停止"
            } else {
                "完成"
            },
            outcome.segments.len(),
            outcome.responded,
            outcome.elapsed.as_secs_f64()
        ));
        let _ = tx.send(JobEvent::ClassifyDone(outcome));
    });
}

/// 检查 GitHub 上的最新版本，结果见 `UpdateChecked`。
pub fn spawn_update_check(tx: UnboundedSender<JobEvent>) {
    runtime().spawn(async move {
        let result = update::latest_release()
            .await
            .map_err(|err| format!("{err:#}"));
        if let Err(err) = &result {
            storage::log_line(&format!("检查更新失败：{err}"));
        }
        let _ = tx.send(JobEvent::UpdateChecked(result));
    });
}

/// 测速选线下载安装包并校验，进度见 `UpdateProgress`，结果见 `UpdateDownloaded`。
pub fn spawn_update_download(
    asset: update::Asset,
    cancel: Arc<AtomicBool>,
    tx: UnboundedSender<JobEvent>,
) {
    storage::log_line(&format!("开始下载更新：{}", asset.name));
    runtime().spawn(async move {
        let progress_tx = tx.clone();
        let handle = tokio::spawn(async move {
            update::download(&asset, cancel, |progress| {
                let _ = progress_tx.send(JobEvent::UpdateProgress(progress));
            })
            .await
        });
        let result = match handle.await {
            Ok(Ok(bytes)) => Ok(bytes),
            Ok(Err(err)) => Err(format!("{err:#}")),
            Err(err) => Err(format!("下载任务异常：{err}")),
        };
        match &result {
            Ok(bytes) => storage::log_line(&format!("更新已下载并校验：{} 字节", bytes.len())),
            Err(err) => storage::log_line(&format!("下载更新失败：{err}")),
        }
        let _ = tx.send(JobEvent::UpdateDownloaded(result));
    });
}
