use std::collections::HashSet;
use std::net::Ipv4Addr;

use rand::Rng;
use rand::seq::index;

pub const MAX_TARGETS: usize = 1_000_000;
pub const MIN_PER_RANGE: usize = 8;
pub const BAD_PREFIXES: &[&str] = &["104.26.", "172.67."];

pub const FALLBACK_RANGES: &[&str] = &[
    "173.245.48.0/20",
    "103.21.244.0/22",
    "103.22.200.0/22",
    "103.31.4.0/22",
    "141.101.64.0/18",
    "108.162.192.0/18",
    "190.93.240.0/20",
    "188.114.96.0/20",
    "197.234.240.0/22",
    "198.41.128.0/17",
    "162.158.0.0/15",
    "104.16.0.0/13",
    "104.24.0.0/14",
    "172.64.0.0/13",
    "131.0.72.0/22",
];

/// 机房所在国家 / 地区代码；完整机房表见 `core::colo`。
pub fn country_for_colo(colo: &str) -> &'static str {
    crate::core::colo::country_code(colo)
}

pub fn colo_city(colo: &str) -> &'static str {
    crate::core::colo::city(colo)
}

pub fn colo_label(colo: &str) -> String {
    let colo = colo.trim().to_ascii_uppercase();
    let city = colo_city(&colo);
    match (colo.is_empty(), city.is_empty()) {
        (true, _) => "?".to_string(),
        (false, true) => colo,
        (false, false) => format!("{colo} · {city}"),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CidrSpec {
    pub network: u32,
    pub prefix: u8,
}

impl CidrSpec {
    pub fn parse(token: &str) -> Result<Self, String> {
        let token = token.trim();
        let (addr_part, prefix_part) = match token.split_once('/') {
            Some((addr, prefix)) => (addr, Some(prefix)),
            None => (token, None),
        };
        let ip = Ipv4Addr::from_str_compat(addr_part)
            .ok_or_else(|| format!("无法解析 IPv4 地址：{token}"))?;
        let prefix = match prefix_part {
            Some(prefix) => prefix
                .parse::<u8>()
                .ok()
                .filter(|prefix| *prefix <= 32)
                .ok_or_else(|| format!("前缀长度无效：{token}"))?,
            None => 32,
        };
        let mask = prefix_mask(prefix);
        Ok(Self {
            network: u32::from(ip) & mask,
            prefix,
        })
    }

    pub fn host_range(&self) -> (u32, u32) {
        let broadcast = self.network | !prefix_mask(self.prefix);
        match self.prefix {
            32 => (self.network, self.network),
            31 => (self.network, self.network + 1),
            _ => (self.network.saturating_add(1), broadcast.saturating_sub(1)),
        }
    }

    pub fn host_count(&self) -> u64 {
        let (start, end) = self.host_range();
        u64::from(end) - u64::from(start) + 1
    }

    pub fn address_count(&self) -> u64 {
        1u64 << (32 - self.prefix as u32)
    }
}

impl std::fmt::Display for CidrSpec {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{}/{}",
            Ipv4Addr::from(self.network),
            self.prefix
        )
    }
}

trait Ipv4ParseExt {
    fn from_str_compat(value: &str) -> Option<Ipv4Addr>;
}

impl Ipv4ParseExt for Ipv4Addr {
    fn from_str_compat(value: &str) -> Option<Ipv4Addr> {
        value.trim().parse::<Ipv4Addr>().ok()
    }
}

fn prefix_mask(prefix: u8) -> u32 {
    if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix as u32)
    }
}

