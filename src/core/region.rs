//! 出口区域：按本机网络实际落到的 Cloudflare 机房给段池里的 /24 段划分国家 / 地区，
//! 并按用户锁定的区域筛选段与结果。
//!
//! Cloudflare 的地址是任播的，同一个段从不同网络连过去会落到不同机房，GeoIP 也几乎都标成美国，
//! 所以段的「区域」只能在本机实测：对每个段探测 `/cdn-cgi/trace`，记下它落到的机房。
//! Workers / Pages 搭建的节点在客户端落到的机房运行，这个机房就是出口所在地。

use std::collections::{HashMap, HashSet};
use std::net::Ipv4Addr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use tokio::task::JoinSet;
use tokio::time::timeout;

use crate::core::colo;
use crate::core::http;
use crate::core::ip::CidrSpec;
use crate::core::model::{ScanHit, SegmentRegion, SegmentRegions};
use crate::core::probe::key_for_ip;

/// 一个段命中这么多个地址就算划分完成，剩下的地址不再探测。
const ENOUGH_HITS: u32 = 2;
/// 每个 /24 段可探测的地址数（.1 – .254）。
const ADDRESSES: u32 = 254;

/// 锁定的出口区域：国家 / 地区代码（`HK`、`JP`）与机房代码（`SJC`），为空表示不锁定。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RegionLock {
    countries: Vec<String>,
    colos: Vec<String>,
}

impl RegionLock {
    /// 两个字母视为国家 / 地区，三个字母视为机房，其余忽略；大小写、重复不敏感。
    pub fn parse<S: AsRef<str>>(entries: &[S]) -> Self {
        let mut lock = Self::default();
        for entry in entries {
            let code = entry.as_ref().trim().to_ascii_uppercase();
            if !code.bytes().all(|byte| byte.is_ascii_uppercase()) {
                continue;
            }
            let list = match code.len() {
                2 => &mut lock.countries,
                3 => &mut lock.colos,
                _ => continue,
            };
            if !list.contains(&code) {
                list.push(code);
            }
        }
        lock.countries.sort();
        lock.colos.sort();
        lock
    }

    /// 解析输入框里的文字：逗号、顿号、空格分隔。
    pub fn parse_text(text: &str) -> Self {
        let entries: Vec<&str> = text
            .split([',', '，', '、', ' ', ';', '；', '\n'])
            .filter(|entry| !entry.trim().is_empty())
            .collect();
        Self::parse(&entries)
    }

    pub fn is_empty(&self) -> bool {
        self.countries.is_empty() && self.colos.is_empty()
    }

    pub fn matches(&self, colo: &str) -> bool {
        if self.is_empty() {
            return true;
        }
        let colo = colo.trim().to_ascii_uppercase();
        if colo.is_empty() {
            return false;
        }
        self.colos.contains(&colo)
            || self
                .countries
                .iter()
                .any(|country| country == colo::country_code(&colo))
    }

    /// 保存到设置里的形式：国家 / 地区在前，机房在后。
    pub fn entries(&self) -> Vec<String> {
        self.countries.iter().chain(&self.colos).cloned().collect()
    }

    pub fn countries(&self) -> &[String] {
        &self.countries
    }

    /// 只有局域网服务（`cfbest-cli serve`）用到。
    #[allow(dead_code)]
    pub fn colos(&self) -> &[String] {
        &self.colos
    }

    /// 区分不同锁定的键，换了锁定就从头轮转。
    pub fn key(&self) -> String {
        self.entries().join(",")
    }

    /// 界面上的说明，如「香港、日本、SJC」；不锁定时为「不限」。
    pub fn label(&self) -> String {
        if self.is_empty() {
            return "不限".to_string();
        }
        self.countries
            .iter()
            .map(|code| colo::country_name(code))
            .chain(self.colos.iter().cloned())
            .collect::<Vec<_>>()
            .join("、")
    }
}

/// 某个国家 / 地区在段池里的分布，供界面列出可选区域。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CountryCount {
    pub code: String,
    pub name: String,
    /// 大区英文名。
    pub region: &'static str,
    pub segments: usize,
    /// 出现过的机房，多的在前。
    pub colos: Vec<String>,
}

