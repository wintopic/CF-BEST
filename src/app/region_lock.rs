//! 出口区域：在自动优选页锁定国家 / 地区，在段池页按本机网络划分段池的出口区域。
//! 划分与筛选逻辑见 `core::region`。

use gpui::{ClickEvent, Context, FontWeight, Window, div, prelude::*};

use super::*;
use crate::core::colo;
use crate::core::region::{ClassifyConfig, RegionLock};
use crate::ui::widgets::{self, ButtonKind, Tone};

impl AppState {
    /// 设置里保存的锁定，加上输入框里尚未保存的其他代码。
    fn current_region_lock(&self, cx: &Context<Self>) -> RegionLock {
        let mut entries = self.settings.region_lock.clone();
        let known: Vec<&str> = self
            .region_countries
            .iter()
            .map(|country| country.code.as_str())
            .collect();
        entries.retain(|code| known.contains(&code.as_str()));
        entries.extend(RegionLock::parse_text(self.region_extra.read(cx).content()).entries());
        RegionLock::parse(&entries)
    }

    /// 开始优选前把输入框里的其他代码并入设置。
    pub(super) fn commit_region_lock(&mut self, cx: &mut Context<Self>) {
        let entries = self.current_region_lock(cx).entries();
        if entries != self.settings.region_lock {
            self.settings.region_lock = entries;
            self.save_settings();
        }
    }

    /// 下一轮优选实际轮转的段与进度：锁定区域时为落在该区域的段及其单独的游标、轮次。
    /// 返回 (段, 游标, 轮次, 是否按锁定区域)。还没有划分记录时与运行时一致：按全部段池预览，
    /// 运行时会先划分。
    pub(super) fn effective_rotation(&self) -> (Vec<String>, usize, u32, bool) {
        let lock = RegionLock::parse(&self.settings.region_lock);
        if lock.is_empty() || self.segment_regions.coverage(&self.pool_order).0 == 0 {
            return (
                self.pool_order.clone(),
                self.daily_state.cursor,
                self.daily_state.round.max(1),
                false,
            );
        }
        let order = self.segment_regions.matching(&self.pool_order, &lock);
        let (cursor, round) = if self.daily_state.region_key == lock.key() {
            (
                self.daily_state.region_cursor,
                self.daily_state.region_round,
            )
        } else {
            (0, 1)
        };
        (order, cursor, round.max(1), true)
    }

    /// 扫描结果中当前筛选显示的行（`scan_hits` 的下标）。
    pub(super) fn scan_visible(&self) -> Vec<usize> {
        self.scan_hits
            .iter()
            .enumerate()
            .filter(|(_, hit)| {
                self.scan_region
                    .as_ref()
                    .is_none_or(|code| colo::country_code(&hit.colo) == code)
            })
            .map(|(index, _)| index)
            .collect()
    }

    /// 扫描结果上方的国家 / 地区筛选标签；只有一个国家时不显示。
    pub(super) fn render_scan_region_filter(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let mut counts: Vec<(String, usize)> = Vec::new();
        for hit in &self.scan_hits {
            let code = colo::country_code(&hit.colo).to_string();
            if code.is_empty() {
                continue;
            }
            match counts.iter_mut().find(|(known, _)| *known == code) {
                Some((_, count)) => *count += 1,
                None => counts.push((code, 1)),
            }
        }
        if counts.len() < 2 && self.scan_region.is_none() {
            return None;
        }
        counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let mut chips = vec![self.region_chip(
            "scan-region-all",
            format!("全部 {}", self.scan_hits.len()),
            None,
            self.scan_region.is_none(),
            |this, _| this.scan_region = None,
            cx,
        )];
        for (index, (code, count)) in counts.into_iter().enumerate() {
            let active = self.scan_region.as_deref() == Some(code.as_str());
            chips.push(self.region_chip(
                ("scan-region", index),
                format!("{} {count}", colo::country_name(&code)),
                None,
                active,
                move |this, _| {
                    this.scan_region = (!active).then(|| code.clone());
                },
                cx,
            ));
        }
        Some(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .child(widgets::hint("按出口区域筛选："))
                .children(chips)
                .into_any_element(),
        )
    }