/// Parse a free-form list of IPv4 addresses / CIDRs (and `ip:port`, which is
/// tolerated for compatibility with ips.txt). Comments start with `#`.
pub fn parse_specs(input: &str) -> Result<Vec<CidrSpec>, String> {
    let mut specs = Vec::new();
    let mut seen = HashSet::new();

    for (line_ix, raw_line) in input.lines().enumerate() {
        let line = raw_line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let normalized = line.replace(['，', '；'], ",");
        for token in normalized
            .split(|c: char| c == ',' || c == ';' || c.is_whitespace())
            .map(str::trim)
            .filter(|token| !token.is_empty())
        {
            let spec = if token.contains('/') {
                CidrSpec::parse(token).map_err(|err| format!("第 {} 行：{err}", line_ix + 1))?
            } else if token.matches(':').count() == 1 {
                let (addr, port) = token.rsplit_once(':').unwrap();
                if port.parse::<u16>().is_err() {
                    return Err(format!("第 {} 行端口无效：{token}", line_ix + 1));
                }
                CidrSpec::parse(addr).map_err(|err| format!("第 {} 行：{err}", line_ix + 1))?
            } else {
                CidrSpec::parse(token).map_err(|err| format!("第 {} 行：{err}", line_ix + 1))?
            };
            if seen.insert((spec.network, spec.prefix)) {
                specs.push(spec);
            }
        }
    }

    if specs.is_empty() {
        return Err("没有解析到任何 IPv4 地址或 CIDR".to_string());
    }
    Ok(specs)
}

pub fn parse_ports(input: &str) -> Result<Vec<u16>, String> {
    let mut ports = Vec::new();
    for token in input
        .replace(['，', '；'], ",")
        .split(|c: char| c == ',' || c == ';' || c.is_whitespace())
        .map(str::trim)
        .filter(|token| !token.is_empty())
    {
        let port = token
            .parse::<u16>()
            .ok()
            .filter(|port| *port > 0)
            .ok_or_else(|| format!("端口无效：{token}"))?;
        if !ports.contains(&port) {
            ports.push(port);
        }
    }
    if ports.is_empty() {
        return Err("至少需要一个端口".to_string());
    }
    Ok(ports)
}

pub fn effective_count(spec: &CidrSpec, sample: f64) -> u64 {
    let count = spec.host_count();
    if sample > 0.0 && sample < 1.0 && count > 50 {
        ((count as f64 * sample).round() as u64).max(1)
    } else {
        count
    }
}

pub fn expand_specs(specs: &[CidrSpec], sample: f64, max: usize) -> Result<Vec<Ipv4Addr>, String> {
    let sample = sample.clamp(0.0, 1.0);
    let expected: u64 = specs.iter().map(|spec| effective_count(spec, sample)).sum();
    if expected > max as u64 {
        return Err(format!(
            "目标数量 {expected} 超过上限 {max}；请拆分 CIDR 或提高抽样比例（当前 {:.0}%）",
            sample * 100.0
        ));
    }

    let mut rng = rand::rng();
    let mut ips = Vec::with_capacity(expected as usize);
    let mut seen = HashSet::with_capacity(expected as usize);

    for spec in specs {
        let count = spec.host_count();
        if count == 0 {
            continue;
        }
        let (start, _) = spec.host_range();
        let take = effective_count(spec, sample) as usize;
        let count_usize = count as usize;

        let mut push = |offset: usize| {
            let value = u64::from(start) + offset as u64;
            if value <= u64::from(u32::MAX) && seen.insert(value as u32) {
                ips.push(Ipv4Addr::from(value as u32));
            }
        };

        if take >= count_usize {
            for offset in 0..count_usize {
                push(offset);
            }
        } else {
            let mut picks = index::sample(&mut rng, count_usize, take).into_vec();
            picks.sort_unstable();
            for offset in picks {
                push(offset);
            }
        }
    }
    Ok(ips)
}

pub fn parse_targets(
    input: &str,
    ports: &[u16],
    sample: f64,
    max: usize,
) -> Result<Vec<(Ipv4Addr, u16)>, String> {
    let specs = parse_specs(input)?;
    let expected: u64 = specs.iter().map(|spec| effective_count(spec, sample)).sum();
    let pairs = expected.saturating_mul(ports.len() as u64);
    if pairs > max as u64 {
        return Err(format!(
            "探测次数 {pairs}（{} 个 IP × {} 个端口）超过上限 {max}",
            expected,
            ports.len()
        ));
    }
    let ips = expand_specs(&specs, sample, max)?;
    let mut targets = Vec::with_capacity(ips.len() * ports.len());
    for ip in ips {
        for port in ports {
            targets.push((ip, *port));
        }
    }
    Ok(targets)
}

