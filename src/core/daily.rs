use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use chrono::Local;

use crate::core::ip;
use crate::core::model::{
    DailyEntry, DailyState, DailySummary, DailyVerification, ScanHit, SegmentScore, VlessNode,
};
use crate::core::storage::AppPaths;
use crate::core::verify::{self, VerifyEvent, VerifyTarget};
use crate::core::{http, pool, scan, speed, storage};

#[derive(Clone, Debug)]
pub struct DailyConfig {
    pub paths: AppPaths,
    pub state: DailyState,
    pub segments: usize,
    pub test: usize,
    pub top: usize,
    pub concurrency: usize,
    pub scan_timeout: Duration,
    pub node: VlessNode,
    pub strategy: String,
    pub scores: Vec<SegmentScore>,
}

#[derive(Clone, Debug)]
pub enum DailyEvent {
    Stage {
        stage: String,
        detail: String,
    },
    Progress {
        stage: String,
        done: usize,
        total: usize,
        label: String,
    },
    Log(String),
}

#[derive(Clone, Debug, Default)]
pub struct DailyResult {
    pub top: Vec<DailyEntry>,
    pub summary: DailySummary,
    pub state: DailyState,
    pub stopped: bool,
    pub elapsed: Duration,
    pub hit_path: Option<std::path::PathBuf>,
    pub output_path: Option<std::path::PathBuf>,
}

/// 每日自动优选是否该开始：到点后当天还没运行过就开始，错过时间（睡眠、关机）之后补跑。
pub fn auto_daily_due(today: &str, now: (u8, u8), last_date: &str, at: (u8, u8)) -> bool {
    last_date != today && now >= at
}

async fn ensure_pool(
    paths: &AppPaths,
    emit_sender: &mut impl FnMut(DailyEvent),
) -> Result<Vec<String>> {
    let existing = storage::load_pool(paths);
    if !existing.is_empty() {
        emit_sender(DailyEvent::Log(format!(
            "段池：{} 个段（{}）",
            existing.len(),
            paths.pool.display()
        )));
        return Ok(existing);
    }

    emit_sender(DailyEvent::Stage {
        stage: "段池".into(),
        detail: "拉取 AS906 前缀并拆分为 /24".into(),
    });
    emit_sender(DailyEvent::Log(
        "段池为空，正在从 RIPE 拉取 AS906 announced-prefixes…".into(),
    ));
    let (prefixes, source) = http::fetch_asn_prefixes("AS906", Duration::from_secs(30)).await?;
    emit_sender(DailyEvent::Log(format!("AS906：{source}")));
    let segments = ip::split_to_24(&prefixes, 200_000).map_err(anyhow::Error::msg)?;
    storage::save_pool(paths, &segments)?;
    emit_sender(DailyEvent::Log(format!(
        "段池已生成：{} 个 /24 → {}",
        segments.len(),
        paths.pool.display()
    )));
    Ok(segments)
}