    /// 段池或划分记录变化后重新统计各国家 / 地区的段数。
    pub(super) fn refresh_regions(&mut self) {
        self.segment_regions = storage::load_segment_regions(&self.paths);
        self.region_countries = self.segment_regions.countries(&self.pool_segments);
    }

    fn toggle_region(&mut self, code: &str, cx: &mut Context<Self>) {
        let mut entries = self.settings.region_lock.clone();
        if let Some(index) = entries.iter().position(|entry| entry == code) {
            entries.remove(index);
        } else {
            entries.push(code.to_string());
        }
        self.settings.region_lock = RegionLock::parse(&entries).entries();
        self.save_settings();
        let lock = RegionLock::parse(&self.settings.region_lock);
        self.notify_status(format!("出口区域：{}", lock.label()), Tone::Accent, cx);
    }

    fn clear_region_lock(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.settings.region_lock.clear();
        self.save_settings();
        self.region_extra.update(cx, |input, cx| {
            input.set_content(String::new());
            cx.notify();
        });
        self.notify_status("出口区域：不限", Tone::Muted, cx);
    }

    fn classify_new(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        if let Err(err) = self.start_classify(false, cx) {
            self.fail(err, cx);
        }
    }

    fn classify_all(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        if let Err(err) = self.start_classify(true, cx) {
            self.fail(err, cx);
        }
    }

    fn start_classify(&mut self, force: bool, cx: &mut Context<Self>) -> Result<(), String> {
        if self.busy() {
            return Err("已有任务在运行，请等待完成或先停止".into());
        }
        let pool = storage::load_pool(&self.paths);
        if pool.is_empty() {
            return Err("段池为空：先运行一次自动优选，或点击「重建段池」".into());
        }
        self.pool_segments = pool.clone();
        self.refresh_regions();
        let segments = self.segment_regions.pending(&pool, force);
        if segments.is_empty() {
            return Err("段池里的段都已划分过；网络环境变了可以点「重新划分全部」".into());
        }
        self.classify_progress = ProgressState {
            done: 0,
            total: segments.len() * 254,
            label: format!("准备划分 {} 段", segments.len()),
        };
        self.classify_detail = self.classify_progress.label.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(cancel.clone());
        self.job = Some(JobKind::Classify);
        let config = ClassifyConfig {
            segments: segments.clone(),
            concurrency: self.settings.daily_concurrency.clamp(1, 1000) as usize,
            timeout: crate::core::daily::classify_timeout(Duration::from_secs_f64(
                self.settings.scan_timeout_secs.clamp(0.5, 30.0),
            )),
        };
        crate::jobs::spawn_classify(config, self.events_tx.clone(), cancel);
        self.notify_status(
            format!(
                "开始划分出口区域：{} 段{}",
                segments.len(),
                if force {
                    "（全部重新划分）"
                } else {
                    ""
                }
            ),
            Tone::Accent,
            cx,
        );
        Ok(())
    }