impl SegmentRegions {
    fn index_of(&self) -> HashMap<String, usize> {
        self.records
            .iter()
            .enumerate()
            .map(|(index, record)| (record.segment.clone(), index))
            .collect()
    }

    fn entry<'a>(
        records: &'a mut Vec<SegmentRegion>,
        index: &mut HashMap<String, usize>,
        segment: &str,
    ) -> &'a mut SegmentRegion {
        let position = *index.entry(segment.to_string()).or_insert_with(|| {
            records.push(SegmentRegion {
                segment: segment.to_string(),
                ..Default::default()
            });
            records.len() - 1
        });
        &mut records[position]
    }

    /// 完整扫描过的段（每日优选的一批、区域划分）：用这次的结果替换原记录，
    /// 没有命中的段记为无响应。`probed` 为每段探测的地址数。
    pub fn record_scan(
        &mut self,
        segments: &[String],
        probed: &HashMap<String, u32>,
        hits: &[ScanHit],
        when: &str,
    ) {
        let mut index = self.index_of();
        let mut counts: HashMap<String, HashMap<String, u32>> = HashMap::new();
        for hit in hits {
            if let Some(segment) = key_for_ip(&hit.ip)
                && !hit.colo.is_empty()
            {
                *counts
                    .entry(segment)
                    .or_default()
                    .entry(hit.colo.to_ascii_uppercase())
                    .or_default() += 1;
            }
        }
        for segment in segments {
            let Some(key) = normalize_segment(segment) else {
                continue;
            };
            let record = Self::entry(&mut self.records, &mut index, &key);
            record.colos = sorted_counts(counts.remove(&key).unwrap_or_default());
            record.probed = probed.get(&key).copied().unwrap_or(ADDRESSES);
            record.updated = when.to_string();
        }
    }

    /// 零散的命中（手动扫描、粗测）：累加到对应段的机房计数上。
    pub fn learn(&mut self, hits: &[ScanHit], when: &str) {
        let mut index = self.index_of();
        for hit in hits {
            let (Some(segment), false) = (key_for_ip(&hit.ip), hit.colo.is_empty()) else {
                continue;
            };
            let record = Self::entry(&mut self.records, &mut index, &segment);
            let colo = hit.colo.to_ascii_uppercase();
            let mut counts: HashMap<String, u32> = record.colos.drain(..).collect();
            *counts.entry(colo).or_default() += 1;
            record.colos = sorted_counts(counts);
            record.updated = when.to_string();
        }
    }

    /// `pool` 中落在锁定区域的段（保持 `pool` 的顺序）。
    pub fn matching(&self, pool: &[String], lock: &RegionLock) -> Vec<String> {
        let matched: HashSet<&str> = self
            .records
            .iter()
            .filter(|record| record.colos.iter().any(|(colo, _)| lock.matches(colo)))
            .map(|record| record.segment.as_str())
            .collect();
        pool.iter()
            .filter(|segment| {
                normalize_segment(segment).is_some_and(|key| matched.contains(key.as_str()))
            })
            .cloned()
            .collect()
    }

    /// `pool` 里有划分记录（已探测或从扫描中学到）的段数，以及其中有响应的段数。
    pub fn coverage(&self, pool: &[String]) -> (usize, usize) {
        let index = self.index_of();
        let mut known = 0;
        let mut alive = 0;
        for segment in pool {
            if let Some(&position) = normalize_segment(segment).and_then(|key| index.get(&key)) {
                known += 1;
                if !self.records[position].colos.is_empty() {
                    alive += 1;
                }
            }
        }
        (known, alive)
    }

    /// 尚未划分的段（`force` 时为全部）。
    pub fn pending(&self, pool: &[String], force: bool) -> Vec<String> {
        if force {
            return pool.to_vec();
        }
        let index = self.index_of();
        pool.iter()
            .filter(|segment| {
                normalize_segment(segment).is_none_or(|key| {
                    index
                        .get(&key)
                        .is_none_or(|&position| self.records[position].probed == 0)
                })
            })
            .cloned()
            .collect()
    }

    /// `pool` 中各国家 / 地区的段数，按大区、段数排序。一个段落到多个国家时各计一次。
    pub fn countries(&self, pool: &[String]) -> Vec<CountryCount> {
        let index = self.index_of();
        let mut by_country: HashMap<String, (usize, HashMap<String, u32>)> = HashMap::new();
        for segment in pool {
            let Some(&position) = normalize_segment(segment).and_then(|key| index.get(&key)) else {
                continue;
            };
            let mut seen = HashSet::new();
            for (colo_code, hits) in &self.records[position].colos {
                let country = colo::country_code(colo_code);
                if country.is_empty() {
                    continue;
                }
                let entry = by_country.entry(country.to_string()).or_default();
                if seen.insert(country) {
                    entry.0 += 1;
                }
                *entry.1.entry(colo_code.clone()).or_default() += hits;
            }
        }
        let mut out: Vec<CountryCount> = by_country
            .into_iter()
            .map(|(code, (segments, colos))| CountryCount {
                name: colo::country_name(&code),
                region: colo::region_of_country(&code),
                code,
                segments,
                colos: sorted_counts(colos)
                    .into_iter()
                    .map(|(colo, _)| colo)
                    .collect(),
            })
            .collect();
        out.sort_by(|a, b| {
            colo::region_rank(a.region)
                .cmp(&colo::region_rank(b.region))
                .then(b.segments.cmp(&a.segments))
                .then(a.code.cmp(&b.code))
        });
        out
    }
}

