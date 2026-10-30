#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod assets;
mod autostart;
mod core;
mod jobs;
mod proxy_clients;
mod taskbar;
mod theme;
mod tray;
mod ui;
mod updater;

use gpui::{
    App, Application, Bounds, KeyBinding, Menu, MenuItem, TitlebarOptions, WindowBounds,
    WindowOptions, point, prelude::*, px, size,
};

use crate::app::AppState;
use crate::core::storage::{self, AppPaths};

fn main() {
    let paths = AppPaths::discover();
    storage::init_logging(&paths);
    storage::install_panic_hook();
    crate::core::sys::raise_fd_limit();
    // 更新后由旧版本拉起：旧进程退出、释放单实例锁之前先等着。
    let after_update = updater::launched_after_update();
    let lock_wait = std::time::Duration::from_secs(if after_update { 20 } else { 0 });
    let _instance_lock = match storage::acquire_instance_lock(&paths, lock_wait) {
        Some(lock) => lock,
        None => {
            storage::log_line("已有另一个实例在运行，退出");
            return;
        }
    };
    if after_update {
        storage::log_line(&format!("已更新到 v{}", env!("CARGO_PKG_VERSION")));
        updater::cleanup_after_update();
    }

    Application::new()
        .with_assets(assets::Assets)
        .run(|cx: &mut App| {
            let mut keys = ui::input::default_key_bindings();
            keys.extend([
                KeyBinding::new("secondary-1", app::SwitchDaily, None),
                KeyBinding::new("secondary-2", app::SwitchScan, None),
                KeyBinding::new("secondary-3", app::SwitchSpeed, None),
                KeyBinding::new("secondary-4", app::SwitchVerify, None),
                KeyBinding::new("secondary-5", app::SwitchPool, None),
                KeyBinding::new("secondary-6", app::SwitchHistory, None),
                KeyBinding::new("secondary-enter", app::StartCurrent, None),
                KeyBinding::new("secondary-.", app::StopCurrent, None),
                KeyBinding::new("secondary-shift-a", app::ToggleAdvanced, None),
                KeyBinding::new("secondary-b", app::ToggleSidebar, None),
                KeyBinding::new("secondary-=", app::ZoomIn, None),
                KeyBinding::new("secondary--", app::ZoomOut, None),
                KeyBinding::new("secondary-0", app::ZoomReset, None),
                KeyBinding::new("tab", app::FocusNext, None),
                KeyBinding::new("shift-tab", app::FocusPrev, None),
            ]);
            cx.bind_keys(keys);
            // macOS 没有菜单栏时 ⌘Q 无效：提供只含「退出」的应用菜单。
            if cfg!(target_os = "macos") {
                cx.on_action(|_: &app::Quit, cx| cx.quit());
                cx.bind_keys([KeyBinding::new("cmd-q", app::Quit, None)]);
                cx.set_menus(vec![Menu {
                    name: "CF Best".into(),
                    items: vec![MenuItem::action("退出 CF Best", app::Quit)],
                }]);
            }
            cx.on_window_closed(|cx| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            let bounds = Bounds::centered(None, size(px(1320.0), px(860.0)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(1120.0), px(720.0))),
                    titlebar: Some(TitlebarOptions {
                        title: Some("CF Best · Cloudflare 优选工作台".into()),
                        appears_transparent: true,
                        // macOS：红绿灯放在侧栏顶部留出的标题栏行里（收起时 64px 也放得下）。
                        traffic_light_position: cfg!(target_os = "macos")
                            .then(|| point(px(10.0), px(10.0))),
                    }),
                    focus: true,
                    show: true,
                    app_id: Some("CFBest".into()),
                    ..Default::default()
                },
                |window, cx| {
                    let view = cx.new(AppState::new);
                    view.update(cx, |app, cx| app.attach_window(window, cx));
                    let focus = view.read(cx).initial_focus(cx);
                    window.focus(&focus);
                    view
                },
            )
            .expect("无法创建主窗口");
            cx.activate(true);
        });
}