/// Split announced prefixes into /24 (or narrower) segments, as used by the
/// daily workflow. Bounded to keep a pathological prefix list from exhausting memory.
pub fn split_to_24(cidrs: &[String], max_segments: usize) -> Result<Vec<String>, String> {
    let mut out = HashSet::new();
    for cidr in cidrs {
        let Ok(spec) = CidrSpec::parse(cidr) else {
            continue;
        };
        if spec.prefix > 24 {
            out.insert(spec.to_string());
            continue;
        }
        let segments = 1u64 << (24 - spec.prefix as u32);
        if out.len() as u64 + segments > max_segments as u64 {
            return Err(format!(
                "段池超过 {max_segments} 个 /24，请检查 ASN 前缀规模"
            ));
        }
        for i in 0..segments {
            let net = spec.network + ((i as u32) << 8);
            out.insert(format!("{}/24", Ipv4Addr::from(net)));
        }
    }
    let mut list: Vec<(u32, u8, String)> = out
        .into_iter()
        .filter_map(|item| {
            let spec = CidrSpec::parse(&item).ok()?;
            Some((spec.network, spec.prefix, item))
        })
        .collect();
    list.sort_by_key(|(network, prefix, _)| (*network, *prefix));
    Ok(list.into_iter().map(|(_, _, item)| item).collect())
}

pub fn allocate_quota(specs: &[CidrSpec], total: usize) -> Vec<usize> {
    let n = specs.len();
    if n == 0 {
        return Vec::new();
    }
    let min_per = MIN_PER_RANGE.min(total / n);
    let mut quota = vec![min_per; n];
    let remaining = total.saturating_sub(min_per * n);
    if remaining == 0 {
        return quota;
    }
    let weights: Vec<f64> = specs
        .iter()
        .map(|spec| spec.address_count() as f64)
        .collect();
    let total_weight: f64 = weights.iter().sum();
    let mut leftover = remaining;
    let mut fractions = Vec::with_capacity(n);
    for (ix, weight) in weights.iter().enumerate() {
        let raw = remaining as f64 * weight / total_weight.max(1.0);
        let base = raw.floor() as usize;
        quota[ix] += base;
        leftover -= base;
        fractions.push((ix, raw - base as f64));
    }
    fractions.sort_by(|a, b| b.1.total_cmp(&a.1));
    for (ix, _) in fractions.into_iter().take(leftover) {
        quota[ix] += 1;
    }
    quota
}

/// Build a random candidate pool from the official Cloudflare ranges. Every
/// range receives a minimum quota so small ranges are not crowded out.
pub fn build_official_pool(cidrs: &[String], total: usize) -> Result<Vec<String>, String> {
    let specs: Vec<CidrSpec> = cidrs
        .iter()
        .filter_map(|cidr| CidrSpec::parse(cidr).ok())
        .filter(|spec| spec.address_count() > 2)
        .collect();
    if specs.is_empty() {
        return Err("官方 IP 段为空".to_string());
    }
    let quota = allocate_quota(&specs, total);
    let mut rng = rand::rng();
    let mut out = HashSet::new();
    for (spec, want) in specs.iter().zip(quota) {
        let (start, end) = spec.host_range();
        if end <= start {
            continue;
        }
        for _ in 0..want {
            let value = rng.random_range(start..=end);
            let ip = Ipv4Addr::from(value);
            let text = ip.to_string();
            if BAD_PREFIXES.iter().any(|prefix| text.starts_with(prefix)) {
                continue;
            }
            out.insert((value, text));
        }
    }
    if out.is_empty() {
        return Err("没有生成可用的候选 IP".to_string());
    }
    let mut list: Vec<(u32, String)> = out.into_iter().collect();
    list.sort_by_key(|(value, _)| *value);
    Ok(list.into_iter().map(|(_, text)| text).collect())
}