async fn sleep_cancel(seconds: f64, cancel: &Arc<AtomicBool>) {
    let deadline = Instant::now() + Duration::from_secs_f64(seconds);
    while Instant::now() < deadline && !cancel.load(Ordering::Relaxed) {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

pub async fn execute<F>(
    config: DailyConfig,
    cancel: Arc<AtomicBool>,
    mut emit: F,
) -> Result<DailyResult>
where
    F: FnMut(DailyEvent) + Send,
{
    let started = Instant::now();
    let mut result = DailyResult {
        state: config.state.clone(),
        ..Default::default()
    };
    let log = |emit: &mut F, message: String| {
        storage::log_line(&message);
        emit(DailyEvent::Log(message));
    };

    // 与 verify::verify_one 的分支一致：仅 VLESS WebSocket 隧道需要 UUID。
    let verification = if config.node.transport.eq_ignore_ascii_case("ws") {
        if config.node.uuid.trim().is_empty() {
            bail!("VLESS 节点 UUID 为空，无法进行隧道验证");
        }
        uuid::Uuid::parse_str(config.node.uuid.trim()).context("UUID 格式无效")?;
        DailyVerification::Tunnel
    } else {
        DailyVerification::Tls
    };
    result.summary.verification = verification;
    log(
        &mut emit,
        if verification == DailyVerification::Tls {
            "本轮使用 TLS 基础验证（无需 UUID），结果不代表代理节点可用".into()
        } else {
            "本轮使用 VLESS 隧道验证".into()
        },
    );

    let pool = ensure_pool(&config.paths, &mut emit).await?;
    if pool.is_empty() {
        bail!("段池为空");
    }

    let strategy = pool::normalize_strategy(&config.strategy);
    let ordered = pool::ordered_pool(&pool, strategy, &config.scores);
    let pool_len = ordered.len();
    if pool_len == 0 {
        bail!("段池为空");
    }
    let cursor = pool::normalize_cursor(config.state.cursor, pool_len);
    let batch_len = pool::batch_size(config.segments, pool_len);
    let batch = pool::batch_segments(&ordered, cursor, batch_len);
    let positions = pool::batch_positions(cursor, batch_len, pool_len);
    let (next_cursor, wrapped) = pool::advance(cursor, batch_len, pool_len);
    let next_round = if wrapped {
        config.state.round + 1
    } else {
        config.state.round
    };

    emit(DailyEvent::Stage {
        stage: "扫描".into(),
        detail: format!("{positions}（第 {} 轮）", config.state.round.max(1)),
    });
    log(
        &mut emit,
        format!(
            "段池策略：{} · {}",
            pool::strategy_label(strategy),
            pool::strategy_detail(strategy)
        ),
    );
    if strategy == "network"
        && !config.scores.iter().any(|record| {
            record.score > 0.0 && pool.iter().any(|segment| segment == &record.segment)
        })
    {
        log(
            &mut emit,
            "尚无有效粗测结果，网络优选本轮暂按均衡顺序推进".into(),
        );
    }
    log(
        &mut emit,
        format!(
            "本轮取段：{}（起点第 {} 段 → 下轮第 {} 段）",
            positions,
            cursor + 1,
            next_cursor + 1
        ),
    );
    log(&mut emit, format!("本轮段：{}", batch.join(", ")));

    let targets = ip::parse_targets(&batch.join("\n"), &[443], 0.0, ip::MAX_TARGETS)
        .map_err(anyhow::Error::msg)?;
    let scan_config = scan::ScanConfig {
        targets: targets.clone(),
        concurrency: config.concurrency,
        timeout: config.scan_timeout,
    };
    log(
        &mut emit,
        format!(
            "扫描 {} 个目标（IP × 端口），并发 {}…",
            targets.len(),
            config.concurrency
        ),
    );

    let scan_outcome = scan::execute(&scan_config, cancel.clone(), |event| match event {
        scan::ScanEvent::Progress {
            done,
            total,
            hits,
            elapsed,
        } => {
            emit(DailyEvent::Progress {
                stage: "扫描".into(),
                done,
                total,
                label: format!("命中 {hits} · {:.0}s", elapsed.as_secs_f64()),
            });
        }
        scan::ScanEvent::Hit(_) => {}
    })
    .await;
    let hits: Vec<ScanHit> = scan_outcome.hits;
    result.summary.segments = batch.clone();
    result.summary.targets = targets.len();
    result.summary.scanned = scan_outcome.done;
    result.summary.hits = hits.len();
    log(
        &mut emit,
        format!(
            "扫描完成：命中 {} / {}（{:.1}%），耗时 {:.1}s",
            hits.len(),
            scan_outcome.total,
            if scan_outcome.total == 0 {
                0.0
            } else {
                100.0 * hits.len() as f64 / scan_outcome.total as f64
            },
            scan_outcome.elapsed.as_secs_f64()
        ),
    );

    if cancel.load(Ordering::Relaxed) {
        result.stopped = true;
        result.elapsed = started.elapsed();
        return Ok(result);
    }

    if !hits.is_empty() {
        match storage::write_daily_hits(&config.paths, &hits) {
            Ok(path) => {
                result.hit_path = Some(path.clone());
                log(&mut emit, format!("扫描命中已保存：{}", path.display()));
            }
            Err(err) => log(&mut emit, format!("保存扫描命中失败：{err}")),
        }
    }

    let mut advance_state = true;
    let mut scored: Vec<(String, u16, String, f64, f64, f64)> = Vec::new();

    if hits.is_empty() {
        log(
            &mut emit,
            "本轮没有可连通的 Cloudflare 节点，直接进入下一批".into(),
        );
    } else {
        let candidates: Vec<&ScanHit> = hits.iter().take(config.test.max(1)).collect();
        log(
            &mut emit,
            format!(
                "测速 {} 个候选（每个 2 轮 × 4 秒，逐个进行）…",
                candidates.len()
            ),
        );
        emit(DailyEvent::Stage {
            stage: "测速".into(),
            detail: format!("逐个测速 {} 个候选", candidates.len()),
        });

        let mut blocked = 0;
        let mut circuit_broken = false;
        for (ix, hit) in candidates.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                result.stopped = true;
                break;
            }
            let mut values = Vec::new();
            for round in 0..2 {
                if cancel.load(Ordering::Relaxed) {
                    result.stopped = true;
                    break;
                }
                let download =
                    speed::download_mbps(&hit.ip, hit.port, 4, Duration::from_secs(6)).await;
                if let Some(mbps) = download.mbps {
                    values.push(mbps);
                } else if download.is_throttled() {
                    blocked += 1;
                    log(
                        &mut emit,
                        format!("{} 被限流（429，连续第 {} 个），跳过该 IP", hit.ip, blocked),
                    );
                    if blocked >= 3 {
                        log(
                            &mut emit,
                            "连续 3 个 IP 被限流（429），已停止测速；请 30–60 分钟后再试".into(),
                        );
                        circuit_broken = true;
                        break;
                    }
                    sleep_cancel(1.5, &cancel).await;
                    break;
                } else {
                    log(&mut emit, format!("{} 无效（{}）", hit.ip, download.note));
                    break;
                }
                if !download.is_throttled() {
                    blocked = 0;
                }
                if round == 0 {
                    sleep_cancel(1.0, &cancel).await;
                }
            }
            if circuit_broken || result.stopped {
                break;
            }
            if values.is_empty() {
                continue;
            }
            let best = values.iter().copied().fold(0.0, f64::max);
            let avg = values.iter().sum::<f64>() / values.len() as f64;
            log(
                &mut emit,
                format!(
                    "[{}/{}] {} {} · 最快 {:.1} / 均值 {:.1} Mbps",
                    ix + 1,
                    candidates.len(),
                    hit.ip,
                    hit.colo,
                    best,
                    avg
                ),
            );
            scored.push((
                hit.ip.clone(),
                hit.port,
                hit.colo.clone(),
                hit.latency_ms,
                best,
                avg,
            ));
            emit(DailyEvent::Progress {
                stage: "测速".into(),
                done: ix + 1,
                total: candidates.len(),
                label: format!("{} · {:.1} Mbps", hit.ip, best),
            });
            sleep_cancel(0.8, &cancel).await;
        }
        result.summary.tested = scored.len();
        if scored.is_empty() {
            log(
                &mut emit,
                "没有测到有效速度（可能被限流或全部失败）；扫描结果已保存，进度照常推进".into(),
            );
        }
    }

    let mut final_rows: Vec<DailyEntry> = Vec::new();
    if !scored.is_empty() && !result.stopped {
        scored.sort_by(|a, b| b.4.total_cmp(&a.4));
        let verify_limit = config.top.max(1) * 2;
        emit(DailyEvent::Stage {
            stage: "验证".into(),
            detail: format!(
                "{}前 {} 个",
                verification.label(),
                verify_limit.min(scored.len())
            ),
        });
        for (ix, (ip, port, colo, latency_ms, best, avg)) in
            scored.iter().take(verify_limit).enumerate()
        {
            if cancel.load(Ordering::Relaxed) {
                result.stopped = true;
                break;
            }
            let target = VerifyTarget {
                addr: ip.clone(),
                port: *port,
                node: config.node.clone(),
            };
            emit(DailyEvent::Progress {
                stage: "验证".into(),
                done: ix,
                total: verify_limit.min(scored.len()),
                label: format!("{ip}:{port}"),
            });
            let record = timeout_verify(&target, &cancel).await;
            log(
                &mut emit,
                format!(
                    "[验证] {} {} {} {}",
                    ip,
                    record.colo.clone().unwrap_or_else(|| "-".into()),
                    if record.ok { "通过" } else { "失败" },
                    record.detail
                ),
            );
            if record.ok {
                final_rows.push(DailyEntry {
                    ip: ip.clone(),
                    port: *port,
                    colo: record.colo.unwrap_or_else(|| colo.clone()),
                    best_mbps: *best,
                    avg_mbps: *avg,
                    latency_ms: *latency_ms,
                    verification,
                });
                if final_rows.len() >= config.top.max(1) {
                    break;
                }
            }
            emit(DailyEvent::Progress {
                stage: "验证".into(),
                done: ix + 1,
                total: verify_limit.min(scored.len()),
                label: format!("{} · 通过 {}", ip, final_rows.len()),
            });
            sleep_cancel(0.3, &cancel).await;
        }
        result.summary.verified = final_rows.len();
        log(
            &mut emit,
            format!("{}完成：{} 个通过", verification.label(), final_rows.len()),
        );
    }

    if result.stopped {
        advance_state = false;
    }

    let now = Local::now();
    result.top = final_rows.clone();
    result.summary.cursor = if advance_state { next_cursor } else { cursor };
    result.summary.round = if advance_state {
        next_round
    } else {
        config.state.round
    };
    result.state.cursor = result.summary.cursor;
    result.state.round = result.summary.round;
    result.state.strategy = strategy.to_string();
    result.state.pool_count = pool_len;
    if advance_state {
        result.state.last_run = now.format("%Y-%m-%d %H:%M").to_string();
        result.state.last_top = final_rows.clone();
    }
    storage::save_daily_state(&config.paths, &result.state);

    if !final_rows.is_empty() {
        match storage::write_daily_output(&config.paths, &final_rows) {
            Ok(path) => {
                result.output_path = Some(path.clone());
                log(
                    &mut emit,
                    format!("今日 Top {} 已保存：{}", final_rows.len(), path.display()),
                );
            }
            Err(err) => log(&mut emit, format!("保存每日优选失败：{err}")),
        }
        if advance_state {
            log(
                &mut emit,
                format!(
                    "进度推进：起点第 {} 段 → 第 {} 段，第 {} 轮",
                    cursor + 1,
                    next_cursor + 1,
                    next_round
                ),
            );
        }
    } else if advance_state {
        log(
            &mut emit,
            format!(
                "进度推进：起点第 {} 段 → 第 {} 段，第 {} 轮（本轮无可用 IP）",
                cursor + 1,
                next_cursor + 1,
                next_round
            ),
        );
    }

    result.elapsed = started.elapsed();
    if cancel.load(Ordering::Relaxed) {
        result.stopped = true;
    }
    Ok(result)
}

