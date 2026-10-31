//! 顶栏、全局进度条与自绘窗口控制（最小化 / 最大化 / 关闭 / 拖拽）。

use super::*;
use crate::ui::widgets::Focusable;

impl AppState {
    pub(super) fn job_progress(&self) -> Option<f32> {
        let progress = match self.job? {
            JobKind::Scan => &self.scan_progress,
            JobKind::Speed => &self.speed_progress,
            JobKind::Verify => &self.verify_progress,
            JobKind::Daily => &self.daily_progress,
            JobKind::Probe => &self.probe_progress,
            JobKind::Classify => &self.classify_progress,
            _ => return None,
        };
        (progress.total > 0).then(|| progress.fraction())
    }

    /// 工作区面板顶部的标题栏：面包屑 + 页面说明 + 任务状态 + 窗口控制。
    /// 对应 Qoder 工作台的 48px 标题栏（这里放大到 56px 以容纳两行）。
    pub(super) fn render_topbar(&mut self, maximized: bool, cx: &mut Context<Self>) -> gpui::Div {
        let running = self.job;
        let progress = self.job_progress();
        div()
            .relative()
            .flex()
            .items_center()
            .justify_between()
            .h(px(56.))
            .pl_5()
            .flex_shrink_0()
            .border_b_1()
            .border_color(theme::border_subtle())
            .child(
                div()
                    .id("titlebar-drag")
                    .window_control_area(gpui::WindowControlArea::Drag)
                    .occlude()
                    .flex()
                    .flex_col()
                    .justify_center()
                    .gap(px(3.))
                    .min_w_0()
                    .h_full()
                    .when(!cfg!(target_os = "windows"), |this| {
                        this.on_mouse_down(MouseButton::Left, cx.listener(Self::start_window_drag))
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .text_size(px(theme::font::BODY))
                            .line_height(px(18.))
                            .child(widgets::icon(self.page.icon(), 13.0, theme::faint()))
                            .child(div().text_color(theme::faint()).child("CF Best"))
                            .child(widgets::icon("chevron-right", 10.0, theme::quaternary()))
                            .child(
                                div()
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(theme::text())
                                    .child(self.page.title()),
                            )
                            .children(self.page.step().map(|step| {
                                div()
                                    .ml_1()
                                    .px(px(6.))
                                    .rounded(px(theme::radius::SM))
                                    .bg(theme::fill_secondary())
                                    .font_family(theme::FONT_MONO)
                                    .text_size(px(theme::font::MICRO))
                                    .text_color(theme::faint())
                                    .child(step)
                            })),
                    )
                    .child(
                        div()
                            .text_size(px(theme::font::SMALL))
                            .line_height(px(17.))
                            .text_color(theme::faint())
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(self.page.subtitle()),
                    ),
            )
            .child(
                div()
                    .id("titlebar-drag-fill")
                    .window_control_area(gpui::WindowControlArea::Drag)
                    .occlude()
                    .flex_1()
                    .h_full()
                    .when(!cfg!(target_os = "windows"), |this| {
                        this.on_mouse_down(MouseButton::Left, cx.listener(Self::start_window_drag))
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .h_full()
                    .when_some(running, |row, kind| {
                        let label = match progress {
                            Some(fraction) => {
                                format!("{}运行中 · {:.0}%", kind.label(), fraction * 100.0)
                            }
                            None => format!("{}运行中", kind.label()),
                        };
                        row.child(widgets::running_chip(
                            "topbar-job-spinner",
                            label,
                            Tone::Accent,
                        ))
                        .child(
                            widgets::button(
                                "stop-job",
                                "停止",
                                "stop",
                                widgets::ButtonKind::Danger,
                            )
                            .on_click(cx.listener(Self::stop_job)),
                        )
                    })
                    .when_some(self.aux_job, |row, kind| {
                        row.child(widgets::running_chip(
                            "topbar-aux-spinner",
                            format!("{}进行中", kind.label()),
                            Tone::Blue,
                        ))
                    })
                    .when(self.sub_loading, |row| {
                        row.child(widgets::running_chip(
                            "topbar-sub-spinner",
                            "订阅载入中",
                            Tone::Blue,
                        ))
                    })
                    // macOS 使用系统的红绿灯按钮（见 main.rs 的 traffic_light_position）。
                    .when(!cfg!(target_os = "macos"), |row| {
                        row.child(
                            div()
                                .flex()
                                .items_center()
                                .h_full()
                                .ml_2()
                                .child(self.window_control(
                                    cx,
                                    "win-min",
                                    "minimize",
                                    false,
                                    Self::minimize_window,
                                ))
                                .child(self.window_control(
                                    cx,
                                    "win-max",
                                    if maximized { "restore" } else { "maximize" },
                                    false,
                                    Self::maximize_window,
                                ))
                                .child(self.window_control(
                                    cx,
                                    "win-close",
                                    "close",
                                    true,
                                    Self::close_window,
                                )),
                        )
                    }),
            )
            .when(running.is_some(), |bar| {
                bar.child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom(px(-1.))
                        .h(px(2.))
                        .child(match progress {
                            Some(fraction) => div()
                                .h_full()
                                .w(gpui::relative(fraction.max(0.01)))
                                .bg(theme::accent()),
                            None => div().h_full().w_full().bg(theme::accent_soft()),
                        }),
                )
            })
    }

    /// Qoder 的窗口控制按钮：46px 宽、中性图标；只有关闭按钮在悬停时变为 `#c42b1c`。
    /// 关闭按钮位于工作区面板右上角，圆角贴合面板边框内沿。
    pub(super) fn window_control(
        &self,
        cx: &mut Context<Self>,
        id: &'static str,
        icon_name: &'static str,
        is_close: bool,
        handler: fn(&mut Self, &ClickEvent, &mut Window, &mut Context<Self>),
    ) -> gpui::AnyElement {
        div()
            .id(id)
            .group(id)
            .flex()
            .items_center()
            .justify_center()
            .w(px(46.))
            .h_full()
            .cursor_pointer()
            .keyboard_focusable(theme::radius::SM)
            .when(is_close, |this| {
                this.rounded_tr(gpui::px(WORKBENCH_OUTER_RADIUS - 1.))
                    .hover(|style| style.bg(gpui::rgb(0xc42b1c)))
            })
            .when(!is_close, |this| {
                this.hover(|style| style.bg(theme::fill_secondary()))
            })
            .on_click(cx.listener(handler))
            .child(
                gpui::svg()
                    .path(format!("icons/{icon_name}.svg"))
                    .size(px(13.))
                    .flex_shrink_0()
                    .text_color(theme::muted())
                    .group_hover(id, move |style| {
                        style.text_color(if is_close {
                            gpui::rgb(0xffffff)
                        } else {
                            theme::text()
                        })
                    }),
            )
            .into_any_element()
    }

    /// 拖拽区域在 Windows 上由系统处理：`WindowControlArea::Drag` 让 `WM_NCHITTEST`
    /// 返回 `HTCAPTION`，系统负责移动窗口、Aero Snap、双击最大化与跨显示器 DPI 切换。
    ///
    /// GPUI 会先把 `WM_NCLBUTTONDOWN` 当作 MouseDown 分发给元素，只要有元素
    /// `prevent_default`（根元素的焦点跟踪就会），消息就被吞掉、系统不会开始拖拽。
    /// 因此拖拽区域需要 `.occlude()` 挡住祖先元素的命中，并且在 Windows 上不注册
    /// 本监听（否则双击会被这里和系统各切换一次最大化而相互抵消）。
    /// 本函数只在其他平台使用。
    pub(super) fn start_window_drag(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.click_count >= 2 {
            // macOS 按系统设置（缩放或最小化）响应标题栏双击。
            if cfg!(target_os = "macos") {
                window.titlebar_double_click();
            } else {
                self.maximize_window(&ClickEvent::default(), window, cx);
            }
            return;
        }
        if !window.is_maximized() {
            window.start_window_move();
        }
    }

    /// 窗口创建后调用一次：系统明暗监听、托盘、拦截 Alt+F4（托盘常驻时改为隐藏）。
    pub fn attach_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.watch_system_appearance(window, cx);
        self.sync_tray(window);
        // 开机自启拉起时直接留在托盘；没有托盘时照常显示窗口。
        if crate::autostart::launched_in_background() && self.tray.is_some() {
            crate::tray::hide_main_window();
        }
        // 程序移动位置后，自启项仍指向旧路径；启动时按当前路径刷新（调试构建不改注册表）。
        if self.settings.launch_at_login
            && !cfg!(debug_assertions)
            && let Err(err) = crate::autostart::set_enabled(true)
        {
            storage::log_line(&format!("刷新开机自启失败：{err}"));
        }
        #[cfg(all(target_os = "windows", debug_assertions))]
        if std::env::var("CFBEST_TRAY_SELFTEST").is_ok() {
            self.tray_self_test(cx);
        }
        window.on_window_should_close(cx, |_, _| {
            if crate::tray::active() && !crate::tray::exit_requested() {
                crate::tray::hide_main_window();
                false
            } else {
                true
            }
        });
    }

    /// 调试用自检（`CFBEST_TRAY_SELFTEST`）：模拟关闭 → 托盘左键恢复，结果写入运行日志。
    #[cfg(all(target_os = "windows", debug_assertions))]
    fn tray_self_test(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let executor = cx.background_executor().clone();
            let wait = |ms| executor.timer(Duration::from_millis(ms));
            wait(1500).await;
            this.update(cx, |app, _| {
                storage::log_line(&format!(
                    "TRAY SELFTEST active={} hidden={}",
                    crate::tray::active(),
                    crate::tray::main_window_hidden()
                ));
                let hid = app.hide_to_tray();
                storage::log_line(&format!("TRAY SELFTEST close->hide_to_tray={hid}"));
            })
            .ok();
            wait(800).await;
            storage::log_line(&format!(
                "TRAY SELFTEST after close hidden={}",
                crate::tray::main_window_hidden()
            ));
            crate::tray::post_left_click();
            wait(800).await;
            storage::log_line(&format!(
                "TRAY SELFTEST after tray click hidden={}",
                crate::tray::main_window_hidden()
            ));
        })
        .detach();
    }

    /// 侧栏与托盘提示共用的任务状态文字。
    pub(super) fn job_summary(&self) -> String {
        if let Some(kind) = self.job {
            format!("{}运行中", kind.label())
        } else if let Some(kind) = self.aux_job {
            format!("{}进行中", kind.label())
        } else if self.sub_loading {
            "订阅载入中".to_string()
        } else {
            "空闲".to_string()
        }
    }

    /// 按设置创建或移除托盘图标。
    pub(super) fn sync_tray(&mut self, window: &Window) {
        if self.settings.tray_enabled && self.tray.is_none() {
            let tip = format!("CF Best · {}", self.job_summary());
            self.tray = crate::tray::create(window, &tip);
            if self.tray.is_none() {
                storage::log_line("托盘图标创建失败");
            }
        } else if !self.settings.tray_enabled {
            self.tray = None;
        }
    }

    /// 开启托盘常驻（如尚未开启）。返回是否由这次调用新开启。
    pub(super) fn ensure_tray(&mut self, window: &Window) -> bool {
        if self.settings.tray_enabled || !crate::tray::SUPPORTED {
            return false;
        }
        self.settings.tray_enabled = true;
        self.save_settings();
        self.sync_tray(window);
        true
    }

    pub(super) fn toggle_tray(
        &mut self,
        _: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.settings.tray_enabled = !self.settings.tray_enabled;
        self.save_settings();
        self.sync_tray(window);
        let text = match (self.settings.tray_enabled, self.tray.is_some()) {
            (true, true) => "托盘常驻已开启：最小化或关闭会隐藏到托盘，右键托盘图标退出",
            (true, false) => "托盘图标创建失败，已保持普通窗口行为",
            (false, _) if self.settings.auto_daily => {
                "托盘常驻已关闭：关闭窗口后应用会退出，每日自动优选将不会运行"
            }
            (false, _) => "托盘常驻已关闭",
        };
        let tone = if (self.settings.tray_enabled && self.tray.is_none())
            || (!self.settings.tray_enabled && self.settings.auto_daily)
        {
            Tone::Amber
        } else {
            Tone::Muted
        };
        self.notify_status(text, tone, cx);
    }

    /// 托盘常驻时把窗口隐藏到托盘；首次隐藏用气泡提示仍在后台运行。
    fn hide_to_tray(&mut self) -> bool {
        let Some(tray) = self.tray.as_ref() else {
            return false;
        };
        crate::tray::hide_main_window();
        if !self.tray_hint_shown {
            self.tray_hint_shown = true;
            tray.notify("CF Best 仍在后台运行", "单击托盘图标恢复窗口，右键可退出。");
        }
        true
    }

    /// 托盘常驻且窗口已隐藏时发送气泡通知（如每日优选完成）。
    pub(super) fn notify_in_background(&self, title: &str, text: &str) {
        if let Some(tray) = self.tray.as_ref()
            && crate::tray::main_window_hidden()
        {
            tray.notify(title, text);
        }
    }

    pub(super) fn minimize_window(
        &mut self,
        _: &ClickEvent,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        if !self.hide_to_tray() {
            window.minimize_window();
        }
    }

    pub(super) fn maximize_window(
        &mut self,
        _: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.is_maximized() {
            #[cfg(target_os = "windows")]
            {
                use raw_window_handle::{HasWindowHandle, RawWindowHandle};
                if let Ok(handle) = window.window_handle()
                    && let RawWindowHandle::Win32(handle) = handle.as_raw()
                {
                    unsafe {
                        windows_sys::Win32::UI::WindowsAndMessaging::ShowWindowAsync(
                            handle.hwnd.get() as windows_sys::Win32::Foundation::HWND,
                            windows_sys::Win32::UI::WindowsAndMessaging::SW_RESTORE,
                        );
                    }
                }
            }
        } else {
            window.zoom_window();
        }
        cx.notify();
    }

    pub(super) fn close_window(
        &mut self,
        _: &ClickEvent,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        if !self.hide_to_tray() {
            window.remove_window();
        }
    }
}
