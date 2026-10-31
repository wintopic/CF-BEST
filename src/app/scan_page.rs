use std::ops::Range;

use gpui::{Context, div, prelude::*};

use crate::theme::px;

use super::AppState;
use super::columns::TableId;
use crate::core::ip;
use crate::core::model::ParamGroup;
use crate::jobs::JobKind;
use crate::theme;
use crate::ui::widgets::{self, ButtonKind, Tone};

fn latency_tone(ms: f64) -> Tone {
    if ms < 120.0 {
        Tone::Green
    } else if ms < 300.0 {
        Tone::Amber
    } else {
        Tone::Muted
    }
}

const SCAN_PRESETS: [&str; 4] = [
    "64.186.224.0/24",
    "104.16.0.0/24",
    "172.64.0.0/24",
    "162.158.0.0/24",
];

impl AppState {
    pub fn render_scan_page(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        let advanced = self.settings.advanced_mode;
        let running = self.job == Some(JobKind::Scan);
        let busy = self.busy();
        let hits = self.scan_hits.len();
        let progress = self.scan_progress.clone();
        let known_total = progress.total;
        let hit_rate = if known_total == 0 {
            0.0
        } else {
            hits as f64 * 100.0 / known_total as f64
        };
        let status = if running {
            ("扫描中", Tone::Accent)
        } else if progress.done > 0 {
            ("已完成", Tone::Green)
        } else {
            ("就绪", Tone::Muted)
        };

        let advanced_fields = advanced.then(|| {
            div()
                .flex()
                .flex_col()
                .gap_3()
                .p_3()
                .rounded(px(theme::radius::LG))
                .bg(theme::panel_alt())
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(widgets::icon("sliders", 14.0, theme::faint()))
                        .child(
                            div()
                                .text_size(px(theme::font::LABEL))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(theme::muted())
                                .child("扫描参数"),
                        )
                        .child(div().flex_1())
                        .children(self.restore_params_button(ParamGroup::Scan, cx)),
                )
                .child(
                    div()
                        .flex()
                        .gap_3()
                        .child(widgets::field("端口", self.scan_ports.clone()))
                        .child(widgets::field("并发", self.scan_concurrency.clone()))
                        .child(widgets::field("超时（秒）", self.scan_timeout.clone()))
                        .child(widgets::field("抽样比例（%）", self.scan_sample.clone())),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_end()
                        .gap_3()
                        .child(
                            div()
                                .w(px(320.))
                                .child(widgets::field("从 ASN 获取 IP 段", self.scan_asn.clone())),
                        )
                        .child(
                            widgets::button_with_state(
                                "fetch-asn",
                                "获取 IP 段",
                                "globe",
                                ButtonKind::Secondary,
                                busy,
                            )
                            .on_click(cx.listener(Self::fetch_asn)),
                        )
                        .child(div().flex_1())
                        .child(widgets::hint("支持 AS906 / AS13335，多个用逗号分隔")),
                )
                .child(widgets::hint(
                    "抽样比例为 0 表示扫描全部 IP；并发建议 200–300，超时建议 3–8 秒。",
                ))
        });

        let progress_text = if running {
            progress.label.clone()
        } else if progress.done > 0 {
            format!(
                "命中 {hits} 个 · 已扫描 {} / {}",
                progress.done, progress.total
            )
        } else {
            "尚未开始扫描".to_string()
        };

