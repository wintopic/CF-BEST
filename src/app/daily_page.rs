use gpui::{ClickEvent, Context, FontWeight, div, prelude::*};

use crate::theme::px;

use super::{AppState, Page};
use crate::core::model::DailyVerification;
use crate::core::model::ParamGroup;
use crate::core::pool;
use crate::core::sub_server::FeedSource;
use crate::jobs::JobKind;
use crate::theme;
use crate::ui::tooltip::tooltip;
use crate::ui::widgets::{self, ButtonKind, Tone};

impl AppState {
    /// 复制本轮 Top N 的全部地址（IP:端口，每行一个），与点击单行复制的格式一致。
    fn copy_daily_top(&mut self, _: &ClickEvent, _: &mut gpui::Window, cx: &mut Context<Self>) {
        let lines = self
            .daily_top_rows
            .iter()
            .map(|entry| format!("{}:{}", entry.ip, entry.port))
            .collect::<Vec<_>>();
        let count = lines.len();
        self.copy_text(
            lines.join(
                "
",
            ),
            &format!(" {count} 个 IP"),
            cx,
        );
    }

    pub fn render_daily_page(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        let advanced = self.settings.advanced_mode;
        let running = self.job == Some(JobKind::Daily);
        let busy = self.busy();
        let progress = self.daily_progress.clone();
        let summary = self.daily_summary.clone();
        let verification_hint = if self
            .verify_nodes
            .iter()
            .any(|node| node.transport.eq_ignore_ascii_case("ws"))
        {
            "已加载 VLESS WebSocket 节点：测速后将通过节点进行隧道验证。"
        } else {
            "无需订阅即可优选 IP；测速后只做 TLS 基础验证，结果不代表代理节点可用。"
        };
        let strategy = pool::normalize_strategy(&self.settings.pool_strategy);
        let (rotation, cursor, round, locked) = self.effective_rotation();
        let cursor = pool::normalize_cursor(cursor, rotation.len());
        let cursor_label = if rotation.is_empty() {
            "起点 —".to_string()
        } else {
            format!("起点第 {} 段", cursor + 1)
        };
        let preview = self.next_batch_preview(true);
        let reset_armed = self
            .reset_armed_at
            .map(|at| at.elapsed() < std::time::Duration::from_secs(5))
            .unwrap_or(false);
        let auto = self.settings.auto_daily;
        let launch_at_login = self.settings.launch_at_login;
        let restore_params = self.restore_params_button(ParamGroup::Daily, cx);
        // 简洁模式与高级模式各显示一份（同一时间只渲染其中一张卡片）。
        let region_lock_simple = self.render_region_lock(cx);
        let region_lock_config = self.render_region_lock(cx);
        let status = if running {
            ("运行中", Tone::Accent)
        } else if self.daily_stage == "完成" || !self.daily_top_rows.is_empty() {
            ("就绪", Tone::Green)
        } else {
            ("待运行", Tone::Muted)
        };

        let auto_toggle = || {
            div()
                .flex()
                .items_center()
                .gap_3()
                .child(
                    div()
                        .id("toggle-auto")
                        .flex()
                        .items_center()
                        .gap_2()
                        .h(px(34.))
                        .cursor_pointer()
                        .on_click(cx.listener(Self::toggle_auto_daily))
                        .child(widgets::switch(auto))
                        .child(
                            div()
                                .text_size(px(theme::font::BODY_LG))
                                .text_color(theme::text())
                                .child("每天自动运行"),
                        ),
                )
                .child(div().w(px(96.)).child(self.daily_auto_time.clone()))
                .child(
                    div()
                        .text_size(px(theme::font::SMALL))
                        .text_color(theme::faint())
                        .child("HH:MM"),
                )
                .child(div().w(px(1.)).h(px(16.)).bg(theme::border()))
                .child(
                    div()
                        .id("toggle-launch-at-login")
                        .flex()
                        .items_center()
                        .gap_2()
                        .h(px(34.))
                        .cursor_pointer()
                        .tooltip(tooltip(
                            if crate::tray::SUPPORTED {
                                "登录系统时自动启动，并在托盘中待命"
                            } else {
                                "登录系统时自动启动"
                            },
                            None,
                        ))
                        .on_click(cx.listener(Self::toggle_launch_at_login))
                        .child(widgets::switch(launch_at_login))
                        .child(
                            div()
                                .text_size(px(theme::font::BODY_LG))
                                .text_color(theme::text())
                                .child("开机自启"),
                        ),
                )
        };
        // 定时运行需要应用一直开着，提示随托盘常驻与开机自启的状态变化。
        let tray_on = self.settings.tray_enabled && crate::tray::SUPPORTED;
        let run_hint = match (auto, tray_on, launch_at_login) {
            (false, _, _) => "开启后需保持应用运行",
            (true, false, _) if !crate::tray::SUPPORTED => "需保持应用运行，可最小化窗口",
            (true, true, true) => "开机后在托盘中待命，到点自动运行",
            (true, true, false) => "关闭窗口后在托盘继续运行",
            (true, false, _) => "需保持应用运行：托盘常驻已关闭，关闭窗口会退出",
        };

        let make_summary_stats = |summary: &Option<crate::core::model::DailySummary>,
                                  running: bool|
         -> Option<gpui::Div> {
            (summary.is_some() || running).then(|| {
                div()
                    .flex()
                    .gap_4()
                    .child(widgets::stat(
                        "扫描",
                        summary
                            .as_ref()
                            .map(|summary| summary.scanned.to_string())
                            .unwrap_or_else(|| "-".into()),
                        Tone::Blue,
                    ))
                    .child(widgets::stat(
                        "命中",
                        summary
                            .as_ref()
                            .map(|summary| summary.hits.to_string())
                            .unwrap_or_else(|| "-".into()),
                        Tone::Green,
                    ))
                    .child(widgets::stat(
                        "测速",
                        summary
                            .as_ref()
                            .map(|summary| summary.tested.to_string())
                            .unwrap_or_else(|| "-".into()),
                        Tone::Amber,
                    ))
                    .child(widgets::stat(
                        summary
                            .as_ref()
                            .map(|summary| summary.verification)
                            .unwrap_or_default()
                            .passed_label(),
                        summary
                            .as_ref()
                            .map(|summary| summary.verified.to_string())
                            .unwrap_or_else(|| "-".into()),
                        Tone::Accent,
                    ))
            })
        };

        let simple_card = widgets::card()
            .w_full()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(widgets::section_title("calendar", "自动优选"))
                    .child(widgets::pill(status.0, status.1)),
            )
            .child(widgets::hint(
                "每天自动收集一批可用 IP；也可以现在立即运行一次。",
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_4()
                    .child(
                        widgets::button_with_state(
                            "start-daily",
                            "立即运行一次",
                            "play",
                            ButtonKind::Primary,
                            busy,
                        )
                        .on_click(cx.listener(Self::start_daily)),
                    )
                    .child(widgets::hint(
                        if self.settings.params_recommended(ParamGroup::Daily) {
                            "使用推荐参数，自动完成扫描、测速与验证"
                        } else {
                            "使用高级模式中设置的参数，自动完成扫描、测速与验证"
                        },
                    )),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(auto_toggle())
                    .child(widgets::hint(run_hint)),
            )
            .child(widgets::hint(verification_hint))
            .child(region_lock_simple)
            .children(make_summary_stats(&summary, running))
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
                                    .child(if self.daily_detail.is_empty() {
                                        "尚未运行".to_string()
                                    } else {
                                        self.daily_detail.clone()
                                    }),
                            )
                            .child(
                                div()
                                    .font_family(theme::FONT_MONO)
                                    .text_size(px(theme::font::SMALL))
                                    .text_color(theme::faint())
                                    .child(format!("{:.0}%", progress.fraction() * 100.0)),
                            ),
                    )
                    .child(widgets::progress_bar(
                        "daily-progress-simple",
                        progress.fraction(),
                        running,
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .text_size(px(theme::font::SMALL))
                    .text_color(theme::faint())
                    .child(if locked {
                        format!("锁定区域 {} 段", rotation.len())
                    } else {
                        format!("段池 {} 段", self.daily_state.pool_count)
                    })
                    .child(div().size(px(3.)).rounded(px(2.)).bg(theme::faint()))
                    .child(format!(
                        "{} · {cursor_label}",
                        pool::strategy_label(strategy)
                    ))
                    .child(div().size(px(3.)).rounded(px(2.)).bg(theme::faint()))
                    .child(format!("第 {round} 轮"))
                    .child(div().size(px(3.)).rounded(px(2.)).bg(theme::faint()))
                    .child(if self.daily_state.last_run.is_empty() {
                        "尚未完成过".to_string()
                    } else {
                        format!("上次完成 {}", self.daily_state.last_run)
                    }),
            );

        let config_card = widgets::card()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(widgets::section_title("sliders", "本轮参数"))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .children(restore_params)
                            .children(running.then(|| widgets::pill("运行中", Tone::Accent))),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap_3()
                    .child(widgets::field("每轮段数", self.daily_segments.clone()))
                    .child(widgets::field("测速数量", self.daily_test.clone()))
                    .child(widgets::field("Top N", self.daily_top.clone()))
                    .child(widgets::field("扫描并发", self.daily_concurrency.clone())),
            )
            .child(region_lock_config)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        widgets::button_with_state(
                            "start-daily",
                            "运行每日优选",
                            "play",
                            ButtonKind::Primary,
                            busy,
                        )
                        .on_click(cx.listener(Self::start_daily)),
                    )
                    .child(
                        widgets::button_with_state(
                            "rebuild-pool",
                            "重建段池",
                            "database",
                            ButtonKind::Secondary,
                            busy,
                        )
                        .on_click(cx.listener(Self::rebuild_pool)),
                    )
                    .child(
                        widgets::button_with_state(
                            "reset-daily",
                            if reset_armed {
                                "确认重置"
                            } else {
                                "重置进度"
                            },
                            "refresh",
                            if reset_armed {
                                ButtonKind::Danger
                            } else {
                                ButtonKind::Ghost
                            },
                            busy,
                        )
                        .on_click(cx.listener(Self::reset_daily)),
                    ),
            )
            .child(widgets::hint(
                "运行流程：扫描本轮 IP 段 → 按握手延迟取前「测速数量」个测速 → 对最快的 2 × Top N 个做可用性验证 → 输出 Top N。",
            ))
            .child(widgets::hint(verification_hint))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .pt_3()
                    .border_t_1()
                    .border_color(theme::border())
                    .child(auto_toggle())
                    .child(widgets::hint(run_hint)),
            );

        let pool_card = widgets::card()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(widgets::section_title("database", "段池与进度"))
                    .child(widgets::pill(pool::strategy_label(strategy), Tone::Accent)),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_start()
                    .gap_5()
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_5()
                            .child(widgets::stat(
                                if locked { "锁定区域" } else { "段池" },
                                if locked {
                                    format!("{} / {}", rotation.len(), self.pool_order.len())
                                } else {
                                    self.daily_state.pool_count.to_string()
                                },
                                Tone::Blue,
                            ))
                            .child(widgets::stat(
                                "当前起点",
                                if rotation.is_empty() {
                                    "—".to_string()
                                } else {
                                    (cursor + 1).to_string()
                                },
                                Tone::Muted,
                            ))
                            .child(widgets::stat("轮次", round.to_string(), Tone::Accent)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(300.))
                            .flex()
                            .flex_col()
                            .gap_1()
                            .p_2p5()
                            .rounded(px(theme::radius::MD))
                            .bg(theme::panel_alt())
                            .child(
                                div()
                                    .text_size(px(theme::font::SMALL))
                                    .text_color(theme::faint())
                                    .child("下一轮将扫描"),
                            )
                            .child(
                                div()
                                    .font_family(theme::FONT_MONO)
                                    .text_size(px(theme::font::SMALL))
                                    .line_height(px(19.))
                                    .text_color(theme::muted())
                                    .child(preview.clone()),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .text_size(px(theme::font::LABEL))
                            .text_color(theme::muted())
                            .child(if self.daily_state.last_run.is_empty() {
                                "尚未完成过每日优选".to_string()
                            } else {
                                format!("上次完成：{}", self.daily_state.last_run)
                            }),
                    )
                    .child(div().flex_1())
                    .child(
                        widgets::button("daily-open-pool", "段池优化", "layers", ButtonKind::Soft)
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                this.switch_page(Page::Pool, window, cx);
                            })),
                    )
                    .child(
                        widgets::button(
                            "daily-open-data",
                            "打开数据目录",
                            "folder-open",
                            ButtonKind::Ghost,
                        )
                        .on_click(cx.listener(Self::open_data_dir)),
                    ),
            );

        let logs: Vec<String> = self.daily_logs.iter().cloned().collect();
        let stage_card = widgets::card()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(widgets::section_title("activity", "运行进度"))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(widgets::pill(
                                self.daily_stage.clone(),
                                if running {
                                    Tone::Accent
                                } else if self.daily_stage == "完成" {
                                    Tone::Green
                                } else {
                                    Tone::Muted
                                },
                            ))
                            .child(
                                widgets::progress_bar(
                                    "daily-progress-adv",
                                    progress.fraction(),
                                    running,
                                )
                                .w(px(220.)),
                            ),
                    ),
            )
            .child(
                div()
                    .text_size(px(theme::font::BODY))
                    .text_color(theme::muted())
                    .child(self.daily_detail.clone()),
            )
            .children(make_summary_stats(&summary, running))
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
                            .child(widgets::icon("terminal", 14.0, theme::faint()))
                            .child(
                                div()
                                    .text_size(px(theme::font::LABEL))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(theme::faint())
                                    .child("运行日志"),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(theme::font::CAPTION))
                            .text_color(theme::faint())
                            .child(format!("{} 条 · 最新在上", logs.len())),
                    ),
            )
            .child(widgets::log_view(&logs));

        let columns: [(&'static str, f32); 5] = [
            ("IP", 220.),
            ("机房", 180.),
            ("最高速度", 130.),
            ("平均速度", 110.),
            ("握手延迟", 110.),
        ];
        let top_rows = self.daily_top_rows.clone();
        let top_body = if top_rows.is_empty() {
            widgets::empty_state(
                "trophy",
                "还没有优选结果",
                "运行一次每日优选，或开启「每天自动运行」",
            )
            .h(px(140.))
            .into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .w_full()
                .children(top_rows.iter().enumerate().map(|(ix, entry)| {
                    let address = format!("{}:{}", entry.ip, entry.port);
                    let copied = address.clone();
                    div()
                        .id(("daily-top-row", ix))
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
                        .group("daily-top-row")
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.copy_text(copied.clone(), &format!(" {copied}"), cx);
                        }))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1p5()
                                .w(px(220.))
                                .flex_shrink_0()
                                .font_family(theme::FONT_MONO)
                                .text_size(px(theme::font::BODY_LG))
                                .child(address)
                                .child(widgets::copy_hint("daily-top-row")),
                        )
                        .child(
                            div()
                                .w(px(180.))
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .gap_2()
                                .overflow_hidden()
                                .child(
                                    div()
                                        .font_family(theme::FONT_MONO)
                                        .text_size(px(theme::font::BODY))
                                        .text_color(theme::green())
                                        .child(entry.colo.clone()),
                                )
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(px(theme::font::SMALL))
                                        .text_color(theme::muted())
                                        .child(crate::core::colo::place(&entry.colo)),
                                ),
                        )
                        .child(
                            div()
                                .w(px(130.))
                                .flex_shrink_0()
                                .font_family(theme::FONT_MONO)
                                .text_size(px(theme::font::BODY_LG))
                                .text_color(theme::text())
                                .child(format!("{:.1} Mbps", entry.best_mbps)),
                        )
                        .child(
                            div()
                                .w(px(110.))
                                .flex_shrink_0()
                                .font_family(theme::FONT_MONO)
                                .text_size(px(theme::font::BODY))
                                .text_color(theme::muted())
                                .child(format!("{:.1} Mbps", entry.avg_mbps)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .font_family(theme::FONT_MONO)
                                .text_size(px(theme::font::BODY))
                                .text_color(theme::muted())
                                .child(format!("{:.0} ms", entry.latency_ms)),
                        )
                }))
                .into_any_element()
        };

        let top_card = widgets::card()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(widgets::section_title("calendar", "本轮 Top N"))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .when_some(
                                top_rows
                                    .first()
                                    .and_then(|entry| entry.verification.known()),
                                |row, verification| {
                                    row.child(widgets::pill_action(
                                        verification.label(),
                                        if verification == DailyVerification::Tunnel {
                                            Tone::Green
                                        } else {
                                            Tone::Amber
                                        },
                                    ))
                                },
                            )
                            .child(widgets::pill_action(
                                format!("{} 个", top_rows.len()),
                                if top_rows.is_empty() {
                                    Tone::Muted
                                } else {
                                    Tone::Green
                                },
                            ))
                            .when(!top_rows.is_empty(), |row| {
                                row.child(
                                    widgets::button(
                                        "daily-copy-top",
                                        "复制全部",
                                        "copy",
                                        ButtonKind::Ghost,
                                    )
                                    .on_click(cx.listener(Self::copy_daily_top)),
                                )
                                .child(
                                    widgets::button(
                                        "daily-open-data-top",
                                        "打开文件夹",
                                        "folder-open",
                                        ButtonKind::Ghost,
                                    )
                                    .on_click(cx.listener(Self::open_data_dir)),
                                )
                            }),
                    ),
            )
            .when(
                top_rows
                    .iter()
                    .any(|entry| entry.verification == DailyVerification::Tls),
                |card| {
                    card.child(widgets::hint(
                        "以下 IP 仅通过 TLS 基础验证；能否用作代理，还需在「可用性验证」中用订阅节点验证。",
                    ))
                },
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w_full()
                    .rounded(px(theme::radius::LG))
                    .overflow_hidden()
                    .child(widgets::table_header(&columns))
                    .child(top_body),
            )
            .children(self.render_proxy_import(FeedSource::Daily, cx));

        if advanced {
            let mut page = div()
                .flex()
                .flex_col()
                .gap_4()
                .w_full()
                .child(config_card)
                .child(pool_card);
            if running {
                page = page.child(stage_card).child(top_card);
            } else {
                page = page.child(top_card).child(stage_card);
            }
            page
        } else {
            div()
                .flex()
                .flex_col()
                .gap_4()
                .w_full()
                .child(simple_card)
                .child(top_card)
        }
    }
}