fn sorted_counts(counts: HashMap<String, u32>) -> Vec<(String, u32)> {
    let mut list: Vec<(String, u32)> = counts.into_iter().collect();
    list.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    list
}

/// 段的统一写法 `a.b.c.0/24`；不是 /24 段时为 None。
pub fn normalize_segment(segment: &str) -> Option<String> {
    let spec = CidrSpec::parse(segment).ok()?;
    if spec.prefix != 24 {
        return None;
    }
    let [a, b, c, _] = Ipv4Addr::from(spec.network).octets();
    Some(format!("{a}.{b}.{c}.0/24"))
}

#[derive(Clone, Debug)]
pub struct ClassifyConfig {
    pub segments: Vec<String>,
    pub concurrency: usize,
    pub timeout: Duration,
}

#[derive(Clone, Debug)]
pub struct ClassifyProgress {
    /// 已探测的地址数。
    pub probed: usize,
    /// 最多还要探测的地址数加上已探测的（划分完成的段不再计入，所以会逐渐变小）。
    pub total: usize,
    /// 已划分出机房的段数。
    pub resolved: usize,
    pub segments: usize,
}

#[derive(Clone, Debug, Default)]
pub struct ClassifyOutcome {
    /// 参与划分的段（统一写法）。
    pub segments: Vec<String>,
    pub probed: HashMap<String, u32>,
    pub hits: Vec<ScanHit>,
    /// 有响应（至少命中一个地址）的段数。
    pub responded: usize,
    pub stopped: bool,
    pub elapsed: Duration,
}

/// 分散的探测顺序：.1 – .254 的一个固定排列，先粗后细地覆盖整个段。
fn offset_at(round: u32) -> u8 {
    (1 + (round * 97) % ADDRESSES) as u8
}

/// 按「轮」推进：每轮给每个尚未划分完成的段探测一个地址。存活地址多的段几轮就完成，
/// 稀疏的段继续往下探，整段都没有响应的段最多探测 254 个地址。
struct Cursor {
    round: u32,
    next: usize,
}

impl Cursor {
    fn next(&mut self, done: &[bool]) -> Option<(usize, u8)> {
        while self.round < ADDRESSES {
            if self.next >= done.len() {
                self.next = 0;
                self.round += 1;
                continue;
            }
            let index = self.next;
            self.next += 1;
            if !done[index] {
                return Some((index, offset_at(self.round)));
            }
        }
        None
    }
}

