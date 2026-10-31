use std::ops::Range;

use gpui::{ClickEvent, Context, FontWeight, div, prelude::*};

use crate::theme::px;

use super::AppState;
use crate::core::model::SegmentScore;
use crate::core::pool;
use crate::jobs::JobKind;
use crate::theme;
use crate::ui::widgets::{self, ButtonKind, Tone};

impl AppState {
    fn strategy_card(&self, cx: &mut Context<Self>, strategy: &'static str) -> gpui::AnyElement {
        let active = self.pool_strategy() == strategy;
        let has_scores = self
            .segment_scores
            .records
            .iter()
            .any(|record| record.score > 0.0);
        let icon = match strategy {
            "sequential" => "refresh",
            "network" => "wifi",
            _ => "layers",
        };
        let badge = if active {
            widgets::pill("使用中", Tone::Accent)
        } else {
            match strategy {
                "sequential" => widgets::pill("全覆盖", Tone::Blue),
                "network" => widgets::pill(
                    if has_scores { "已粗测" } else { "待粗测" },
                    if has_scores { Tone::Green } else { Tone::Muted },
                ),
                _ => widgets::pill("默认推荐", Tone::Accent),
            }
        };
        div()
            .id(strategy)
            .flex()
            .flex_col()
            .gap_1p5()
            .flex_1()
            .p_3()
            .rounded(px(theme::radius::XL))
            .bg(if active {
                theme::accent_dim()
            } else {
                theme::panel_alt()
            })
            .border_1()
            .border_color(if active {
                theme::accent()
            } else {
                theme::border()
            })
            .cursor_pointer()
            .hover(|this| this.border_color(theme::border_strong()))
            .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                this.apply_pool_strategy(strategy, cx);
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .size(px(24.))
                                    .rounded(px(7.))
                                    .bg(if active {
                                        theme::accent()
                                    } else {
                                        theme::panel()
                                    })
                                    .child(widgets::icon(
                                        icon,
                                        14.0,
                                        if active {
                                            theme::on_accent()
                                        } else {
                                            theme::muted()
                                        },
                                    )),
                            )
                            .child(
                                div()
                                    .text_size(px(theme::font::SUBTITLE))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(if active {
                                        theme::accent_text()
                                    } else {
                                        theme::text()
                                    })
                                    .child(pool::strategy_label(strategy)),
                            ),
                    )
                    .child(badge),
            )
            .child(
                div()
                    .text_size(px(theme::font::SMALL))
                    .line_height(px(19.))
                    .text_color(theme::muted())
                    .child(pool::strategy_detail(strategy)),
            )
            .into_any_element()
    }

    pub fn render_pool_page(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        if !self.settings.advanced_mode {
            return widgets::card()
                .w_full()
                .child(widgets::section_title("layers", "段池优化"))
                .child(widgets::hint(
                    "段池策略、起点与游标、本机网络粗测都在高级模式中。",
                ))
                .child(
                    widgets::button(
                        "pool-enable-advanced",
                        "开启高级模式",
                        "sliders",
                        ButtonKind::Soft,
                    )
                    .on_click(cx.listener(Self::toggle_advanced)),
                );
        }

        let pool_len = self.pool_segments.len();
        let order_len = self.pool_order.len();
        let cursor = pool::normalize_cursor(self.daily_state.cursor, order_len);
        let batch = pool::batch_size(self.settings.daily_segments, order_len);
        let preview = self.next_batch_preview(false);
        let (locked_order, locked_cursor, _, locked) = self.effective_rotation();
        let lock_note = locked.then(|| {
            format!(
                "已锁定出口区域 {}：自动优选在落在该区域的 {} 段中单独轮转（下一轮从其中第 {} 段开始）；这里的起点与预览只用于不锁定时。",
                crate::core::region::RegionLock::parse(&self.settings.region_lock).label(),
                locked_order.len(),
                pool::normalize_cursor(locked_cursor, locked_order.len()) + 1
            )
        });
        let strategy = self.pool_strategy();
        let busy = self.busy();
        let probe_running = self.job == Some(JobKind::Probe);
        let probe_progress = self.probe_progress.clone();
        let segment_scores = self.segment_scores.clone();
        let top_records: Vec<SegmentScore> = segment_scores
            .records
            .iter()
            .filter(|record| record.hits > 0)
            .take(12)
            .cloned()
            .collect();
        let alive = segment_scores
            .records
            .iter()
            .filter(|record| record.hits > 0)
            .count();
        let measured = segment_scores
            .records
            .iter()
            .filter(|record| record.mbps.is_some())
            .count();
        let best = segment_scores
            .records
            .iter()
            .filter(|record| record.hits > 0)
            .max_by(|a, b| a.score.total_cmp(&b.score))
            .cloned();
        let probe_time = if segment_scores.time.is_empty() {
            "—".to_string()
        } else {
            segment_scores.time.clone()
        };

        let status_pill = if probe_running {
            widgets::pill("粗测中", Tone::Accent)
        } else if self.probe_stage == "失败" {
            widgets::pill("失败", Tone::Red)
        } else if segment_scores.time.is_empty() {
            widgets::pill("未粗测", Tone::Muted)
        } else {
            widgets::pill("已有结果", Tone::Green)
        };

        let overview = widgets::card()
            .w_full()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(widgets::section_title("layers", "段池与分配"))
                    .child(widgets::pill(
                        format!("{} 段", pool_len),
                        if pool_len == 0 {
                            Tone::Muted
                        } else {
                            Tone::Blue
                        },
                    )),
            )
            .child(widgets::hint(
                "每日优选按当前策略的段池顺序，从起点开始连续取 N 段；每完成一轮，起点后移 N 段，到达末尾后回到第 1 段，进入下一轮。",
            ))
            .child(
                div()
                    .flex()
                    .gap_3()
                    .children(
                        pool::STRATEGIES
                            .iter()
                            .map(|strategy| self.strategy_card(cx, strategy)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap_5()
                    .flex_wrap()
                    .child(widgets::stat(
                        "段池",
                        pool_len.to_string(),
                        Tone::Blue,
                    ))
                    .child(widgets::stat(
                        "当前起点",
                        if order_len == 0 {
                            "—".to_string()
                        } else {
                            format!("第 {} 段", cursor + 1)
                        },
                        Tone::Accent,
                    ))
                    .child(widgets::stat(
                        "每轮段数",
                        if batch == 0 {
                            "—".to_string()
                        } else {
                            batch.to_string()
                        },
                        Tone::Muted,
                    ))
                    .child(widgets::stat(
                        "轮次",
                        self.daily_state.round.max(1).to_string(),
                        Tone::Amber,
                    ))
                    .child(widgets::stat(
                        "策略",
                        pool::strategy_label(strategy),
                        Tone::Green,
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_3()
                    .rounded(px(theme::radius::LG))
                    .bg(theme::panel_alt())
                    .border_1()
                    .border_color(theme::border())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .child(widgets::icon("chevron-right", 13.0, theme::faint()))
                            .child(
                                div()
                                    .text_size(px(theme::font::LABEL))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(theme::text())
                                    .child("下一批预览"),
                            ),
                    )
                    .child(
                        div()
                            .font_family(theme::FONT_MONO)
                            .text_size(px(theme::font::BODY))
                            .line_height(px(21.))
                            .text_color(theme::muted())
                            .child(preview),
                    ),
            )
            .children(lock_note.map(widgets::hint))
            .child(
                div()
                    .flex()
                    .items_end()
                    .gap_3()
                    .child(widgets::field("起始段序号（从 1 开始）", self.pool_cursor.clone()))
                    .child(widgets::field(
                        "每轮段数 N",
                        self.daily_segments.clone(),
                    ))
                    .child(
                        widgets::button_with_state("pool-apply-cursor", "应用起点", "zap", ButtonKind::Soft, busy)
                            .on_click(cx.listener(Self::apply_pool_cursor)),
                    )
                    .child(
                        widgets::button_with_state("pool-reset-recommended", "恢复推荐起点", "refresh", ButtonKind::Ghost, busy)
                            .on_click(cx.listener(Self::reset_pool_recommended)),
                    )
                    .child(div().flex_1())
                    .child(
                        widgets::button_with_state("pool-run-daily", "用此配置运行", "play", ButtonKind::Primary, busy)
                            .on_click(cx.listener(Self::apply_pool_cursor_and_run)),
                    ),
            )
            .child(widgets::hint(
                "「恢复推荐起点」会切回均衡轮转策略，并从通用推荐段 64.186.224.0/24 开始；修改参数后请先点击「应用起点」。",
            ));

        let probe = widgets::card()
            .w_full()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(widgets::section_title("wifi", "本机网络粗测"))
                    .child(status_pill),
            )
            .child(widgets::hint(
                "从每个段均匀抽取少量 IP 做 TLS 握手，再对握手最快的若干段做短时下载测速。结果用于「网络优选」排序，也可随时把表现好的段设为起点。",
            ))
            .child(
                div()
                    .flex()
                    .gap_3()
                    .child(widgets::field("每段抽样 IP 数", self.probe_samples.clone()))
                    .child(widgets::field("并发", self.probe_concurrency.clone()))
                    .child(widgets::field("测速段数", self.probe_top.clone()))
                    .child(widgets::field("每段测速秒数", self.probe_seconds.clone())),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        widgets::button_with_state("start-probe", "开始粗测", "wifi", ButtonKind::Primary, busy)
                            .on_click(cx.listener(Self::start_probe)),
                    )
                    .when(probe_running, |row| {
                        row.child(
                            widgets::button("stop-probe", "停止粗测", "stop", ButtonKind::Danger)
                                .on_click(cx.listener(Self::stop_job)),
                        )
                    })
                    .child(widgets::hint(
                        "粗测只建立短连接，不使用订阅信息；建议在网络空闲时运行。",
                    )),
            )
            .child(
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
                                    .truncate()
                                    .text_size(px(theme::font::LABEL))
                                    .text_color(theme::muted())
                                    .child(if self.probe_detail.is_empty() {
                                        "尚未开始".to_string()
                                    } else {
                                        self.probe_detail.clone()
                                    }),
                            )
                            .child(
                                div()
                                    .font_family(theme::FONT_MONO)
                                    .text_size(px(theme::font::SMALL))
                                    .text_color(theme::faint())
                                    .child(format!(
                                        "{:.0}%",
                                        probe_progress.fraction() * 100.0
                                    )),
                            ),
                    )
                    .child(widgets::progress_bar(
                        "pool-probe-progress",
                        probe_progress.fraction(),
                        probe_running,
                    )),
            )
            .child(
                div()
                    .flex()
                    .gap_5()
                    .flex_wrap()
                    .child(widgets::stat(
                        "可用段",
                        if segment_scores.records.is_empty() {
                            "—".to_string()
                        } else {
                            format!("{alive}/{}", segment_scores.records.len())
                        },
                        Tone::Green,
                    ))
                    .child(widgets::stat("已测速", measured.to_string(), Tone::Blue))
                    .child(widgets::stat(
                        "最佳段",
                        best.as_ref()
                            .map(|record| record.segment.clone())
                            .unwrap_or_else(|| "—".to_string()),
                        Tone::Accent,
                    ))
                    .child(widgets::stat(
                        "最高速度",
                        best.as_ref()
                            .and_then(|record| record.mbps)
                            .map(|mbps| format!("{mbps:.1} Mbps"))
                            .unwrap_or_else(|| "—".to_string()),
                        Tone::Amber,
                    ))
                    .child(widgets::stat("上次粗测", probe_time, Tone::Muted)),
            )
            .child(self.probe_table(cx, &top_records));

        let columns: [(&'static str, f32); 6] = [
            ("#", 52.),
            ("IP 段", 180.),
            ("/16 组", 90.),
            ("粗测得分", 100.),
            ("状态", 160.),
            ("操作", 100.),
        ];
        let order_len = self.pool_order.len();
        let order_body_height = (order_len as f32 * 36.0).clamp(120.0, 420.0);
        let order_body = if order_len == 0 {
            widgets::empty_state(
                "archive",
                "段池为空",
                "先在「自动优选」运行一次，或点击「重建段池」",
            )
            .h(px(220.))
            .into_any_element()
        } else {
            gpui::uniform_list(
                "pool-order-rows",
                order_len,
                cx.processor(
                    |this: &mut AppState,
                     range: Range<usize>,
                     _window,
                     cx: &mut Context<AppState>| {
                        let len = this.pool_order.len();
                        if len == 0 {
                            return Vec::new();
                        }
                        let cursor = pool::normalize_cursor(this.daily_state.cursor, len);
                        let batch = pool::batch_size(this.settings.daily_segments, len);
                        let mut rows = Vec::with_capacity(range.end - range.start);
                        for ix in range {
                            let Some(segment) = this.pool_order.get(ix).cloned() else {
                                continue;
                            };
                            let in_batch = (ix + len - cursor) % len < batch;
                            let score = this.score_map.get(&segment).copied().unwrap_or(0.0);
                            let probed = this.score_map.contains_key(&segment);
                            let score_label = if score > 0.0 {
                                format!("{score:.0}")
                            } else {
                                "—".to_string()
                            };
                            let score_color = if score > 0.0 {
                                theme::text()
                            } else {
                                theme::faint()
                            };
                            rows.push(
                                div()
                                    .flex()
                                    .w_full()
                                    .items_center()
                                    .h(px(42.))
                                    .px_4()
                                    .bg(if ix == cursor {
                                        theme::accent_soft()
                                    } else if in_batch {
                                        theme::accent_dim()
                                    } else if ix % 2 == 0 {
                                        theme::row_even()
                                    } else {
                                        theme::panel()
                                    })
                                    .border_b_1()
                                    .border_color(theme::border())
                                    .hover(|style| style.bg(theme::row_hover()))
                                    .child(
                                        div()
                                            .w(px(52.))
                                            .flex_shrink_0()
                                            .font_family(theme::FONT_MONO)
                                            .text_size(px(theme::font::SMALL))
                                            .text_color(theme::faint())
                                            .child(format!("{}", ix + 1)),
                                    )
                                    .child(
                                        div()
                                            .w(px(180.))
                                            .flex_shrink_0()
                                            .font_family(theme::FONT_MONO)
                                            .text_size(px(theme::font::BODY_LG))
                                            .text_color(theme::text())
                                            .child(segment.clone()),
                                    )
                                    .child(
                                        div()
                                            .w(px(90.))
                                            .flex_shrink_0()
                                            .font_family(theme::FONT_MONO)
                                            .text_size(px(theme::font::LABEL))
                                            .text_color(theme::muted())
                                            .child(pool::group_key(&segment)),
                                    )
                                    .child(
                                        div()
                                            .w(px(100.))
                                            .flex_shrink_0()
                                            .font_family(theme::FONT_MONO)
                                            .text_size(px(theme::font::BODY))
                                            .text_color(score_color)
                                            .child(score_label),
                                    )
                                    .child(
                                        div()
                                            .w(px(160.))
                                            .flex_shrink_0()
                                            .flex()
                                            .items_center()
                                            .gap_1p5()
                                            .when(ix == cursor, |cell| {
                                                cell.child(widgets::pill("起点", Tone::Accent))
                                            })
                                            .when(ix != cursor && in_batch, |cell| {
                                                cell.child(widgets::pill("本轮", Tone::Blue))
                                            })
                                            .when(!in_batch && probed, |cell| {
                                                cell.child(widgets::pill(
                                                    if score > 0.0 { "已测" } else { "0 命中" },
                                                    if score > 0.0 {
                                                        Tone::Green
                                                    } else {
                                                        Tone::Muted
                                                    },
                                                ))
                                            })
                                            .when(!in_batch && !probed, |cell| {
                                                cell.child(widgets::pill("未测", Tone::Muted))
                                            }),
                                    )
                                    .child(
                                        div()
                                            .w(px(100.))
                                            .flex_shrink_0()
                                            .flex()
                                            .justify_start()
                                            .child(
                                                div()
                                                    .id(ix + 4_000_000)
                                                    .flex()
                                                    .items_center()
                                                    .justify_start()
                                                    .h(px(24.))
                                                    .px_1p5()
                                                    .rounded(px(5.))
                                                    .text_size(px(theme::font::SMALL))
                                                    .text_color(theme::muted())
                                                    .cursor_pointer()
                                                    .hover(|style| {
                                                        style
                                                            .bg(theme::row_hover())
                                                            .text_color(theme::text())
                                                    })
                                                    .on_click(cx.listener(
                                                        move |this, _: &ClickEvent, _window, cx| {
                                                            this.set_pool_start_index(ix, cx);
                                                        },
                                                    ))
                                                    .child("设为起点"),
                                            ),
                                    ),
                            );
                        }
                        rows
                    },
                ),
            )
            .h(px(order_body_height))
            .w_full()
            .into_any_element()
        };

        let order_card = widgets::card()
            .w_full()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(widgets::section_title("layers", "段池顺序"))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(widgets::pill(pool::strategy_label(strategy), Tone::Accent))
                            .child(widgets::pill(
                                format!("{order_len} 段"),
                                Tone::Muted,
                            )),
                    ),
            )
            .child(widgets::hint(
                "这里是每日优选实际使用的顺序。点击任意行的「设为起点」即可从该段开始，下一批会在表中高亮。",
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w_full()
                    .rounded(px(theme::radius::LG))
                    .overflow_hidden()
                    .child(widgets::table_header(&columns))
                    .child(order_body),
            );

        div()
            .flex()
            .flex_col()
            .gap_4()
            .w_full()
            .child(overview)
            .child(self.render_region_card(cx))
            .child(probe)
            .child(order_card)
    }

    fn probe_table(&self, cx: &mut Context<Self>, records: &[SegmentScore]) -> gpui::AnyElement {
        if records.is_empty() {
            return widgets::empty_state(
                "target",
                "还没有本机粗测结果",
                "点击「开始粗测」，先摸清各段在当前网络下的表现",
            )
            .h(px(160.))
            .into_any_element();
        }
        let columns: [(&'static str, f32); 8] = [
            ("#", 40.),
            ("IP 段", 160.),
            ("机房", 110.),
            ("命中/抽样", 90.),
            ("握手", 90.),
            ("速度", 110.),
            ("粗测得分", 90.),
            ("操作", 100.),
        ];
        let pool_order = self.pool_order.clone();
        let mut rows = Vec::with_capacity(records.len());
        for (rank, record) in records.iter().enumerate() {
            let index = pool::index_of(&pool_order, &record.segment);
            let score_color = if record.score >= 1000.0 {
                theme::accent_text()
            } else {
                theme::green()
            };
            let throughput = record
                .mbps
                .map(|mbps| format!("{mbps:.1} Mbps"))
                .unwrap_or_else(|| "—".to_string());
            let colo = if record.colo.is_empty() {
                "—".to_string()
            } else {
                crate::core::ip::colo_label(&record.colo)
            };
            rows.push(
                div()
                    .flex()
                    .w_full()
                    .items_center()
                    .h(px(42.))
                    .px_4()
                    .bg(if rank % 2 == 0 {
                        theme::row_even()
                    } else {
                        theme::panel()
                    })
                    .border_b_1()
                    .border_color(theme::border())
                    .child(
                        div()
                            .w(px(40.))
                            .flex_shrink_0()
                            .font_family(theme::FONT_MONO)
                            .text_size(px(theme::font::SMALL))
                            .text_color(theme::faint())
                            .child(format!("{}", rank + 1)),
                    )
                    .child(
                        div()
                            .w(px(160.))
                            .flex_shrink_0()
                            .font_family(theme::FONT_MONO)
                            .text_size(px(theme::font::BODY_LG))
                            .text_color(theme::text())
                            .child(record.segment.clone()),
                    )
                    .child(
                        div()
                            .w(px(110.))
                            .flex_shrink_0()
                            .text_size(px(theme::font::LABEL))
                            .text_color(theme::green())
                            .child(colo),
                    )
                    .child(
                        div()
                            .w(px(90.))
                            .flex_shrink_0()
                            .font_family(theme::FONT_MONO)
                            .text_size(px(theme::font::LABEL))
                            .text_color(theme::muted())
                            .child(format!("{}/{}", record.hits, record.samples)),
                    )
                    .child(
                        div()
                            .w(px(90.))
                            .flex_shrink_0()
                            .font_family(theme::FONT_MONO)
                            .text_size(px(theme::font::LABEL))
                            .text_color(theme::muted())
                            .child(if record.hits == 0 {
                                "—".to_string()
                            } else {
                                format!("{:.0} ms", record.avg_latency_ms)
                            }),
                    )
                    .child(
                        div()
                            .w(px(110.))
                            .flex_shrink_0()
                            .font_family(theme::FONT_MONO)
                            .text_size(px(theme::font::BODY))
                            .text_color(if record.mbps.is_some() {
                                theme::text()
                            } else {
                                theme::faint()
                            })
                            .child(throughput),
                    )
                    .child(
                        div()
                            .w(px(90.))
                            .flex_shrink_0()
                            .font_family(theme::FONT_MONO)
                            .text_size(px(theme::font::BODY))
                            .text_color(score_color)
                            .child(format!("{:.0}", record.score)),
                    )
                    .child(
                        div()
                            .w(px(100.))
                            .flex_shrink_0()
                            .flex()
                            .justify_start()
                            .children(index.map(|index| {
                                div()
                                    .id(rank + 5_000_000)
                                    .flex()
                                    .items_center()
                                    .justify_start()
                                    .h(px(24.))
                                    .px_1p5()
                                    .rounded(px(5.))
                                    .text_size(px(theme::font::SMALL))
                                    .text_color(theme::muted())
                                    .cursor_pointer()
                                    .hover(|style| {
                                        style.bg(theme::row_hover()).text_color(theme::text())
                                    })
                                    .on_click(cx.listener(
                                        move |this, _: &ClickEvent, _window, cx| {
                                            this.set_pool_start_index(index, cx);
                                        },
                                    ))
                                    .child("设为起点")
                            })),
                    ),
            );
        }
        div()
            .flex()
            .flex_col()
            .w_full()
            .rounded(px(theme::radius::LG))
            .overflow_hidden()
            .child(widgets::table_header(&columns))
            .children(rows)
            .into_any_element()
    }
}
