//! 后台任务事件处理：把 JobEvent 落到 AppState。

use super::*;

impl AppState {
    pub(super) fn handle_event(&mut self, event: JobEvent, cx: &mut Context<Self>) {
        match event {
            JobEvent::ScanProgress {
                done,
                total,
                hits,
                elapsed_ms,
            } => {
                self.scan_progress.done = done;
                self.scan_progress.total = total;
                self.scan_progress.label = format!(
                    "{done}/{total} · 命中 {hits} · {:.0}s",
                    elapsed_ms as f64 / 1000.0
                );
            }
            JobEvent::ScanHit(hit) => {
                let first = self.scan_hits.is_empty();
                self.scan_hits.push(hit);
                if first && self.page == Page::Scan && self.job == Some(JobKind::Scan) {
                    self.main_scroll.scroll_to_bottom();
                }
            }
            JobEvent::ScanDone(outcome) => {
                // 手动扫描的命中也记下所在段落到的机房，补充出口区域划分。
                if !outcome.hits.is_empty() {
                    let mut regions = storage::load_segment_regions(&self.paths);
                    regions.learn(
                        &outcome.hits,
                        &Local::now().format("%Y-%m-%d %H:%M").to_string(),
                    );
                    storage::save_segment_regions(&self.paths, &regions);
                    self.refresh_regions();
                }
                self.scan_hits = outcome.hits;
                self.scan_progress.done = outcome.done;
                self.scan_progress.total = outcome.total;
                let elapsed = outcome.elapsed.as_secs_f64();
                let mut message = if outcome.stopped {
                    format!(
                        "扫描已停止：命中 {} / 已完成 {}，用时 {:.1}s",
                        self.scan_hits.len(),
                        outcome.done,
                        elapsed
                    )
                } else {
                    format!(
                        "扫描完成：命中 {} / {}，用时 {:.1}s",
                        self.scan_hits.len(),
                        outcome.total,
                        elapsed
                    )
                };
                if !self.scan_hits.is_empty() {
                    storage::save_last_scan(
                        &self.paths,
                        &crate::core::model::LastScan {
                            time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                            total: outcome.total,
                            hits: self.scan_hits.clone(),
                        },
                    );
                    match storage::write_scan_exports(&self.paths, &self.scan_hits) {
                        Ok(paths) => {
                            if let Some(path) = paths.first() {
                                message.push_str(&format!(
                                    " · 已导出 {}",
                                    path.file_name()
                                        .map(|name| name.to_string_lossy().to_string())
                                        .unwrap_or_default()
                                ));
                            }
                        }
                        Err(err) => storage::log_line(&format!("自动导出扫描结果失败：{err}")),
                    }
                }
                self.scan_note = message.clone();
                self.scan_progress.label = message.clone();
                let tone = if self.scan_hits.is_empty() || outcome.stopped {
                    Tone::Amber
                } else {
                    Tone::Green
                };
                self.notify_status(message, tone, cx);
            }
            JobEvent::SpeedPhase { name, total } => {
                self.speed_phase = name.to_string();
                self.speed_current = format!("{name} · {total} 个");
                self.speed_progress = ProgressState {
                    done: 0,
                    total,
                    label: format!("进入{name}阶段"),
                };
            }
            JobEvent::SpeedProgress {
                phase,
                done,
                total,
                throttled,
                current,
            } => {
                self.speed_phase = phase.to_string();
                self.speed_throttled = throttled;
                self.speed_progress.done = done;
                self.speed_progress.total = total;
                self.speed_current = current.unwrap_or_default();
                self.speed_progress.label = format!("{phase} {done}/{total}");
            }
            JobEvent::SpeedRecord(record) => {
                let first = self.speed_records.is_empty();
                self.speed_records.push(record);
                if first && self.page == Page::Speed && self.job == Some(JobKind::Speed) {
                    self.main_scroll.scroll_to_bottom();
                }
            }
            JobEvent::SpeedWarning(warning) => {
                if !self.speed_warnings.contains(&warning) {
                    self.speed_warnings.push(warning);
                }
            }
            JobEvent::SpeedDone(outcome) => {
                self.speed_records = outcome.records;
                self.speed_throttled = outcome.throttled;
                self.speed_phase = if outcome.stopped {
                    "已停止".into()
                } else {
                    "完成".into()
                };
                self.speed_current.clear();
                for warning in &outcome.warnings {
                    if !self.speed_warnings.contains(warning) {
                        self.speed_warnings.push(warning.clone());
                    }
                }
                let mut message = format!(
                    "测速完成：{} 个有效结果，{} 次 429，用时 {:.1}s",
                    self.speed_records.len(),
                    outcome.throttled,
                    outcome.elapsed.as_secs_f64()
                );
                if !self.speed_records.is_empty() {
                    match storage::write_speed_export(
                        &self.paths,
                        &self.speed_records,
                        outcome.throttled,
                    ) {
                        Ok(paths) => {
                            message.push_str(&format!(
                                " · 已导出 {}",
                                paths
                                    .iter()
                                    .filter_map(|path| path.file_name())
                                    .map(|name| name.to_string_lossy().to_string())
                                    .collect::<Vec<_>>()
                                    .join("、")
                            ));
                        }
                        Err(err) => storage::log_line(&format!("自动导出测速结果失败：{err}")),
                    }
                    self.history.speed.insert(
                        0,
                        crate::core::model::SpeedRun {
                            time: Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
                            throttled: outcome.throttled,
                            results: self.speed_records.clone(),
                        },
                    );
                    storage::trim_history(&mut self.history);
                    storage::save_history(&self.paths, &self.history);
                }
                self.speed_progress.label = message.clone();
                let tone = if outcome.stopped {
                    Tone::Amber
                } else {
                    Tone::Green
                };
                self.notify_status(message, tone, cx);
            }
            JobEvent::VerifyProgress {
                done,
                total,
                current,
            } => {
                self.verify_progress.done = done;
                self.verify_progress.total = total;
                self.verify_progress.label = format!("{done}/{total} · {current}");
            }
            JobEvent::VerifyRecord(record) => {
                let first = self.verify_records.is_empty();
                self.verify_records.push(record);
                if first && self.page == Page::Verify && self.job == Some(JobKind::Verify) {
                    self.main_scroll.scroll_to_bottom();
                }
            }
            JobEvent::VerifyDone(outcome) => {
                self.verify_records = outcome.records;
                self.verify_progress.done = self.verify_records.len();
                self.verify_progress.total =
                    self.verify_progress.total.max(self.verify_records.len());
                let mut message = format!(
                    "可用性验证完成：{}/{} 通过，用时 {:.1}s",
                    outcome.passed,
                    self.verify_records.len(),
                    outcome.elapsed.as_secs_f64()
                );
                if outcome.passed > 0 {
                    match storage::write_verify_picks(&self.paths, &self.verify_records) {
                        Ok(paths) => message.push_str(&format!(
                            " · 已导出 {}",
                            paths
                                .iter()
                                .filter_map(|path| path.file_name())
                                .map(|name| name.to_string_lossy().to_string())
                                .collect::<Vec<_>>()
                                .join("、")
                        )),
                        Err(err) => storage::log_line(&format!("导出 PICKS 失败：{err}")),
                    }
                }
                self.history.verify.insert(
                    0,
                    crate::core::model::VerifyRun {
                        time: Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
                        total: self.verify_records.len(),
                        passed: outcome.passed,
                        records: self.verify_records.clone(),
                    },
                );
                storage::trim_history(&mut self.history);
                storage::save_history(&self.paths, &self.history);
                self.verify_progress.label = message.clone();
                let tone = if outcome.stopped {
                    Tone::Amber
                } else if outcome.passed > 0 {
                    Tone::Green
                } else {
                    Tone::Red
                };
                self.notify_status(message, tone, cx);
            }
            JobEvent::DailyStage { stage, detail } => {
                self.daily_stage = stage;
                self.daily_detail = detail;
            }
            JobEvent::DailyProgress {
                stage,
                done,
                total,
                label,
            } => {
                self.daily_stage = stage;
                self.daily_progress.done = done;
                self.daily_progress.total = total;
                self.daily_progress.label = label.clone();
                self.daily_detail = label;
            }
            JobEvent::DailyLog(line) => {
                self.daily_logs.push_front(line);
                while self.daily_logs.len() > 300 {
                    self.daily_logs.pop_back();
                }
            }
            JobEvent::DailyDone(result) => {
                self.daily_state = result.state;
                let loaded_pool = storage::load_pool(&self.paths);
                if loaded_pool != self.pool_segments {
                    self.pool_segments = loaded_pool;
                    self.rebuild_pool_order();
                }
                // 优选会生成段池、首次锁定时划分区域、更新扫描过的段。
                self.refresh_regions();
                self.sync_pool_cursor_input(cx);
                self.daily_top_rows = result.top;
                self.daily_summary = Some(result.summary.clone());
                self.daily_stage = if result.stopped {
                    "已停止".into()
                } else {
                    "完成".into()
                };
                self.daily_progress.done = self.daily_progress.total;
                self.daily_progress.label = format!(
                    "Top {} · 命中 {} · {} · {:.0}s",
                    self.daily_top_rows.len(),
                    result.summary.hits,
                    result.summary.verification.label(),
                    result.elapsed.as_secs_f64()
                );
                self.daily_detail = self.daily_progress.label.clone();
                if result.summary.scanned > 0 {
                    self.history.daily.insert(
                        0,
                        crate::core::model::DailyRun {
                            time: Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
                            top: self.daily_top_rows.clone(),
                            scanned: result.summary.scanned,
                            hits: result.summary.hits,
                            tested: result.summary.tested,
                        },
                    );
                    storage::trim_history(&mut self.history);
                    storage::save_history(&self.paths, &self.history);
                }
                let mut message = if self.daily_top_rows.is_empty() {
                    format!(
                        "每日优选完成：本轮无可用 IP（命中 {}，测速 {}）",
                        result.summary.hits, result.summary.tested
                    )
                } else {
                    format!(
                        "每日优选完成：Top {}，扫描命中 {}",
                        self.daily_top_rows.len(),
                        result.summary.hits
                    )
                };
                message.push_str(&format!(" · {}", result.summary.verification.label()));
                if let Some(path) = &result.output_path {
                    message.push_str(&format!(
                        " · {}",
                        path.file_name()
                            .map(|name| name.to_string_lossy().to_string())
                            .unwrap_or_default()
                    ));
                }
                let tone = if result.stopped || self.daily_top_rows.is_empty() {
                    Tone::Amber
                } else {
                    Tone::Green
                };
                self.notify_in_background("每日优选完成", &message);
                self.notify_status(message, tone, cx);
            }
            JobEvent::AsnReady { prefixes, source } => {
                let existing = self.scan_input.read(cx).content().trim().to_string();
                let mut content = prefixes.join("\n");
                if !existing.is_empty() {
                    content = format!("{existing}\n{content}");
                }
                self.scan_input
                    .update(cx, |input, _| input.set_content(content));
                self.notify_status(
                    format!("已载入 {source}（{} 个前缀）", prefixes.len()),
                    Tone::Green,
                    cx,
                );
            }
            JobEvent::PoolReady { ips, source } => {
                let count = ips.len();
                self.speed_input
                    .update(cx, |input, _| input.set_content(ips.join("\n")));
                self.notify_status(
                    format!("候选池已载入：{count} 个 IP（{source}）"),
                    Tone::Green,
                    cx,
                );
            }
            JobEvent::SubscriptionReady { url, nodes } => {
                self.sub_loading = false;
                self.sub_cache = SubscriptionCache {
                    url,
                    time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                    nodes: nodes.clone(),
                };
                storage::save_subscription_cache(&self.paths, &self.sub_cache);
                let ws_count = nodes
                    .iter()
                    .filter(|node| node.transport.eq_ignore_ascii_case("ws"))
                    .count();
                let count = nodes.len();
                self.apply_subscription_nodes(nodes, None);
                self.notify_status(
                    format!("订阅已载入：{count} 个节点（{ws_count} 个 ws）"),
                    Tone::Green,
                    cx,
                );
            }
            JobEvent::SubscriptionFailed { url, error } => {
                self.sub_loading = false;
                // 同一订阅上次成功载入过：继续用那批节点，隧道验证与导入不受影响。
                match self.sub_cache.nodes_for(&url).map(<[_]>::to_vec) {
                    Some(nodes) => {
                        let time = self.sub_cache.time.clone();
                        let count = nodes.len();
                        self.apply_subscription_nodes(
                            nodes,
                            Some(&format!("沿用 {time} 载入的结果")),
                        );
                        self.notify_status(
                            format!(
                                "订阅拉取失败，继续使用 {time} 载入的 {count} 个节点（{error}）"
                            ),
                            Tone::Amber,
                            cx,
                        );
                    }
                    None => self.fail(format!("订阅失败：{error}"), cx),
                }
            }
            JobEvent::PoolRebuilt { count, source } => {
                self.pool_segments = storage::load_pool(&self.paths);
                let len = self.pool_segments.len();
                self.daily_state.pool_count = len;
                self.daily_state.cursor = 0;
                self.daily_state.round = 1;
                self.daily_state.strategy = self.pool_strategy().to_string();
                storage::save_daily_state(&self.paths, &self.daily_state);
                self.rebuild_pool_order();
                self.sync_pool_cursor_input(cx);
                self.notify_status(
                    format!(
                        "段池已重建：{} 个 /24（{source}）· 已回到推荐起点",
                        if len == 0 { count } else { len }
                    ),
                    Tone::Green,
                    cx,
                );
            }
            JobEvent::ProbePhase { name, total } => {
                self.probe_stage = name.to_string();
                self.probe_progress = ProgressState {
                    done: 0,
                    total,
                    label: format!("进入{name}阶段"),
                };
            }
            JobEvent::ProbeProgress {
                phase,
                done,
                total,
                current,
            } => {
                self.probe_stage = phase.to_string();
                self.probe_progress.done = done;
                self.probe_progress.total = total;
                self.probe_progress.label = format!("{phase} {done}/{total} · {current}");
                self.probe_detail = self.probe_progress.label.clone();
            }
            JobEvent::ProbeWarning(warning) => {
                storage::log_line(&warning);
                self.probe_detail = warning;
            }
            JobEvent::ProbeDone(outcome) => {
                self.segment_scores = SegmentScores {
                    time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                    records: outcome.records,
                };
                storage::save_segment_scores(&self.paths, &self.segment_scores);
                self.rebuild_pool_order();
                if pool::normalize_strategy(&self.settings.pool_strategy) == "network" {
                    self.daily_state.cursor = 0;
                    self.daily_state.round = 1;
                    storage::save_daily_state(&self.paths, &self.daily_state);
                    self.sync_pool_cursor_input(cx);
                }
                self.probe_stage = if outcome.stopped {
                    "已停止".into()
                } else {
                    "完成".into()
                };
                if !outcome.stopped {
                    self.probe_progress.done = self.probe_progress.total;
                }
                let alive = self
                    .segment_scores
                    .records
                    .iter()
                    .filter(|record| record.hits > 0)
                    .count();
                let message = format!(
                    "段池粗测完成：{} 段可用，{} 次握手命中，{} 段完成测速，用时 {:.1}s",
                    alive,
                    outcome.handshake_hits,
                    outcome.throughput_tested,
                    outcome.elapsed.as_secs_f64()
                );
                self.probe_detail = message.clone();
                self.probe_progress.label = message.clone();
                self.notify_status(
                    message,
                    if outcome.stopped {
                        Tone::Amber
                    } else {
                        Tone::Green
                    },
                    cx,
                );
            }
            JobEvent::ClassifyProgress(step) => {
                self.classify_progress = ProgressState {
                    done: step.probed,
                    total: step.total,
                    label: format!("已划分 {} / {} 段", step.resolved, step.segments),
                };
                self.classify_detail = format!(
                    "已探测 {} 个地址 · 已划分 {} / {} 段",
                    step.probed, step.resolved, step.segments
                );
            }
            JobEvent::ClassifyDone(outcome) => {
                let now = Local::now().format("%Y-%m-%d %H:%M").to_string();
                let mut regions = storage::load_segment_regions(&self.paths);
                crate::core::region::apply_classification(&mut regions, &outcome, &now);
                storage::save_segment_regions(&self.paths, &regions);
                self.refresh_regions();
                let spread = self
                    .region_countries
                    .iter()
                    .take(5)
                    .map(|country| format!("{} {} 段", country.name, country.segments))
                    .collect::<Vec<_>>()
                    .join("、");
                let message = format!(
                    "出口区域划分{}：{} 段中 {} 段有响应，用时 {:.0}s{}",
                    if outcome.stopped {
                        "已停止"
                    } else {
                        "完成"
                    },
                    outcome.segments.len(),
                    outcome.responded,
                    outcome.elapsed.as_secs_f64(),
                    if spread.is_empty() {
                        String::new()
                    } else {
                        format!(" · {spread}")
                    }
                );
                self.classify_progress.done = self.classify_progress.total;
                self.classify_detail = message.clone();
                self.notify_status(
                    message,
                    if outcome.stopped {
                        Tone::Amber
                    } else {
                        Tone::Green
                    },
                    cx,
                );
            }
            JobEvent::Error(message) => {
                if self.job == Some(JobKind::Daily) {
                    self.daily_logs.push_front(message.clone());
                }
                if self.job == Some(JobKind::Probe) {
                    self.probe_stage = "失败".into();
                    self.probe_detail = message.clone();
                }
                self.fail(message, cx);
            }
            JobEvent::Done(kind) => {
                self.job = None;
                self.cancel = None;
                if kind == JobKind::Scan && self.scan_progress.total > 0 {
                    self.scan_progress.label = self.scan_note.clone();
                }
                // 验证 / 自动优选的结果此时才完整，交给本机订阅服务。
                if matches!(kind, JobKind::Verify | JobKind::Daily) {
                    self.publish_proxy_feeds();
                }
            }
            JobEvent::AuxDone => {
                self.aux_job = None;
            }
            JobEvent::UpdateChecked(result) => self.on_update_checked(result, cx),
            JobEvent::UpdateProgress(progress) => self.on_update_progress(progress),
            JobEvent::UpdateDownloaded(result) => self.on_update_downloaded(result, cx),
        }
    }
}