/// 探测每个段落到的机房；可随时停止，已探测的结果照常返回。
pub async fn classify<F>(
    config: &ClassifyConfig,
    cancel: Arc<AtomicBool>,
    mut emit: F,
) -> ClassifyOutcome
where
    F: FnMut(ClassifyProgress) + Send,
{
    let started = Instant::now();
    let mut seen = HashSet::new();
    let segments: Vec<[u8; 3]> = config
        .segments
        .iter()
        .filter_map(|segment| normalize_segment(segment))
        .filter(|segment| seen.insert(segment.clone()))
        .filter_map(|segment| {
            let spec = CidrSpec::parse(&segment).ok()?;
            let [a, b, c, _] = Ipv4Addr::from(spec.network).octets();
            Some([a, b, c])
        })
        .collect();
    let count = segments.len();
    let mut hits_per = vec![0u32; count];
    let mut probed_per = vec![0u32; count];
    let mut done = vec![false; count];
    let mut hits = Vec::new();
    let mut probed = 0usize;
    let mut total = count * ADDRESSES as usize;
    let mut resolved = 0usize;
    let mut cursor = Cursor { round: 0, next: 0 };
    let mut tasks = JoinSet::new();
    let wait = config.timeout;
    let mut last_emit = Instant::now() - Duration::from_secs(1);
    let mut stopped = false;

    let spawn = |tasks: &mut JoinSet<_>, index: usize, octets: [u8; 3], offset: u8| {
        let ip = Ipv4Addr::new(octets[0], octets[1], octets[2], offset).to_string();
        tasks.spawn(async move {
            let result = timeout(wait, http::probe_trace(&ip, 443, wait)).await;
            (index, ip, result.ok().and_then(Result::ok))
        });
    };
    for _ in 0..config.concurrency.clamp(1, 2000) {
        let Some((index, offset)) = cursor.next(&done) else {
            break;
        };
        probed_per[index] += 1;
        spawn(&mut tasks, index, segments[index], offset);
    }

    while let Some(joined) = tasks.join_next().await {
        probed += 1;
        if let Ok((index, ip, Some((colo_code, ms)))) = joined {
            hits.push(ScanHit::new(ip, 443, colo_code, ms));
            hits_per[index] += 1;
            if !done[index] && hits_per[index] >= ENOUGH_HITS {
                done[index] = true;
                resolved += 1;
                total = total.saturating_sub((ADDRESSES - probed_per[index]) as usize);
            }
        }
        if cancel.load(Ordering::Relaxed) {
            stopped = true;
            tasks.abort_all();
            break;
        }
        if let Some((index, offset)) = cursor.next(&done) {
            probed_per[index] += 1;
            spawn(&mut tasks, index, segments[index], offset);
        }
        if last_emit.elapsed() >= Duration::from_millis(200) {
            last_emit = Instant::now();
            emit(ClassifyProgress {
                probed,
                total: total.max(probed),
                resolved,
                segments: count,
            });
        }
    }
    while tasks.join_next().await.is_some() {}

    let keys: Vec<String> = segments
        .iter()
        .map(|[a, b, c]| format!("{a}.{b}.{c}.0/24"))
        .collect();
    let mut outcome = ClassifyOutcome {
        probed: keys
            .iter()
            .zip(&probed_per)
            .filter(|(_, count)| **count > 0)
            .map(|(key, count)| (key.clone(), *count))
            .collect(),
        segments: keys,
        hits,
        responded: hits_per.iter().filter(|count| **count > 0).count(),
        stopped,
        elapsed: started.elapsed(),
    };
    if stopped {
        // 停止时只保留真正探测过的段，没轮到的段下次继续。
        outcome
            .segments
            .retain(|segment| outcome.probed.contains_key(segment));
    }
    emit(ClassifyProgress {
        probed,
        total: probed,
        resolved,
        segments: count,
    });
    outcome
}