    /// 区域标签：国家 / 地区名 + 段数，点击切换锁定。
    fn region_chip(
        &self,
        id: impl Into<gpui::ElementId>,
        label: String,
        count: Option<usize>,
        active: bool,
        on_click: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        div()
            .id(id)
            .flex()
            .items_center()
            .gap_1()
            .h(px(28.))
            .px_3()
            .rounded(px(14.))
            .border_1()
            .border_color(if active {
                theme::accent()
            } else {
                theme::border()
            })
            .bg(if active {
                theme::accent_dim()
            } else {
                theme::panel_alt()
            })
            .cursor_pointer()
            .hover(|this| this.border_color(theme::border_strong()))
            .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                on_click(this, cx);
                cx.notify();
            }))
            .child(
                div()
                    .text_size(px(theme::font::LABEL))
                    .font_weight(if active {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::NORMAL
                    })
                    .text_color(if active {
                        theme::accent_text()
                    } else {
                        theme::text()
                    })
                    .child(label),
            )
            .children(count.map(|count| {
                div()
                    .text_size(px(theme::font::SMALL))
                    .text_color(theme::faint())
                    .child(format!("{count} 段"))
            }))
            .into_any_element()
    }

    /// 自动优选页的「出口区域」：可选的国家 / 地区来自段池的划分结果。
    pub(super) fn render_region_lock(&self, cx: &mut Context<Self>) -> gpui::Div {
        let lock = RegionLock::parse(&self.settings.region_lock);
        let unrestricted = self.settings.region_lock.is_empty()
            && self.region_extra.read(cx).content().trim().is_empty();
        let mut chips = vec![self.region_chip(
            "region-any",
            "不限".to_string(),
            None,
            unrestricted,
            |this, cx| {
                this.settings.region_lock.clear();
                this.save_settings();
                this.region_extra.update(cx, |input, cx| {
                    input.set_content(String::new());
                    cx.notify();
                });
            },
            cx,
        )];
        for (index, country) in self.region_countries.iter().enumerate() {
            let code = country.code.clone();
            chips.push(self.region_chip(
                ("region-chip", index),
                country.name.clone(),
                Some(country.segments),
                lock.countries().contains(&country.code),
                move |this, cx| this.toggle_region(&code, cx),
                cx,
            ));
        }
        let hint = if self.segment_regions.records.is_empty() {
            "还没有划分段池的出口区域：首次锁定时会自动划分，也可以先在「段池」页手动划分。"
                .to_string()
        } else if self.region_countries.is_empty() {
            "段池里已划分的段都没有响应，可在「段池」页重新划分。".to_string()
        } else if lock.is_empty() {
            "选择国家 / 地区后，优选只扫描落在这些区域的段，结果也只保留这些区域的 IP。".to_string()
        } else {
            let matched = self
                .segment_regions
                .matching(&self.pool_segments, &lock)
                .len();
            format!(
                "已锁定 {}：段池 {} 段中 {matched} 段落在该区域，优选只在这些段中轮转。",
                lock.label(),
                self.pool_segments.len()
            )
        };
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(theme::font::LABEL))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme::text())
                            .child("出口区域"),
                    )
                    .when(!lock.is_empty(), |row| {
                        row.child(
                            widgets::button("region-clear", "不限", "close", ButtonKind::Ghost)
                                .on_click(cx.listener(Self::clear_region_lock)),
                        )
                    }),
            )
            .child(div().flex().flex_wrap().gap_2().children(chips))
            .child(
                widgets::field(
                    "其他区域（国家 / 地区代码或机房代码，逗号分隔，如 SG, NRT, SJC）",
                    self.region_extra.clone(),
                )
                .w_full(),
            )
            .child(widgets::hint(hint))
    }

    /// 段池页的「出口区域」卡片：划分进度与各国家 / 地区的段数。
    pub(super) fn render_region_card(&self, cx: &mut Context<Self>) -> gpui::Div {
        let running = self.job == Some(JobKind::Classify);
        let busy = self.busy();
        let (known, alive) = self.segment_regions.coverage(&self.pool_segments);
        let pool = self.pool_segments.len();
        let lock = RegionLock::parse(&self.settings.region_lock);
        let status = if running {
            widgets::pill("划分中", Tone::Accent)
        } else if pool > 0 && known >= pool {
            widgets::pill("已划分", Tone::Green)
        } else if known > 0 {
            widgets::pill("部分划分", Tone::Amber)
        } else {
            widgets::pill("未划分", Tone::Muted)
        };
        let max = self
            .region_countries
            .iter()
            .map(|country| country.segments)
            .max()
            .unwrap_or(1)
            .max(1);
        let rows = self.region_countries.iter().map(|country| {
            let locked = lock.countries().contains(&country.code);
            div()
                .flex()
                .items_center()
                .gap_3()
                .child(
                    div()
                        .w(px(120.))
                        .flex_none()
                        .truncate()
                        .text_size(px(theme::font::LABEL))
                        .font_weight(if locked {
                            FontWeight::SEMIBOLD
                        } else {
                            FontWeight::NORMAL
                        })
                        .text_color(if locked {
                            theme::accent_text()
                        } else {
                            theme::text()
                        })
                        .child(format!(
                            "{} · {}",
                            colo::region_name(country.region),
                            country.name
                        )),
                )
                .child(
                    div()
                        .flex_1()
                        .h(px(6.))
                        .rounded(px(3.))
                        .bg(theme::panel_alt())
                        .child(
                            div()
                                .h_full()
                                .rounded(px(3.))
                                .bg(theme::accent())
                                .w(gpui::relative(country.segments as f32 / max as f32)),
                        ),
                )
                .child(
                    div()
                        .w(px(200.))
                        .flex_none()
                        .truncate()
                        .font_family(theme::FONT_MONO)
                        .text_size(px(theme::font::SMALL))
                        .text_color(theme::muted())
                        .child(format!(
                            "{} 段 · {}",
                            country.segments,
                            country
                                .colos
                                .iter()
                                .take(4)
                                .cloned()
                                .collect::<Vec<_>>()
                                .join(" ")
                        )),
                )
        });
        widgets::card()
            .w_full()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(widgets::section_title("globe", "出口区域"))
                    .child(status),
            )
            .child(widgets::hint(
                "Cloudflare 的地址是任播的，同一个段从不同网络连过去会落到不同机房；这里按本机网络探测每个段实际落到的机房，划分国家 / 地区。Workers / Pages 节点的出口就在这个机房。",
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        widgets::button_with_state(
                            "classify-new",
                            "划分新段",
                            "globe",
                            ButtonKind::Primary,
                            busy,
                        )
                        .on_click(cx.listener(Self::classify_new)),
                    )
                    .child(
                        widgets::button_with_state(
                            "classify-all",
                            "重新划分全部",
                            "refresh",
                            ButtonKind::Ghost,
                            busy,
                        )
                        .on_click(cx.listener(Self::classify_all)),
                    )
                    .when(running, |row| {
                        row.child(
                            widgets::button("stop-classify", "停止", "stop", ButtonKind::Danger)
                                .on_click(cx.listener(Self::stop_job)),
                        )
                    })
                    .child(widgets::hint(
                        "整段探测，段多时需要几分钟；可随时停止，已探测的段会保留。",
                    )),
            )
            .when(running || !self.classify_detail.is_empty(), |card| {
                card.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .truncate()
                                .text_size(px(theme::font::LABEL))
                                .text_color(theme::muted())
                                .child(self.classify_detail.clone()),
                        )
                        .child(widgets::progress_bar(
                            "classify-progress",
                            self.classify_progress.fraction(),
                            running,
                        )),
                )
            })
            .child(
                div()
                    .flex()
                    .gap_5()
                    .flex_wrap()
                    .child(widgets::stat("段池", pool.to_string(), Tone::Muted))
                    .child(widgets::stat(
                        "已划分",
                        format!("{known}/{pool}"),
                        Tone::Blue,
                    ))
                    .child(widgets::stat("有响应", alive.to_string(), Tone::Green))
                    .child(widgets::stat(
                        "国家 / 地区",
                        self.region_countries.len().to_string(),
                        Tone::Accent,
                    ))
                    .child(widgets::stat(
                        "上次划分",
                        if self.segment_regions.classified.is_empty() {
                            "—".to_string()
                        } else {
                            self.segment_regions.classified.clone()
                        },
                        Tone::Muted,
                    )),
            )
            .child(div().flex().flex_col().gap_2().children(rows))
    }
}