        let preset_row = (!advanced).then(|| {
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .justify_end()
                .gap_2()
                .h(px(30.))
                .px_3()
                .flex_shrink_0()
                .child(widgets::icon("sparkles", 11.0, theme::faint()))
                .child(
                    div()
                        .text_size(px(theme::font::SMALL))
                        .text_color(theme::faint())
                        .child("推荐段"),
                )
                .children(SCAN_PRESETS.iter().map(|segment| {
                    div()
                        .id(*segment)
                        .flex()
                        .items_center()
                        .justify_center()
                        .h(px(24.))
                        .px_1p5()
                        .rounded(px(theme::radius::XS))
                        .font_family(theme::FONT_MONO)
                        .text_size(px(theme::font::SMALL))
                        .text_color(theme::muted())
                        .cursor_pointer()
                        .hover(|style| {
                            style
                                .bg(theme::row_hover())
                                .text_color(theme::accent_text())
                        })
                        .on_click(cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                            this.fill_scan_segment(segment, cx);
                        }))
                        .child(*segment)
                }))
        });

        let scan_field = div()
            .flex()
            .flex_col()
            .w_full()
            .h(px(210.))
            .rounded(px(theme::radius::LG))
            .bg(theme::input_bg())
            .border_1()
            .border_color(theme::border())
            .overflow_hidden()
            .on_scroll_wheel(|_, _: &mut gpui::Window, cx: &mut gpui::App| {
                cx.stop_propagation();
            })
            .child(self.scan_input.clone())
            .children(preset_row);

        let config_card = widgets::card()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(widgets::section_title("search", "扫描目标"))
                    .child(widgets::pill(status.0, status.1)),
            )
            .child(widgets::hint(if advanced {
                "支持 CIDR、单个 IP 与 IP:端口，每行一条。"
            } else {
                "每行一个 IP 段或单个 IP；不确定填什么时，可点击输入框右下角的「推荐段」。"
            }))
            .child(scan_field)
            .children(advanced_fields)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        widgets::button_with_state(
                            "start-scan",
                            "开始扫描",
                            "play",
                            ButtonKind::Primary,
                            busy,
                        )
                        .on_click(cx.listener(Self::start_scan)),
                    )
                    .when(!advanced && !running, |row| {
                        row.child(
                            widgets::button(
                                "scan-go-daily",
                                "一键自动优选",
                                "sparkles",
                                ButtonKind::Soft,
                            )
                            .on_click(cx.listener(
                                |this, event, window, cx| {
                                    this.run_daily_now(event, window, cx);
                                },
                            )),
                        )
                    })
                    .child(
                        div()
                            .text_size(px(theme::font::SMALL))
                            .text_color(theme::faint())
                            .child(if advanced {
                                "参数修改后立即生效"
                            } else {
                                if self.settings.params_recommended(ParamGroup::Scan) {
                                    "已使用推荐参数，无需额外设置"
                                } else {
                                    "使用高级模式中设置的参数"
                                }
                            }),
                    ),
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
                            .child(widgets::progress_caption(
                                progress_text,
                                running,
                                "scan-spinner",
                            ))
                            .child(
                                div()
                                    .font_family(theme::FONT_MONO)
                                    .text_size(px(theme::font::SMALL))
                                    .text_color(theme::faint())
                                    .child(format!("{:.0}%", progress.fraction() * 100.0)),
                            ),
                    )
                    .child(widgets::progress_bar(
                        "scan-progress",
                        progress.fraction(),
                        running,
                    )),
            )
            .when(advanced, |this| {
                this.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_5()
                        .pt_3()
                        .border_t_1()
                        .border_color(theme::border())
                        .child(widgets::stat(
                            "已完成",
                            format!("{} / {}", progress.done, progress.total),
                            Tone::Blue,
                        ))
                        .child(widgets::stat("命中", hits.to_string(), Tone::Green))
                        .child(widgets::stat(
                            "命中率",
                            format!("{hit_rate:.0}%"),
                            Tone::Amber,
                        ))
                        .child(div().flex_1())
                        .child(widgets::hint(if running {
                            "命中的地址会实时出现在下方列表。"
                        } else if hits > 0 {
                            "下一步：把结果送去测速，选出下载最快的地址。"
                        } else {
                            "没有命中时，可以换一个 IP 段再试。"
                        })),
                )
            })
            .when(!advanced && hits == 0 && !running, |this| {
                this.child(widgets::inline_guide([
                    "粘贴 IP 段",
                    "开始扫描",
                    "在结果列表中点击「下一步：测速」",
                ]))
            });

        let visible = self.scan_visible();
        let row_count = visible.len();
        let region_filter = self.render_scan_region_filter(cx);
        let table_body = if row_count == 0 {
            widgets::empty_state(
                "scan",
                if running {
                    "扫描进行中…"
                } else {
                    "还没有扫描结果"
                },
                if running {
                    "命中的地址会实时出现在这里"
                } else {
                    "粘贴 IP 段并点击「开始扫描」"
                },
            )
            .h(px(160.))
            .into_any_element()
        } else {
            gpui::uniform_list(
                "scan-rows",
                row_count,
                cx.processor(|this: &mut AppState, range: Range<usize>, _window, _cx| {
                    let widths = this.column_widths(TableId::Scan);
                    let mut rows = Vec::with_capacity(range.end - range.start);
                    let visible = this.scan_visible();
                    for ix in range {
                        let Some(hit) =
                            visible.get(ix).and_then(|&index| this.scan_hits.get(index))
                        else {
                            continue;
                        };
                        rows.push(
                            div()
                                .id(ix)
                                .flex()
                                .w_full()
                                .items_center()
                                .h(px(42.))
                                .px_4()
                                .bg(if ix % 2 == 0 {
                                    theme::row_even()
                                } else {
                                    theme::panel()
                                })
                                .border_b_1()
                                .border_color(theme::border())
                                .hover(|style| style.bg(theme::row_hover()))
                                .child(
                                    div()
                                        .w(px(widths[0]))
                                        .flex_shrink_0()
                                        .font_family(theme::FONT_MONO)
                                        .text_size(px(theme::font::BODY_LG))
                                        .text_color(theme::text())
                                        .child(hit.ip.clone()),
                                )
                                .child(
                                    div()
                                        .w(px(widths[1]))
                                        .flex_shrink_0()
                                        .font_family(theme::FONT_MONO)
                                        .text_size(px(theme::font::BODY))
                                        .text_color(theme::muted())
                                        .child(hit.port.to_string()),
                                )
                                .child(
                                    div()
                                        .w(px(widths[2]))
                                        .flex_shrink_0()
                                        .font_family(theme::FONT_MONO)
                                        .text_size(px(theme::font::BODY))
                                        .text_color(theme::green())
                                        .child(hit.colo.clone()),
                                )
                                .child(
                                    div()
                                        .w(px(widths[3]))
                                        .flex_shrink_0()
                                        .text_size(px(theme::font::BODY))
                                        .text_color(theme::muted())
                                        .child(if hit.country.is_empty() {
                                            "-".to_string()
                                        } else {
                                            crate::core::colo::country_name(&hit.country)
                                        }),
                                )
                                .child(
                                    div()
                                        .w(px(widths[4]))
                                        .flex_shrink_0()
                                        .text_size(px(theme::font::BODY))
                                        .text_color(theme::muted())
                                        .child({
                                            let city = ip::colo_city(&hit.colo);
                                            if city.is_empty() {
                                                "-".to_string()
                                            } else {
                                                city.to_string()
                                            }
                                        }),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .font_family(theme::FONT_MONO)
                                        .text_size(px(theme::font::BODY))
                                        .text_color(latency_tone(hit.latency_ms).color())
                                        .child(format!("{:.0} ms", hit.latency_ms)),
                                ),
                        );
                    }
                    rows
                }),
            )
            .h(px(320.))
            .w_full()
            .into_any_element()
        };

        let table_actions = (hits > 0).then(|| {
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    widgets::button("scan-to-speed", "下一步：测速", "gauge", ButtonKind::Soft)
                        .on_click(cx.listener(Self::next_to_speed)),
                )
                .child(
                    widgets::button("export-scan", "导出", "download", ButtonKind::Ghost)
                        .on_click(cx.listener(Self::export_scan)),
                )
        });

        let table_card = widgets::card()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(widgets::section_title("search", "扫描结果"))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(widgets::pill_action(
                                format!("{hits} 条"),
                                if hits > 0 { Tone::Green } else { Tone::Muted },
                            ))
                            .children(table_actions),
                    ),
            )
            .children(region_filter)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w_full()
                    .rounded(px(theme::radius::LG))
                    .overflow_hidden()
                    .child({
                        let widths = self.column_widths(TableId::Scan);
                        widgets::table_header_row()
                            .child(self.header_cell(
                                TableId::Scan,
                                0,
                                widths[0],
                                widgets::plain_header_cell("IP"),
                                cx,
                            ))
                            .child(self.header_cell(
                                TableId::Scan,
                                1,
                                widths[1],
                                widgets::plain_header_cell("端口"),
                                cx,
                            ))
                            .child(self.header_cell(
                                TableId::Scan,
                                2,
                                widths[2],
                                widgets::plain_header_cell("机房"),
                                cx,
                            ))
                            .child(self.header_cell(
                                TableId::Scan,
                                3,
                                widths[3],
                                widgets::plain_header_cell("国家"),
                                cx,
                            ))
                            .child(self.header_cell(
                                TableId::Scan,
                                4,
                                widths[4],
                                widgets::plain_header_cell("城市"),
                                cx,
                            ))
                            .child(div().flex_1().child(widgets::plain_header_cell("握手延迟")))
                    })
                    .child(table_body),
            );

        div()
            .flex()
            .flex_col()
            .gap_4()
            .w_full()
            .child(config_card)
            .child(table_card)
    }

    fn fill_scan_segment(&mut self, segment: &'static str, cx: &mut Context<Self>) {
        let current = self.scan_input.read(cx).content().trim().to_string();
        if current.lines().any(|line| line.trim() == segment) {
            self.notify_status(format!("{segment} 已在列表中"), Tone::Muted, cx);
            return;
        }
        let content = if current.is_empty() {
            segment.to_string()
        } else {
            format!("{current}\n{segment}")
        };
        self.scan_input
            .update(cx, |input, _| input.set_content(content));
        self.notify_status(
            format!("已填入 {segment}，点击「开始扫描」"),
            Tone::Green,
            cx,
        );
    }
}