/// Parse a list of IPv4 endpoints. Each token may be `ip` or `ip:port`.
/// Used by the speed and tunnel workflows, which operate on explicit IPs.
pub fn parse_ip_targets(input: &str, default_port: u16) -> Result<Vec<(String, u16)>, String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for (line_ix, raw_line) in input.lines().enumerate() {
        let line = raw_line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        for token in line
            .replace(['，', '；'], ",")
            .split(|c: char| c == ',' || c == ';' || c.is_whitespace())
            .map(str::trim)
            .filter(|token| !token.is_empty())
        {
            let (addr_text, port) = match token.rsplit_once(':') {
                Some((addr, port)) if !addr.contains(':') => {
                    let port = port
                        .parse::<u16>()
                        .ok()
                        .filter(|port| *port > 0)
                        .ok_or_else(|| format!("第 {} 行端口无效：{token}", line_ix + 1))?;
                    (addr, port)
                }
                _ => (token, default_port),
            };
            let ip = addr_text
                .parse::<Ipv4Addr>()
                .map_err(|_| format!("第 {} 行不是有效 IPv4：{addr_text}", line_ix + 1))?;
            if seen.insert((u32::from(ip), port)) {
                out.push((ip.to_string(), port));
            }
        }
    }
    if out.is_empty() {
        return Err("没有解析到任何 IP".to_string());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_ip_and_cidr() {
        let specs =
            parse_specs("1.2.3.4\n10.0.0.0/30 # comment\n192.168.1.0/31, 8.8.8.8:443").unwrap();
        assert_eq!(specs.len(), 4);
        assert_eq!(specs[0].to_string(), "1.2.3.4/32");
        assert_eq!(specs[1].to_string(), "10.0.0.0/30");
    }

    #[test]
    fn expands_hosts_like_python() {
        let specs = parse_specs("10.0.0.0/30\n192.168.1.0/31\n1.2.3.4").unwrap();
        let ips = expand_specs(&specs, 0.0, MAX_TARGETS).unwrap();
        let texts: Vec<String> = ips.iter().map(|ip| ip.to_string()).collect();
        assert_eq!(
            texts,
            vec![
                "10.0.0.1",
                "10.0.0.2",
                "192.168.1.0",
                "192.168.1.1",
                "1.2.3.4"
            ]
        );
    }

    #[test]
    fn samples_large_networks() {
        let specs = parse_specs("10.0.0.0/16").unwrap();
        let ips = expand_specs(&specs, 0.01, MAX_TARGETS).unwrap();
        assert_eq!(ips.len(), 655);
        let unique: HashSet<Ipv4Addr> = ips.iter().copied().collect();
        assert_eq!(unique.len(), ips.len());
    }

    #[test]
    fn rejects_oversized_targets() {
        let specs = parse_specs("10.0.0.0/8").unwrap();
        let err = expand_specs(&specs, 0.0, 1000).unwrap_err();
        assert!(err.contains("超过上限"));
    }

    #[test]
    fn parses_ip_targets_with_ports() {
        let list = parse_ip_targets(
            "1.2.3.4
5.6.7.8:8443 # x
1.2.3.4",
            443,
        )
        .unwrap();
        assert_eq!(
            list,
            vec![("1.2.3.4".to_string(), 443), ("5.6.7.8".to_string(), 8443),]
        );
    }

    #[test]
    fn parses_ports() {
        assert_eq!(
            parse_ports("443, 8443 2053").unwrap(),
            vec![443, 8443, 2053]
        );
        assert!(parse_ports("0").is_err());
    }

    #[test]
    fn splits_prefixes_to_24() {
        let list = split_to_24(
            &["10.0.0.0/23".to_string(), "192.0.2.0/24".to_string()],
            100,
        )
        .unwrap();
        assert_eq!(list, vec!["10.0.0.0/24", "10.0.1.0/24", "192.0.2.0/24"]);
    }

    #[test]
    fn quota_has_minimum_for_each_range() {
        let specs: Vec<CidrSpec> = FALLBACK_RANGES
            .iter()
            .filter_map(|cidr| CidrSpec::parse(cidr).ok())
            .collect();
        let quota = allocate_quota(&specs, 400);
        assert_eq!(quota.iter().sum::<usize>(), 400);
        assert!(quota.iter().all(|amount| *amount >= MIN_PER_RANGE));
    }

    #[test]
    fn official_pool_respects_size_and_bad_prefixes() {
        let cidrs: Vec<String> = FALLBACK_RANGES.iter().map(|s| s.to_string()).collect();
        let pool = build_official_pool(&cidrs, 120).unwrap();
        assert!(pool.len() <= 120);
        assert!(
            !pool
                .iter()
                .any(|ip| BAD_PREFIXES.iter().any(|bad| ip.starts_with(bad)))
        );
    }
}
