//! 与平台相关的小工具。

/// macOS 给 GUI 进程的 `RLIMIT_NOFILE` 软上限默认只有 256，而扫描默认并发是 300，
/// 超出上限的那部分 `socket()` 会直接以 EMFILE 失败：表现出来就是扫描「命中 0」，
/// 且耗时短到不合理（例如 5080 个目标 5 秒扫完）。启动时把软上限提到硬上限，
/// 上限以内的高并发才真正能发出去。Linux 上 systemd 服务与部分 NAS 的 Docker
/// 默认软上限只有 1024，高并发扫描同样会碰到。
///
/// macOS 上软上限超过 `kern.maxfilesperproc` 时 setrlimit 会以 EINVAL 失败（硬上限
/// 通常是 `RLIM_INFINITY`，挡不住），而这个值随内存大小变化、常低于 65536，所以先取它封顶。
#[cfg(unix)]
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
        let per_proc = max_files_per_proc().unwrap_or(TARGET);
        let before = limit.rlim_cur;
        let target = limit.rlim_max.min(per_proc).min(TARGET);
        if target <= before {
            return;
        }
        let raised = libc::rlimit {
            rlim_cur: target,
            rlim_max: limit.rlim_max,
        };
        if libc::setrlimit(libc::RLIMIT_NOFILE, &raised) == 0 {
            crate::core::storage::log_line(&format!("文件句柄上限：{before} → {target}"));
        } else {
            let err = std::io::Error::last_os_error();
            crate::core::storage::log_line(&format!(
                "提升文件句柄上限失败（{before} → {target}）：{err}"
            ));
        }
    }
}

#[cfg(target_os = "macos")]
fn max_files_per_proc() -> Option<libc::rlim_t> {
    let mut raw: libc::c_int = 0;
    let mut len = std::mem::size_of::<libc::c_int>();
    let ok = unsafe {
        libc::sysctlbyname(
            c"kern.maxfilesperproc".as_ptr(),
            (&raw mut raw).cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    } == 0;
    (ok && raw > 0).then_some(raw as libc::rlim_t)
}

/// Linux 的软上限只受硬上限约束。
#[cfg(all(unix, not(target_os = "macos")))]
fn max_files_per_proc() -> Option<libc::rlim_t> {
    None
}

/// Windows 没有每进程的 socket 数上限，无需调整。
#[cfg(not(unix))]
pub fn raise_fd_limit() {}
