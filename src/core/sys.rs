//! 与平台相关的小工具。

/// macOS 给 GUI 进程的 `RLIMIT_NOFILE` 软上限默认只有 256，而扫描默认并发是 300，
/// 超出上限的那部分 `socket()` 会直接以 EMFILE 失败：表现出来就是扫描「命中 0」，
/// 且耗时短到不合理（例如 5080 个目标 5 秒扫完）。启动时把软上限提到硬上限，
/// 上限以内的高并发才真正能发出去。
#[cfg(target_os = "macos")]
pub fn raise_fd_limit() {
    const TARGET: libc::rlim_t = 65_536;
    unsafe {
        let mut limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        if libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) != 0 {
            return;
        }
        let before = limit.rlim_cur;
        let target = limit.rlim_max.min(TARGET);
        if target <= before {
            return;
        }
        let raised = libc::rlimit {
            rlim_cur: target,
            rlim_max: limit.rlim_max,
        };
        if libc::setrlimit(libc::RLIMIT_NOFILE, &raised) == 0 {
            crate::core::storage::log_line(&format!("文件句柄上限：{before} → {target}"));
        }
    }
}

/// 其他系统的默认 fd 上限足够跑满并发，无需调整。
#[cfg(not(target_os = "macos"))]
pub fn raise_fd_limit() {}
