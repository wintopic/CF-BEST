use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::core::ip::CidrSpec;
use crate::core::model::SegmentScore;
use crate::core::pool;
use crate::core::{scan, speed};

const PROBE_PORT: u16 = 443;
const MAX_SAMPLES: usize = 16;

#[derive(Clone, Debug)]
pub struct ProbeConfig {
    pub segments: Vec<String>,
    pub samples: usize,
    pub concurrency: usize,
    pub timeout: Duration,
    pub throughput_top: usize,
    pub throughput_secs: u64,
}

#[derive(Clone, Debug)]
pub enum ProbeEvent {
    Phase {
        name: &'static str,
        total: usize,
    },
    Progress {
        phase: &'static str,
        done: usize,
        total: usize,
        current: String,
    },
    Warning(String),
}

#[derive(Clone, Debug, Default)]
pub struct ProbeOutcome {
    pub records: Vec<SegmentScore>,
    pub elapsed: Duration,
    pub stopped: bool,
    pub handshake_hits: usize,
    pub throughput_tested: usize,
}

#[derive(Default)]
struct Aggregate {
    hits: usize,
    sum_ms: f64,
    best_ms: f64,
    colo: String,
    ip: String,
}

/// 均匀分布的采样点，保证同一段每次粗测使用同样的地址，便于横向比较。
pub fn sample_offsets(samples: usize) -> Vec<u8> {
    let samples = samples.clamp(1, MAX_SAMPLES);
    (0..samples)
        .map(|index| {
            let value = ((2 * index + 1) * 255) / (2 * samples);
            value.clamp(1, 254) as u8
        })
        .collect()
}

fn segment_octets(segment: &str) -> Option<[u8; 3]> {
    let spec = CidrSpec::parse(segment).ok()?;
    if spec.prefix != 24 {
        return None;
    }
    let octets = Ipv4Addr::from(spec.network).octets();
    Some([octets[0], octets[1], octets[2]])
}

fn key_for_octets(octets: [u8; 3]) -> String {
    format!("{}.{}.{}.0/24", octets[0], octets[1], octets[2])
}

pub(crate) fn key_for_ip(ip: &str) -> Option<String> {
    let octets = ip.parse::<Ipv4Addr>().ok()?.octets();
    Some(key_for_octets([octets[0], octets[1], octets[2]]))
}

fn score_for(record: &SegmentScore) -> f64 {
    if record.hits == 0 {
        0.0
    } else if let Some(mbps) = record.mbps {
        1000.0 + mbps.max(0.0)
    } else {
        50.0 + (300.0 - record.avg_latency_ms).clamp(0.0, 300.0) / 10.0
    }
}

fn finalize(records: &mut [SegmentScore]) {
    for record in records.iter_mut() {
        record.score = score_for(record);
    }
    records.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.best_latency_ms.total_cmp(&b.best_latency_ms))
            .then_with(|| a.segment.cmp(&b.segment))
    });
}