/// 把一次划分的结果写进记录。停止时没探测完的段也按已探测的地址数记下，下次不会重复。
pub fn apply_classification(regions: &mut SegmentRegions, outcome: &ClassifyOutcome, when: &str) {
    regions.record_scan(&outcome.segments, &outcome.probed, &outcome.hits, when);
    if !outcome.stopped {
        regions.classified = when.to_string();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(ip: &str, colo: &str) -> ScanHit {
        ScanHit::new(ip.into(), 443, colo.into(), 50.0)
    }

    #[test]
    fn lock_parses_countries_and_colos() {
        let lock = RegionLock::parse_text("hk，JP、sjc  HK x 1234 東京");
        assert_eq!(lock.countries(), ["HK", "JP"]);
        assert_eq!(lock.colos(), ["SJC"]);
        assert_eq!(lock.key(), "HK,JP,SJC");
        assert_eq!(lock.label(), "香港、日本、SJC");
        assert!(lock.matches("hkg"));
        assert!(lock.matches("NRT"));
        assert!(lock.matches("SJC"));
        assert!(!lock.matches("LAX"));
        assert!(!lock.matches(""));
        assert!(RegionLock::default().matches("LAX"));
        assert_eq!(RegionLock::default().label(), "不限");
    }

    #[test]
    fn scans_replace_and_learning_accumulates() {
        let mut regions = SegmentRegions::default();
        let segments = vec!["1.1.1.0/24".to_string(), "2.2.2.0/24".to_string()];
        regions.record_scan(
            &segments,
            &HashMap::new(),
            &[
                hit("1.1.1.5", "HKG"),
                hit("1.1.1.9", "HKG"),
                hit("9.9.9.9", "LAX"),
            ],
            "d1",
        );
        assert_eq!(regions.records.len(), 2);
        assert_eq!(regions.records[0].colos, vec![("HKG".to_string(), 2)]);
        assert!(regions.records[1].colos.is_empty());
        assert_eq!(regions.records[1].probed, ADDRESSES);
        assert_eq!(regions.coverage(&segments), (2, 1));

        regions.learn(&[hit("2.2.2.7", "NRT"), hit("2.2.2.8", "nrt")], "d2");
        assert_eq!(regions.records[1].colos, vec![("NRT".to_string(), 2)]);

        let pool = vec![
            "2.2.2.0/24".to_string(),
            "1.1.1.0/24".to_string(),
            "3.3.3.0/24".to_string(),
        ];
        let lock = RegionLock::parse(&["HK", "JP"]);
        assert_eq!(regions.matching(&pool, &lock), ["2.2.2.0/24", "1.1.1.0/24"]);
        assert_eq!(
            regions.matching(&pool, &RegionLock::parse(&["US"])),
            Vec::<String>::new()
        );
        assert_eq!(regions.pending(&pool, false), ["3.3.3.0/24"]);
        assert_eq!(regions.pending(&pool, true).len(), 3);

        let countries = regions.countries(&pool);
        let codes: Vec<&str> = countries.iter().map(|c| c.code.as_str()).collect();
        assert_eq!(codes, ["HK", "JP"]);
        assert_eq!(countries[0].colos, ["HKG"]);

        // 重新完整扫描时以新结果为准。
        regions.record_scan(
            &segments[..1],
            &HashMap::new(),
            &[hit("1.1.1.5", "SIN")],
            "d3",
        );
        assert_eq!(regions.records[0].colos, vec![("SIN".to_string(), 1)]);
    }

    #[test]
    fn cursor_skips_finished_segments_and_covers_every_address() {
        let mut cursor = Cursor { round: 0, next: 0 };
        let mut done = vec![false, false];
        assert_eq!(cursor.next(&done), Some((0, 1)));
        assert_eq!(cursor.next(&done), Some((1, 1)));
        done[0] = true;
        assert_eq!(cursor.next(&done).map(|(index, _)| index), Some(1));
        let mut offsets: Vec<u8> = (0..ADDRESSES).map(offset_at).collect();
        offsets.sort_unstable();
        offsets.dedup();
        assert_eq!(offsets.len(), ADDRESSES as usize);
        assert_eq!((offsets[0], offsets[253]), (1, 254));
    }

    #[test]
    fn segments_normalize_to_slash_24() {
        assert_eq!(
            normalize_segment("64.186.224.7/24"),
            Some("64.186.224.0/24".into())
        );
        assert_eq!(normalize_segment("64.186.224.0/20"), None);
    }

    #[tokio::test]
    async fn classify_marks_unreachable_segments() {
        let config = ClassifyConfig {
            // 保留地址，不会有 Cloudflare 响应。
            segments: vec!["192.0.2.0/24".into()],
            concurrency: 300,
            timeout: Duration::from_millis(30),
        };
        let outcome = classify(&config, Arc::new(AtomicBool::new(false)), |_| {}).await;
        assert_eq!(outcome.responded, 0);
        assert_eq!(outcome.probed.get("192.0.2.0/24"), Some(&ADDRESSES));
        let mut regions = SegmentRegions::default();
        apply_classification(&mut regions, &outcome, "now");
        assert_eq!(regions.classified, "now");
        assert!(regions.records[0].colos.is_empty());
        assert_eq!(regions.coverage(&config.segments), (1, 0));
    }
}
