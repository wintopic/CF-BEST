//! 扫描 / 测速 / 验证 / 每日优选的按钮回调与任务启动。

use super::*;

impl AppState {
    pub(super) fn start_scan(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.start_scan_inner(cx);
    }

    pub(super) fn start_scan_inner(&mut self, cx: &mut Context<Self>) {
        if self.busy() {
            self.fail("已有任务在运行，请等待完成或先停止", cx);
            return;
        }
        let advanced = self.settings.advanced_mode;
        let (ports, concurrency, timeout_secs, sample_pct) = if advanced {
            let ports = match ip::parse_ports(self.scan_ports.read(cx).content()) {
                Ok(ports) => ports,
                Err(err) => return self.fail(err, cx),
            };
            let concurrency =
                match parse_range::<u32>(self.scan_concurrency.read(cx).content(), "并发", 1, 1000)
                {
                    Ok(value) => value,
                    Err(err) => return self.fail(err, cx),
                };
            let timeout_secs =
                match parse_range::<f64>(self.scan_timeout.read(cx).content(), "超时", 0.5, 30.0)
                {
                    Ok(value) => value,
                    Err(err) => return self.fail(err, cx),
                };
            let sample_pct = match parse_range::<f64>(
                self.scan_sample.read(cx).content(),
                "抽样比例",
                0.0,
                100.0,
            ) {
                Ok(value) => value,
                Err(err) => return self.fail(err, cx),
            };
            (ports, concurrency, timeout_secs, sample_pct)
        } else {
            // 简单模式不显示参数，沿用已保存的设置（默认即推荐值），与高级模式一致。
            let settings = &self.settings;
            (
                ip::parse_ports(&settings.scan_ports).unwrap_or_else(|_| vec![443]),
                settings.scan_concurrency,
                settings.scan_timeout_secs,
                settings.scan_sample_pct,
            )
        };
        if self.scan_input.read(cx).content().trim().is_empty() {
            self.fail("请先粘贴要扫描的 IP 段，例如 104.16.0.0/24", cx);
            return;
        }
        let targets = match ip::parse_targets(
            self.scan_input.read(cx).content(),
            &ports,
            sample_pct / 100.0,
            ip::MAX_TARGETS,
        ) {
            Ok(targets) => targets,
            Err(err) => return self.fail(err, cx),
        };

        self.settings.scan_ports = ports
            .iter()
            .map(u16::to_string)
            .collect::<Vec<_>>()
            .join(",");
        self.settings.scan_concurrency = concurrency;
        self.settings.scan_timeout_secs = timeout_secs;
        self.settings.scan_sample_pct = sample_pct;
        self.save_settings();

        self.scan_hits.clear();
        self.scan_region = None;
        self.scan_progress = ProgressState {
            done: 0,
            total: targets.len(),
            label: format!("准备探测 {} 个目标", targets.len()),
        };
        self.scan_note = "扫描运行中…".into();
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(cancel.clone());
        self.job = Some(JobKind::Scan);
        self.page = Page::Scan;
        self.reset_page_scroll();
        let config = crate::core::scan::ScanConfig {
            targets,
            concurrency: concurrency as usize,
            timeout: Duration::from_secs_f64(timeout_secs),
        };
        let target_count = config.targets.len();
        crate::jobs::spawn_scan(config, self.events_tx.clone(), cancel);
        self.notify_status(
            if advanced {
                format!("扫描已启动 · 并发 {concurrency} · 超时 {timeout_secs}s")
            } else {
                format!("开始扫描 · 共 {target_count} 个目标")
            },
            Tone::Accent,
            cx,
        );
    }

    pub(super) fn stop_job(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.stop_job_inner(cx);
    }

