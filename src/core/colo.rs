//! 机房（Cloudflare 数据中心）信息：城市、国家 / 地区与大区。数据见 [`colo_data`](crate::core::colo_data)。

use crate::core::colo_data::{CITY_NAMES, COLOS, COUNTRY_NAMES, REGION_NAMES};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColoInfo {
    pub iata: &'static str,
    /// 国家 / 地区代码，如 `HK`、`US`。
    pub country: &'static str,
    /// 大区英文名，如 `Asia Pacific`。
    pub region: &'static str,
    pub city_en: &'static str,
}

pub fn lookup(colo: &str) -> Option<ColoInfo> {
    let colo = colo.trim().to_ascii_uppercase();
    let index = COLOS
        .binary_search_by(|(iata, ..)| (*iata).cmp(colo.as_str()))
        .ok()?;
    let (iata, country, region, city_en) = COLOS[index];
    Some(ColoInfo {
        iata,
        country,
        region,
        city_en,
    })
}

/// 机房所在国家 / 地区代码；不认识的机房为空串。
pub fn country_code(colo: &str) -> &'static str {
    lookup(colo).map_or("", |info| info.country)
}

/// 国家 / 地区的中文名；没有中文名时返回代码本身。
pub fn country_name(code: &str) -> String {
    let code = code.trim().to_ascii_uppercase();
    COUNTRY_NAMES
        .iter()
        .find(|(cc, _)| *cc == code)
        .map_or(code, |(_, name)| (*name).to_string())
}

/// 大区的中文名，如「亚太」。
pub fn region_name(region: &str) -> &str {
    REGION_NAMES
        .iter()
        .find(|(en, _)| *en == region)
        .map_or(region, |(_, zh)| *zh)
}

/// 大区在界面上的排序（亚太、北美、欧洲……），不认识的排最后。
pub fn region_rank(region: &str) -> usize {
    REGION_NAMES
        .iter()
        .position(|(en, _)| *en == region)
        .unwrap_or(REGION_NAMES.len())
}

/// 国家 / 地区所属的大区（取它第一个机房的大区）。
pub fn region_of_country(code: &str) -> &'static str {
    let code = code.trim().to_ascii_uppercase();
    COLOS
        .iter()
        .find(|(_, cc, ..)| *cc == code)
        .map_or("", |(_, _, region, _)| *region)
}

/// 机房城市：常见机房用中文名，其余用英文名；不认识的机房为空串。
pub fn city(colo: &str) -> &'static str {
    let colo = colo.trim().to_ascii_uppercase();
    CITY_NAMES
        .iter()
        .find(|(iata, _)| *iata == colo)
        .map(|(_, city)| *city)
        .or_else(|| lookup(&colo).map(|info| info.city_en))
        .unwrap_or("")
}

/// 城市与国家 / 地区，如「洛杉矶 · 美国」；城市与地区同名时只写一次（「香港」）。
pub fn place(colo: &str) -> String {
    let Some(info) = lookup(colo) else {
        return String::new();
    };
    let city = city(colo);
    let country = country_name(info.country);
    if city == country {
        country
    } else {
        format!("{city} · {country}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_sorted_and_unique_for_binary_search() {
        assert!(COLOS.windows(2).all(|pair| pair[0].0 < pair[1].0));
        assert!(COLOS.len() > 300);
    }

    #[test]
    fn lookups_cover_common_colos() {
        assert_eq!(country_code("hkg"), "HK");
        assert_eq!(country_code(" LAX "), "US");
        assert_eq!(country_code("XXX"), "");
        assert_eq!(country_name("JP"), "日本");
        assert_eq!(country_name("zz"), "ZZ");
        assert_eq!(region_name(lookup("NRT").unwrap().region), "亚太");
        assert_eq!(region_of_country("US"), "North America");
        assert_eq!(place("LAX"), "洛杉矶 · 美国");
        assert_eq!(place("HKG"), "香港");
        assert_eq!(place("SIN"), "新加坡");
        assert_eq!(place("ABQ"), "Albuquerque · 美国");
        assert_eq!(place(""), "");
        assert!(region_rank("Asia Pacific") < region_rank("Africa"));
    }
}