pub async fn execute<F>(config: &ProbeConfig, cancel: Arc<AtomicBool>, mut emit: F) -> ProbeOutcome
where
    F: FnMut(ProbeEvent) + Send,
{
    let started = Instant::now();
    let offsets = sample_offsets(config.samples);
    let mut segments = Vec::new();
    let mut index_by_key: HashMap<String, usize> = HashMap::new();
    let mut targets = Vec::new();

    for segment in &config.segments {
        let Some(octets) = segment_octets(segment) else {
            emit(ProbeEvent::Warning(format!("粗测跳过非 /24 段：{segment}")));
            continue;
        };
        let key = key_for_octets(octets);
        if index_by_key.contains_key(&key) {
            continue;
        }
        index_by_key.insert(key, segments.len());
        segments.push(segment.clone());
        for offset in &offsets {
            targets.push((
                Ipv4Addr::new(octets[0], octets[1], octets[2], *offset),
                PROBE_PORT,
            ));
        }
    }

    let mut records: Vec<SegmentScore> = segments
        .iter()
        .map(|segment| SegmentScore {
            segment: segment.clone(),
            group: pool::group_key(segment),
            samples: offsets.len(),
            hits: 0,
            colo: String::new(),
            best_latency_ms: 0.0,
            avg_latency_ms: 0.0,
            mbps: None,
            score: 0.0,
        })
        .collect();

    let mut outcome = ProbeOutcome::default();
    if targets.is_empty() {
        outcome.elapsed = started.elapsed();
        return outcome;
    }

    emit(ProbeEvent::Phase {
        name: "握手",
        total: targets.len(),
    });
    let scan_config = scan::ScanConfig {
        targets,
        concurrency: config.concurrency.clamp(1, 300),
        timeout: config.timeout,
    };
    let scan_outcome = scan::execute(&scan_config, cancel.clone(), |event| {
        if let scan::ScanEvent::Progress {
            done,
            total,
            hits,
            elapsed,
        } = event
        {
            emit(ProbeEvent::Progress {
                phase: "握手",
                done,
                total,
                current: format!("命中 {hits} · {:.0}s", elapsed.as_secs_f64()),
            });
        }
    })
    .await;

    let mut aggregates: HashMap<usize, Aggregate> = HashMap::new();
    for hit in &scan_outcome.hits {
        let Some(key) = key_for_ip(&hit.ip) else {
            continue;
        };
        let Some(&index) = index_by_key.get(&key) else {
            continue;
        };
        let aggregate = aggregates.entry(index).or_insert_with(|| Aggregate {
            best_ms: f64::MAX,
            ..Default::default()
        });
        aggregate.hits += 1;
        aggregate.sum_ms += hit.latency_ms;
        if hit.latency_ms < aggregate.best_ms {
            aggregate.best_ms = hit.latency_ms;
            aggregate.ip = hit.ip.clone();
        }
        if aggregate.colo.is_empty() && !hit.colo.is_empty() {
            aggregate.colo = hit.colo.clone();
        }
    }

    for (index, aggregate) in &aggregates {
        if let Some(record) = records.get_mut(*index) {
            record.hits = aggregate.hits;
            record.best_latency_ms = aggregate.best_ms;
            record.avg_latency_ms = aggregate.sum_ms / aggregate.hits as f64;
            record.colo = aggregate.colo.clone();
        }
    }
    outcome.handshake_hits = scan_outcome.hits.len();

    if cancel.load(Ordering::Relaxed) || scan_outcome.stopped {
        outcome.stopped = true;
        finalize(&mut records);
        outcome.records = records;
        outcome.elapsed = started.elapsed();
        return outcome;
    }

    let mut candidates: Vec<(usize, String)> = aggregates
        .iter()
        .filter(|(_, aggregate)| aggregate.hits > 0 && !aggregate.ip.is_empty())
        .map(|(index, aggregate)| (*index, aggregate.ip.clone()))
        .collect();
    candidates.sort_by(|a, b| {
        records[a.0]
            .avg_latency_ms
            .total_cmp(&records[b.0].avg_latency_ms)
    });
    candidates.truncate(config.throughput_top.clamp(1, 32));

    if !candidates.is_empty() {
        emit(ProbeEvent::Phase {
            name: "吞吐",
            total: candidates.len(),
        });
        for (done, (index, ip)) in candidates.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                outcome.stopped = true;
                break;
            }
            emit(ProbeEvent::Progress {
                phase: "吞吐",
                done,
                total: candidates.len(),
                current: format!("{ip} · {}", records[*index].segment),
            });
            let download = speed::download_mbps(
                ip,
                PROBE_PORT,
                config.throughput_secs.clamp(1, 10),
                Duration::from_secs(6),
            )
            .await;
            if let Some(mbps) = download.mbps {
                records[*index].mbps = Some(mbps);
                outcome.throughput_tested += 1;
            } else if download.is_throttled() {
                emit(ProbeEvent::Warning(
                    "粗测测速触发 Cloudflare 限流（429），已停止剩余测速".to_string(),
                ));
                emit(ProbeEvent::Progress {
                    phase: "吞吐",
                    done: done + 1,
                    total: candidates.len(),
                    current: "被限流，跳过剩余测速".to_string(),
                });
                break;
            }
            emit(ProbeEvent::Progress {
                phase: "吞吐",
                done: done + 1,
                total: candidates.len(),
                current: format!("{ip} · 完成"),
            });
        }
    }

    finalize(&mut records);
    outcome.records = records;
    outcome.elapsed = started.elapsed();
    if cancel.load(Ordering::Relaxed) {
        outcome.stopped = true;
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_offsets_are_spread_and_stable() {
        assert_eq!(sample_offsets(1), vec![127]);
        assert_eq!(sample_offsets(2), vec![63, 191]);
        let offsets = sample_offsets(8);
        assert_eq!(offsets.len(), 8);
        assert!(offsets.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(offsets.iter().all(|offset| (1..=254).contains(offset)));
    }

    #[test]
    fn only_slash_24_segments_are_probed() {
        assert_eq!(segment_octets("64.186.224.0/24"), Some([64, 186, 224]));
        assert_eq!(segment_octets("64.186.224.0/20"), None);
        assert_eq!(segment_octets("not-an-ip"), None);
    }

    #[test]
    fn ip_maps_back_to_segment() {
        assert_eq!(
            key_for_ip("64.186.224.37"),
            Some("64.186.224.0/24".to_string())
        );
        assert_eq!(key_for_ip("bad"), None);
    }

    #[test]
    fn throughput_outranks_handshake_only() {
        let alive = SegmentScore {
            segment: "1.1.1.0/24".into(),
            group: "1.1".into(),
            samples: 2,
            hits: 2,
            colo: "HKG".into(),
            best_latency_ms: 40.0,
            avg_latency_ms: 45.0,
            mbps: None,
            score: 0.0,
        };
        let mut measured = alive.clone();
        measured.mbps = Some(30.0);
        let dead = SegmentScore {
            hits: 0,
            score: 0.0,
            ..alive.clone()
        };
        assert_eq!(score_for(&dead), 0.0);
        assert!(score_for(&measured) > score_for(&alive));
        assert!(score_for(&alive) > score_for(&dead));
    }
}