    pub(super) fn stop_job_inner(&mut self, cx: &mut Context<Self>) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Relaxed);
            self.notify_status("正在停止任务，等待在途请求结束…", Tone::Amber, cx);
        }
    }

    pub(super) fn load_scan_hits_to_speed(
        &mut self,
        _: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.scan_hits.is_empty() {
            self.fail("暂无扫描结果，请先完成一次扫描", cx);
            return;
        }
        let count = self.fill_speed_from_scan(cx);
        self.page = Page::Speed;
        self.reset_page_scroll();
        self.focus_page_input(Page::Speed, window, cx);
        self.notify_status(
            format!("已载入前 {count} 个扫描结果，点击「开始测速」继续"),
            Tone::Green,
            cx,
        );
    }

    /// 把握手最快的扫描结果填入测速候选，返回填入数量。
    fn fill_speed_from_scan(&mut self, cx: &mut Context<Self>) -> usize {
        let limit = if self.settings.advanced_mode { 100 } else { 20 };
        // 筛选了出口区域时只送筛选出的地址。
        let mut sorted: Vec<_> = self
            .scan_visible()
            .into_iter()
            .map(|index| self.scan_hits[index].clone())
            .collect();
        sorted.sort_by(|a, b| a.latency_ms.total_cmp(&b.latency_ms));
        let selected: Vec<String> = sorted
            .iter()
            .take(limit)
            .map(|hit| format!("{}:{}", hit.ip, hit.port))
            .collect();
        let count = selected.len();
        self.speed_input
            .update(cx, |input, _| input.set_content(selected.join("\n")));
        count
    }

    /// 「下一步：测速」：载入扫描结果并直接开始测速，省去到测速页再点一次。
    pub(super) fn next_to_speed(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.scan_hits.is_empty() {
            return self.fail("暂无扫描结果，请先完成一次扫描", cx);
        }
        if self.busy() {
            return self.fail("已有任务在运行，请等待完成或先停止", cx);
        }
        self.fill_speed_from_scan(cx);
        self.start_speed_inner(cx);
    }

    /// 「下一步：验证」：载入测速最快的 IP 并直接开始可用性验证。
    pub(super) fn next_to_verify(
        &mut self,
        _: &ClickEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.speed_records.is_empty() {
            return self.fail("暂无测速结果，请先完成一次测速", cx);
        }
        if self.busy() {
            return self.fail("已有任务在运行，请等待完成或先停止", cx);
        }
        self.fill_verify_from_speed(cx);
        self.start_verify_inner(cx);
    }

    /// 一键自动优选：切到自动优选页并立即运行一轮。
    pub(super) fn run_daily_now(
        &mut self,
        _: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.switch_page(Page::Daily, window, cx);
        if let Err(err) = self.start_daily_inner(cx) {
            self.fail(err, cx);
        }
    }

    pub(super) fn export_scan(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.scan_hits.is_empty() {
            self.fail("没有可导出的扫描结果", cx);
            return;
        }
        match storage::write_scan_exports(&self.paths, &self.scan_hits) {
            Ok(paths) => self.show_export_toast(&paths, cx),
            Err(err) => self.fail(format!("导出失败：{err}"), cx),
        }
    }

    pub(super) fn fetch_asn(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.busy() {
            self.fail("已有任务在运行", cx);
            return;
        }
        let asn = self.scan_asn.read(cx).content().trim().to_string();
        if asn.is_empty() {
            self.fail("请输入 ASN，例如 AS906", cx);
            return;
        }
        self.aux_job = Some(JobKind::Asn);
        crate::jobs::spawn_asn_fetch(asn.clone(), self.events_tx.clone());
        self.notify_status(format!("正在拉取 {asn} 的 IPv4 前缀…"), Tone::Accent, cx);
    }

    pub(super) fn start_speed(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.start_speed_inner(cx);
    }

    pub(super) fn start_speed_inner(&mut self, cx: &mut Context<Self>) {
        if self.busy() {
            self.fail("已有任务在运行，请等待完成或先停止", cx);
            return;
        }
        let advanced = self.settings.advanced_mode;
        // 候选为空时自动带入扫描结果，两种模式一致。
        if self.speed_input.read(cx).content().trim().is_empty() && !self.scan_hits.is_empty() {
            self.fill_speed_from_scan(cx);
        }
        if self.speed_input.read(cx).content().trim().is_empty() {
            return self.fail(
                "候选为空：先在「扫描 IP」得到结果，或点击「官方候选池」",
                cx,
            );
        }
        let targets = match ip::parse_ip_targets(self.speed_input.read(cx).content(), 443) {
            Ok(targets) => targets,
            Err(err) => return self.fail(err, cx),
        };
        if targets.len() > 5000 {
            return self.fail("候选数超过 5000，请分批测速", cx);
        }
        let (coarse, fine, rounds, topn) = if advanced {
            let coarse =
                match parse_range::<u64>(self.speed_coarse.read(cx).content(), "初筛时长", 1, 30)
                {
                    Ok(value) => value,
                    Err(err) => return self.fail(err, cx),
                };
            let fine =
                match parse_range::<u64>(self.speed_fine.read(cx).content(), "精测时长", 1, 60)
                {
                    Ok(value) => value,
                    Err(err) => return self.fail(err, cx),
                };
            let rounds =
                match parse_range::<u32>(self.speed_rounds.read(cx).content(), "精测轮数", 1, 10)
                {
                    Ok(value) => value,
                    Err(err) => return self.fail(err, cx),
                };
            let topn =
                match parse_range::<usize>(self.speed_topn.read(cx).content(), "精测数量", 1, 200)
                {
                    Ok(value) => value,
                    Err(err) => return self.fail(err, cx),
                };
            (coarse, fine, rounds, topn)
        } else {
            let settings = &self.settings;
            (
                settings.speed_coarse_secs,
                settings.speed_fine_secs,
                settings.speed_fine_rounds,
                settings.speed_fine_topn,
            )
        };

        self.settings.speed_coarse_secs = coarse;
        self.settings.speed_fine_secs = fine;
        self.settings.speed_fine_rounds = rounds;
        self.settings.speed_fine_topn = topn;
        self.save_settings();

        self.speed_records.clear();
        self.speed_warnings.clear();
        self.speed_throttled = 0;
        self.speed_phase = "准备".into();
        self.speed_current = format!("{} 个候选", targets.len());
        self.speed_progress = ProgressState {
            done: 0,
            total: targets.len(),
            label: "准备测速".into(),
        };
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(cancel.clone());
        self.job = Some(JobKind::Speed);
        self.page = Page::Speed;
        self.reset_page_scroll();
        let config = crate::core::speed::SpeedConfig {
            targets,
            coarse_secs: coarse,
            fine_secs: fine,
            fine_rounds: rounds,
            fine_topn: topn,
            timeout: Duration::from_secs(6),
        };
        let target_count = config.targets.len();
        crate::jobs::spawn_speed(config, self.events_tx.clone(), cancel);
        self.notify_status(
            if advanced {
                "测速已启动 · 逐个测速，避免多个连接分摊带宽".to_string()
            } else {
                format!("开始测速 · {target_count} 个候选，请稍候")
            },
            Tone::Accent,
            cx,
        );
    }

    pub(super) fn load_official_pool(
        &mut self,
        quick: bool,
        _: &ClickEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy() {
            self.fail("已有任务在运行", cx);
            return;
        }
        let total = if quick {
            if self.settings.advanced_mode { 120 } else { 40 }
        } else {
            400
        };
        self.aux_job = Some(JobKind::Pool);
        crate::jobs::spawn_official_pool(total, self.events_tx.clone());
        self.notify_status(
            format!("正在生成官方候选池（{total} 个）…"),
            Tone::Accent,
            cx,
        );
    }

    pub(super) fn pool_quick(
        &mut self,
        event: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.load_official_pool(true, event, window, cx);
    }

    pub(super) fn pool_full(
        &mut self,
        event: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.load_official_pool(false, event, window, cx);
    }

    pub(super) fn export_speed(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.speed_records.is_empty() {
            self.fail("没有可导出的测速结果", cx);
            return;
        }
        match storage::write_speed_export(&self.paths, &self.speed_records, self.speed_throttled) {
            Ok(paths) => self.show_export_toast(&paths, cx),
            Err(err) => self.fail(format!("导出失败：{err}"), cx),
        }
    }

    pub(super) fn load_speed_to_verify(
        &mut self,
        _: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.speed_records.is_empty() {
            self.fail("暂无测速结果，请先完成一次测速", cx);
            return;
        }
        let count = self.fill_verify_from_speed(cx);
        self.page = Page::Verify;
        self.reset_page_scroll();
        self.focus_page_input(Page::Verify, window, cx);
        self.notify_status(
            format!("已载入速度最快的 {count} 个 IP 到「可用性验证」"),
            Tone::Green,
            cx,
        );
    }

    /// 把中位速度最快的 20 个测速结果填入验证列表，返回填入数量。
    fn fill_verify_from_speed(&mut self, cx: &mut Context<Self>) -> usize {
        let mut sorted = self.speed_records.clone();
        sorted.sort_by(|a, b| b.median.total_cmp(&a.median));
        let selected: Vec<String> = sorted
            .iter()
            .take(20)
            .map(|record| format!("{}:{}", record.ip, record.port))
            .collect();
        let count = selected.len();
        self.verify_input
            .update(cx, |input, _| input.set_content(selected.join("\n")));
        count
    }

    pub(super) fn load_subscription(
        &mut self,
        _: &ClickEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.sub_loading {
            self.notify_status("订阅正在载入，请稍候", Tone::Muted, cx);
            return;
        }
        let url = self.sub_url.read(cx).content().trim().to_string();
        if url.is_empty() {
            self.fail("订阅地址为空", cx);
            return;
        }
        self.settings.sub_url = url.clone();
        self.save_settings();
        self.start_subscription_load(url);
        self.notify_status("正在拉取订阅节点…", Tone::Accent, cx);
    }

    /// 在后台拉取订阅，失败时自动重试；结果见 `SubscriptionReady` / `SubscriptionFailed`。
    pub(super) fn start_subscription_load(&mut self, url: String) {
        self.sub_loading = true;
        crate::jobs::spawn_subscription(url, self.events_tx.clone());
    }

    /// 换上一批订阅节点（新拉取的或上次成功载入的），更新节点说明并交给本机订阅服务。
    pub(super) fn apply_subscription_nodes(&mut self, nodes: Vec<VlessNode>, note: Option<&str>) {
        let ws_count = nodes
            .iter()
            .filter(|node| node.transport.eq_ignore_ascii_case("ws"))
            .count();
        let mut source = format!("{} 个节点（{ws_count} 个 ws）", nodes.len());
        if let Some(note) = note {
            source.push_str(" · ");
            source.push_str(note);
        }
        // 与 verify::node_for_ips 一致：只有 WS 节点用于验证 IP，其余传输改用内置探测。
        self.verify_node_source = match nodes
            .iter()
            .find(|node| node.transport.eq_ignore_ascii_case("ws"))
        {
            Some(node) => format!("{source} · {}", node.summary()),
            None if nodes.is_empty() => format!("{source} · 无可用节点"),
            None => format!("{source} · 无 WS 节点，验证 IP 时只做 TLS 基础验证"),
        };
        self.verify_nodes = nodes;
        self.publish_proxy_feeds();
    }

    pub(super) fn start_verify(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.start_verify_inner(cx);
    }

    pub(super) fn start_verify_inner(&mut self, cx: &mut Context<Self>) {
        if self.busy() {
            self.fail("已有任务在运行，请等待完成或先停止", cx);
            return;
        }
        if self.awaiting_nodes() {
            self.notify_status(
                "订阅节点正在载入，载入后再验证才能走隧道验证",
                Tone::Amber,
                cx,
            );
            return;
        }
        let advanced = self.settings.advanced_mode;
        let raw = self.verify_input.read(cx).content().trim().to_string();
        let mut targets = Vec::new();
        if !advanced {
            if self.speed_records.is_empty() {
                return self.fail("还没有测速结果：请先在「测速排行」完成测速", cx);
            }
            let node = self.sync_verify_node();
            let mut sorted = self.speed_records.clone();
            sorted.sort_by(|a, b| b.median.total_cmp(&a.median));
            let selected: Vec<String> = sorted
                .iter()
                .take(20)
                .map(|record| format!("{}:{}", record.ip, record.port))
                .collect();
            self.verify_input
                .update(cx, |input, _| input.set_content(selected.join("\n")));
            for record in sorted.iter().take(20) {
                targets.push(VerifyTarget {
                    addr: record.ip.clone(),
                    port: record.port,
                    node: node.clone(),
                });
            }
        } else if raw.is_empty() {
            if !self.verify_nodes.is_empty() {
                for node in &self.verify_nodes {
                    targets.push(VerifyTarget {
                        addr: node.addr.clone(),
                        port: node.port,
                        node: node.clone(),
                    });
                }
            } else if !self.speed_records.is_empty() {
                let node = self.sync_verify_node();
                let mut sorted = self.speed_records.clone();
                sorted.sort_by(|a, b| b.median.total_cmp(&a.median));
                let selected: Vec<String> = sorted
                    .iter()
                    .take(20)
                    .map(|record| format!("{}:{}", record.ip, record.port))
                    .collect();
                self.verify_input
                    .update(cx, |input, _| input.set_content(selected.join("\n")));
                for record in sorted.iter().take(20) {
                    targets.push(VerifyTarget {
                        addr: record.ip.clone(),
                        port: record.port,
                        node: node.clone(),
                    });
                }
            } else {
                self.fail("IP 列表为空，且没有测速结果或订阅节点", cx);
                return;
            }
        } else {
            let node = self.sync_verify_node();
            let endpoints = match ip::parse_ip_targets(&raw, 443) {
                Ok(endpoints) => endpoints,
                Err(err) => return self.fail(err, cx),
            };
            for (ip, port) in endpoints {
                targets.push(VerifyTarget {
                    addr: ip,
                    port,
                    node: node.clone(),
                });
            }
        }
        self.spawn_verify_targets(targets, cx);
    }

    pub(super) fn verify_subscription(
        &mut self,
        _: &ClickEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy() {
            self.fail("已有任务在运行", cx);
            return;
        }
        if self.verify_nodes.is_empty() {
            if self.sub_loading {
                self.notify_status("订阅节点正在载入，请稍候", Tone::Amber, cx);
            } else {
                self.fail("尚未拉取订阅，无法验证订阅节点", cx);
            }
            return;
        }
        let targets: Vec<VerifyTarget> = self
            .verify_nodes
            .iter()
            .map(|node| VerifyTarget {
                addr: node.addr.clone(),
                port: node.port,
                node: node.clone(),
            })
            .collect();
        self.spawn_verify_targets(targets, cx);
    }

    pub(super) fn spawn_verify_targets(
        &mut self,
        targets: Vec<VerifyTarget>,
        cx: &mut Context<Self>,
    ) {
        if targets.is_empty() {
            self.fail("没有待验证的目标", cx);
            return;
        }
        self.verify_records.clear();
        self.verify_progress = ProgressState {
            done: 0,
            total: targets.len(),
            label: "准备验证".into(),
        };
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(cancel.clone());
        self.job = Some(JobKind::Verify);
        self.page = Page::Verify;
        self.reset_page_scroll();
        let count = targets.len();
        crate::jobs::spawn_verify(
            targets,
            Duration::from_secs(8),
            self.events_tx.clone(),
            cancel,
        );
        self.notify_status(
            format!("可用性验证已启动 · {count} 个目标 · 逐个执行"),
            Tone::Accent,
            cx,
        );
    }

    pub(super) fn sync_verify_node(&self) -> VlessNode {
        crate::core::verify::node_for_ips(&self.verify_nodes)
    }

    pub(super) fn export_picks(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.verify_records.is_empty() {
            self.fail("没有可导出的验证结果", cx);
            return;
        }
        match storage::write_verify_picks(&self.paths, &self.verify_records) {
            Ok(paths) => self.show_export_toast(&paths, cx),
            Err(err) => self.fail(format!("导出失败：{err}"), cx),
        }
    }

    pub(super) fn start_daily(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        if let Err(err) = self.start_daily_inner(cx) {
            self.fail(err, cx);
        }
    }

    pub(super) fn start_daily_inner(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        self.launch_daily(true, cx)
    }

    /// 启动每日优选。`show_page` 为 false 时在后台启动（定时触发），不切换当前页面。
    pub(super) fn launch_daily(
        &mut self,
        show_page: bool,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if self.busy() {
            return Err("已有任务在运行，请等待完成或先停止".into());
        }
        if self.awaiting_nodes() {
            return Err("订阅节点正在载入，载入后再运行才能走隧道验证".into());
        }
        let advanced = self.settings.advanced_mode;
        let (segments, test, top, concurrency) = if advanced {
            let segments =
                parse_range::<usize>(self.daily_segments.read(cx).content(), "每轮段数", 1, 200)?;
            let test =
                parse_range::<usize>(self.daily_test.read(cx).content(), "测速数量", 1, 500)?;
            let top = parse_range::<usize>(self.daily_top.read(cx).content(), "Top N", 1, 100)?;
            let concurrency = parse_range::<u32>(
                self.daily_concurrency.read(cx).content(),
                "扫描并发",
                1,
                1000,
            )?;
            (segments, test, top, concurrency)
        } else {
            let settings = &self.settings;
            (
                settings.daily_segments,
                settings.daily_test,
                settings.daily_top,
                settings.daily_concurrency,
            )
        };
        let node = self.sync_verify_node();
        self.commit_region_lock(cx);
        let scan_timeout = self.settings.scan_timeout_secs.clamp(1.0, 30.0);
        self.settings.daily_segments = segments;
        self.settings.daily_test = test;
        self.settings.daily_top = top;
        self.settings.daily_concurrency = concurrency;
        self.save_settings();

        self.daily_logs.clear();
        self.daily_top_rows.clear();
        self.daily_summary = None;
        self.daily_stage = "准备".into();
        self.daily_detail = "读取段池".into();
        self.daily_progress = ProgressState::default();
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(cancel.clone());
        self.job = Some(JobKind::Daily);
        if show_page {
            self.page = Page::Daily;
            self.reset_page_scroll();
        }
        let config = crate::core::daily::DailyConfig {
            paths: self.paths.clone(),
            state: self.daily_state.clone(),
            segments,
            test,
            top,
            concurrency: concurrency as usize,
            scan_timeout: Duration::from_secs_f64(scan_timeout),
            node,
            strategy: self.settings.pool_strategy.clone(),
            scores: self.segment_scores.records.clone(),
            region_lock: crate::core::region::RegionLock::parse(&self.settings.region_lock),
        };
        crate::jobs::spawn_daily(config, self.events_tx.clone(), cancel);
        self.notify_status(
            if advanced {
                "每日优选已启动"
            } else {
                "自动优选已启动，完成后会显示 Top 结果"
            },
            Tone::Accent,
            cx,
        );
        Ok(())
    }

    pub(super) fn rebuild_pool(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.busy() {
            self.fail("已有任务在运行", cx);
            return;
        }
        self.aux_job = Some(JobKind::PoolRebuild);
        crate::jobs::spawn_pool_rebuild(self.paths.clone(), self.events_tx.clone());
        self.notify_status("正在重建 AS906 段池…", Tone::Accent, cx);
    }

    pub(super) fn toggle_auto_daily(
        &mut self,
        _: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.settings.auto_daily {
            self.settings.auto_daily = false;
            self.save_settings();
            self.notify_status("已关闭每日自动优选", Tone::Muted, cx);
            return;
        }
        match parse_hhmm(self.daily_auto_time.read(cx).content()) {
            Ok((hour, minute)) => {
                self.settings.auto_daily = true;
                self.daily_auto_time.update(cx, |input, cx| {
                    input.set_content(format!("{hour:02}:{minute:02}"));
                    cx.notify();
                });
                let deferred = self.schedule_auto_daily(hour, minute);
                // 定时运行要求应用一直开着：顺带开启托盘常驻，关闭窗口后留在托盘。
                let tray_added = self.ensure_tray(window);
                let mut text = format!("已开启每日自动优选：每天 {hour:02}:{minute:02}");
                if deferred {
                    text.push_str("；今天已过该时间，从明天开始");
                }
                if tray_added {
                    text.push_str("；已同时开启托盘常驻，关闭窗口后在托盘继续运行");
                }
                self.notify_status(text, Tone::Green, cx);
            }
            Err(err) => self.fail(err, cx),
        }
    }

    /// 参数区标题旁的「恢复推荐参数」，参数已是推荐值时不显示。
    pub(super) fn restore_params_button(
        &self,
        group: ParamGroup,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        if self.settings.params_recommended(group) {
            return None;
        }
        let id = match group {
            ParamGroup::Scan => "restore-scan-params",
            ParamGroup::Speed => "restore-speed-params",
            ParamGroup::Daily => "restore-daily-params",
        };
        Some(
            widgets::button(id, "恢复推荐参数", "refresh", widgets::ButtonKind::Ghost)
                .on_click(
                    cx.listener(move |this, _: &ClickEvent, _, cx| this.restore_params(group, cx)),
                )
                .into_any_element(),
        )
    }

    /// 把一组参数恢复为推荐值，并同步到高级模式的输入框。
    pub(super) fn restore_params(&mut self, group: ParamGroup, cx: &mut Context<Self>) {
        self.settings.reset_params(group);
        self.save_settings();
        let settings = self.settings.clone();
        let fields: Vec<(&Entity<TextInput>, String)> = match group {
            ParamGroup::Scan => vec![
                (&self.scan_ports, settings.scan_ports.clone()),
                (
                    &self.scan_concurrency,
                    settings.scan_concurrency.to_string(),
                ),
                (&self.scan_timeout, float_text(settings.scan_timeout_secs)),
                (&self.scan_sample, float_text(settings.scan_sample_pct)),
            ],
            ParamGroup::Speed => vec![
                (&self.speed_coarse, settings.speed_coarse_secs.to_string()),
                (&self.speed_fine, settings.speed_fine_secs.to_string()),
                (&self.speed_rounds, settings.speed_fine_rounds.to_string()),
                (&self.speed_topn, settings.speed_fine_topn.to_string()),
            ],
            ParamGroup::Daily => vec![
                (&self.daily_segments, settings.daily_segments.to_string()),
                (&self.daily_test, settings.daily_test.to_string()),
                (&self.daily_top, settings.daily_top.to_string()),
                (
                    &self.daily_concurrency,
                    settings.daily_concurrency.to_string(),
                ),
            ],
        };
        for (input, value) in fields {
            input.update(cx, |input, cx| {
                input.set_content(value);
                cx.notify();
            });
        }
        self.notify_status("已恢复推荐参数", Tone::Green, cx);
    }

    pub(super) fn toggle_launch_at_login(
        &mut self,
        _: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let enabled = !self.settings.launch_at_login;
        if let Err(err) = crate::autostart::set_enabled(enabled) {
            return self.fail(format!("设置开机自启失败：{err}"), cx);
        }
        self.settings.launch_at_login = enabled;
        self.save_settings();
        if !enabled {
            return self.notify_status("已关闭开机自启", Tone::Muted, cx);
        }
        // 有托盘时开机后在托盘中待命，不弹出窗口。
        let tray_added = self.ensure_tray(window);
        self.notify_status(
            if !crate::tray::SUPPORTED {
                "已开启开机自启：登录后自动打开"
            } else if tray_added {
                "已开启开机自启，并同时开启托盘常驻：登录后在托盘中待命"
            } else {
                "已开启开机自启：登录后在托盘中待命"
            },
            Tone::Green,
            cx,
        );
    }

    pub(super) fn reset_daily(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.busy() {
            self.fail("任务运行中，无法重置进度", cx);
            return;
        }
        let armed = self
            .reset_armed_at
            .map(|at| at.elapsed() < Duration::from_secs(5))
            .unwrap_or(false);
        if !armed {
            self.reset_armed_at = Some(Instant::now());
            self.notify_status("再次点击「确认重置」将清零每日进度", Tone::Amber, cx);
            return;
        }
        self.reset_armed_at = None;
        self.daily_state.cursor = 0;
        self.daily_state.round = 1;
        self.daily_state.strategy = self.pool_strategy().to_string();
        self.daily_state.last_run.clear();
        self.daily_state.last_top.clear();
        storage::save_daily_state(&self.paths, &self.daily_state);
        self.sync_pool_cursor_input(cx);
        self.daily_top_rows.clear();
        self.daily_summary = None;
        self.daily_progress = ProgressState::default();
        self.daily_stage = "已重置".into();
        self.daily_detail = "进度已清零，下次将从段池开头扫描".into();
        self.publish_proxy_feeds();
        self.notify_status("每日进度已重置", Tone::Green, cx);
    }
}