async fn timeout_verify(
    target: &VerifyTarget,
    cancel: &Arc<AtomicBool>,
) -> crate::core::model::VerifyRecord {
    let result = verify::execute(
        std::slice::from_ref(target),
        Duration::from_secs(8),
        cancel.clone(),
        |_event: VerifyEvent| {},
    )
    .await;
    result
        .records
        .into_iter()
        .next()
        .unwrap_or_else(|| crate::core::model::VerifyRecord {
            ip: target.addr.clone(),
            port: target.port,
            transport: target.node.transport.clone(),
            ok: false,
            stage: "ERR".into(),
            detail: "验证未返回结果".into(),
            colo: None,
            ms: None,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct Fixture {
        config: DailyConfig,
    }

    impl Fixture {
        fn new(transport: &str, uuid: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "cfbest-daily-test-{}-{}",
                std::process::id(),
                rand::random::<u64>()
            ));
            fs::create_dir(&root).unwrap();
            let paths = AppPaths {
                exports: root.join("exports"),
                settings: root.join("settings.json"),
                history: root.join("history.json"),
                daily_state: root.join("daily_state.json"),
                pool: root.join("pool.txt"),
                segment_scores: root.join("segment_scores.json"),
                log: root.join("runtime.log"),
                root,
            };
            // 只探测本机回环地址，不依赖订阅、RIPE 或 Cloudflare 网络。
            storage::save_pool(&paths, &["127.0.0.1/32".into()]).unwrap();
            Self {
                config: DailyConfig {
                    paths,
                    state: DailyState::default(),
                    segments: 1,
                    test: 1,
                    top: 1,
                    concurrency: 1,
                    scan_timeout: Duration::from_millis(1),
                    node: VlessNode {
                        name: "test".into(),
                        uuid: uuid.into(),
                        addr: "cp.cloudflare.com".into(),
                        port: 443,
                        transport: transport.into(),
                        path: "/cdn-cgi/trace".into(),
                        host: "cp.cloudflare.com".into(),
                        sni: "cp.cloudflare.com".into(),
                        params: Vec::new(),
                    },
                    strategy: "sequential".into(),
                    scores: Vec::new(),
                },
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let paths = &self.config.paths;
            let _ = fs::remove_file(&paths.pool);
            let _ = fs::remove_file(&paths.daily_state);
            let _ = fs::remove_dir(&paths.root);
        }
    }

    #[test]
    fn auto_daily_due_catches_up_once_per_day() {
        let at = (9, 0);
        assert!(!auto_daily_due("2026-10-28", (8, 59), "2026-10-27", at));
        assert!(auto_daily_due("2026-10-28", (9, 0), "2026-10-27", at));
        // 9 点在睡眠或忙碌中错过，之后补跑。
        assert!(auto_daily_due("2026-10-28", (15, 30), "2026-10-27", at));
        assert!(auto_daily_due("2026-10-28", (9, 1), "", at));
        assert!(!auto_daily_due("2026-10-28", (15, 30), "2026-10-28", at));
    }

    #[tokio::test]
    async fn daily_without_subscription_scans_and_saves_progress() {
        let fixture = Fixture::new("tls", "");
        let mut scanned = false;
        let result = execute(
            fixture.config.clone(),
            Arc::new(AtomicBool::new(false)),
            |event| {
                if let DailyEvent::Stage { stage, .. } = event {
                    scanned |= stage == "扫描";
                }
            },
        )
        .await
        .expect("无订阅的每日优选应完成基础扫描");

        assert!(scanned);
        assert!(!result.stopped);
        assert_eq!(result.summary.verification, DailyVerification::Tls);
        assert_eq!(result.summary.targets, 1);
        assert_eq!(result.summary.scanned, 1);
        let saved = storage::load_daily_state(&fixture.config.paths);
        assert_eq!(saved.round, 2);
        assert_eq!(saved.cursor, 0);
        assert!(!saved.last_run.is_empty());
    }

    #[tokio::test]
    async fn daily_websocket_rejects_missing_or_invalid_uuid_before_scanning() {
        for uuid in ["", " \t ", "not-a-uuid"] {
            let fixture = Fixture::new("WS", uuid);
            let mut events = Vec::new();
            let error = execute(
                fixture.config.clone(),
                Arc::new(AtomicBool::new(false)),
                |event| events.push(event),
            )
            .await
            .expect_err("VLESS WebSocket 隧道仍然需要有效 UUID");

            assert!(error.to_string().contains("UUID"));
            assert!(events.is_empty());
            assert!(!fixture.config.paths.daily_state.exists());
        }
    }

    #[tokio::test]
    async fn daily_websocket_with_valid_uuid_keeps_tunnel_verification() {
        let fixture = Fixture::new("WS", " b831381d-6324-4d53-ad4f-8cda48b30811 ");
        let result = execute(
            fixture.config.clone(),
            Arc::new(AtomicBool::new(false)),
            |_| {},
        )
        .await
        .unwrap();

        assert_eq!(result.summary.verification, DailyVerification::Tunnel);
        assert_eq!(result.summary.scanned, 1);
        assert!(!result.stopped);
    }
}
