//! 段池策略、起点游标与粗测。

use super::*;

impl AppState {
    pub(super) fn pool_strategy(&self) -> &'static str {
        pool::normalize_strategy(&self.settings.pool_strategy)
    }

    pub(super) fn rebuild_pool_order(&mut self) {
        self.pool_order = pool::ordered_pool(
            &self.pool_segments,
            &self.settings.pool_strategy,
            &self.segment_scores.records,
        );
        self.score_map = self
            .segment_scores
            .records
            .iter()
            .map(|record| (record.segment.clone(), record.score))
            .collect();
    }

    pub(super) fn sync_pool_cursor_input(&mut self, cx: &mut Context<Self>) {
        let len = self.pool_order.len();
        let cursor = pool::normalize_cursor(self.daily_state.cursor, len);
        if len > 0 && cursor != self.daily_state.cursor {
            self.daily_state.cursor = cursor;
        }
        let text = if len == 0 {
            String::new()
        } else {
            (cursor + 1).to_string()
        };
        self.pool_cursor
            .update(cx, |input, _| input.set_content(text));
    }

    /// 下一轮要扫描的段。`follow_lock` 为 true 时按锁定区域的单独轮转（自动优选页），
    /// 否则按整个段池（段池页管理的就是整个段池的起点）。
    pub(super) fn next_batch_preview(&self, follow_lock: bool) -> String {
        let (order, cursor, _, locked) = if follow_lock {
            self.effective_rotation()
        } else {
            (
                self.pool_order.clone(),
                self.daily_state.cursor,
                self.daily_state.round,
                false,
            )
        };
        let len = order.len();
        if len == 0 {
            return if locked {
                "段池里没有落在锁定区域的段".to_string()
            } else {
                "段池为空".to_string()
            };
        }
        let cursor = pool::normalize_cursor(cursor, len);
        let batch = pool::batch_size(self.settings.daily_segments, len);
        let positions = pool::batch_positions(cursor, batch, len);
        let segments = pool::batch_segments(&order, cursor, batch);
        let preview = segments
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join("、");
        let more = if segments.len() > 3 {
            format!(" 等 {} 段", segments.len())
        } else {
            String::new()
        };
        format!("{positions} · {preview}{more}")
    }

    pub(super) fn apply_pool_strategy(&mut self, strategy: &str, cx: &mut Context<Self>) {
        if self.busy() {
            self.fail("任务运行中，完成后再切换段池策略", cx);
            return;
        }
        let strategy = pool::normalize_strategy(strategy);
        if strategy == self.pool_strategy() {
            self.notify_status(
                format!("当前已是「{}」", pool::strategy_label(strategy)),
                Tone::Muted,
                cx,
            );
            return;
        }
        self.settings.pool_strategy = strategy.to_string();
        self.daily_state.cursor = 0;
        self.daily_state.round = 1;
        self.daily_state.strategy = strategy.to_string();
        self.save_settings();
        storage::save_daily_state(&self.paths, &self.daily_state);
        self.rebuild_pool_order();
        self.sync_pool_cursor_input(cx);
        let has_scores = self
            .segment_scores
            .records
            .iter()
            .any(|record| record.score > 0.0);
        let message = if strategy == "network" && !has_scores {
            "已切换为「网络优选」；尚无粗测结果，暂按均衡顺序推进，建议先做一次粗测".to_string()
        } else {
            format!(
                "段池策略已切换为「{}」 · 游标回到起点",
                pool::strategy_label(strategy)
            )
        };
        self.notify_status(
            message,
            if strategy == "network" && !has_scores {
                Tone::Amber
            } else {
                Tone::Green
            },
            cx,
        );
    }

    pub(super) fn reset_pool_recommended(
        &mut self,
        _: &ClickEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy() {
            self.fail("任务运行中，完成后再调整起点", cx);
            return;
        }
        self.settings.pool_strategy = pool::DEFAULT_STRATEGY.to_string();
        self.daily_state.cursor = 0;
        self.daily_state.round = 1;
        self.daily_state.strategy = pool::DEFAULT_STRATEGY.to_string();
        self.save_settings();
        storage::save_daily_state(&self.paths, &self.daily_state);
        self.rebuild_pool_order();
        self.sync_pool_cursor_input(cx);
        self.notify_status(
            format!("已恢复推荐起点：均衡轮转 · {}", pool::SEED_SEGMENT),
            Tone::Green,
            cx,
        );
    }

    pub(super) fn apply_pool_cursor_inner(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        let len = self.pool_order.len();
        if len == 0 {
            return Err("段池为空：先运行一次自动优选，或点击「重建段池」".into());
        }
        let index = parse_range::<usize>(self.pool_cursor.read(cx).content(), "起始序号", 1, len)?;
        let batch =
            parse_range::<usize>(self.daily_segments.read(cx).content(), "每轮段数", 1, len)?;
        self.daily_state.cursor = index - 1;
        self.settings.daily_segments = batch;
        self.save_settings();
        storage::save_daily_state(&self.paths, &self.daily_state);
        self.sync_pool_cursor_input(cx);
        Ok(())
    }

    pub(super) fn apply_pool_cursor(
        &mut self,
        _: &ClickEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.apply_pool_cursor_inner(cx) {
            Ok(()) => {
                let preview = self.next_batch_preview(false);
                self.notify_status(format!("起点已更新 · {preview}"), Tone::Green, cx);
            }
            Err(err) => self.fail(err, cx),
        }
    }

    pub(super) fn set_pool_start_index(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.busy() {
            self.fail("任务运行中，完成后再调整起点", cx);
            return;
        }
        let len = self.pool_order.len();
        if len == 0 {
            self.fail("段池为空", cx);
            return;
        }
        self.daily_state.cursor = index % len;
        storage::save_daily_state(&self.paths, &self.daily_state);
        self.sync_pool_cursor_input(cx);
        let message = format!(
            "起点已设为第 {} 段 · {}",
            self.daily_state.cursor + 1,
            self.pool_order
                .get(self.daily_state.cursor)
                .cloned()
                .unwrap_or_default()
        );
        self.notify_status(message, Tone::Green, cx);
    }

    pub(super) fn apply_pool_cursor_and_run(
        &mut self,
        _: &ClickEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(err) = self.apply_pool_cursor_inner(cx) {
            self.fail(err, cx);
            return;
        }
        if let Err(err) = self.start_daily_inner(cx) {
            self.fail(err, cx);
        }
    }

    pub(super) fn start_probe(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        if let Err(err) = self.start_probe_inner(cx) {
            self.fail(err, cx);
        }
    }

    pub(super) fn start_probe_inner(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        if self.busy() {
            return Err("已有任务在运行，请等待完成或先停止".into());
        }
        let pool_segments = storage::load_pool(&self.paths);
        if pool_segments.is_empty() {
            return Err("段池为空：先运行一次自动优选，或点击「重建段池」".into());
        }
        let samples =
            parse_range::<usize>(self.probe_samples.read(cx).content(), "每段抽样", 1, 16)?;
        let concurrency = parse_range::<u32>(
            self.probe_concurrency.read(cx).content(),
            "粗测并发",
            1,
            300,
        )?;
        let top = parse_range::<usize>(self.probe_top.read(cx).content(), "测速段数", 1, 32)?;
        let seconds =
            parse_range::<u64>(self.probe_seconds.read(cx).content(), "每段测速秒数", 1, 10)?;
        self.settings.probe_samples = samples;
        self.settings.probe_concurrency = concurrency;
        self.settings.probe_top = top;
        self.settings.probe_seconds = seconds;
        self.save_settings();

        if self.pool_segments != pool_segments {
            self.pool_segments = pool_segments.clone();
            self.daily_state.pool_count = pool_segments.len();
            self.daily_state.cursor =
                pool::normalize_cursor(self.daily_state.cursor, pool_segments.len());
            storage::save_daily_state(&self.paths, &self.daily_state);
            self.rebuild_pool_order();
            self.sync_pool_cursor_input(cx);
        }
        let targets = pool_segments.len() * samples;
        self.probe_progress = ProgressState {
            done: 0,
            total: targets,
            label: format!("准备粗测 {} 段 × {} 个抽样", pool_segments.len(), samples),
        };
        self.probe_stage = "准备".into();
        self.probe_detail = self.probe_progress.label.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(cancel.clone());
        self.job = Some(JobKind::Probe);
        self.page = Page::Pool;
        self.reset_page_scroll();
        let config = crate::core::probe::ProbeConfig {
            segments: pool_segments,
            samples,
            concurrency: concurrency as usize,
            timeout: Duration::from_secs_f64(self.settings.scan_timeout_secs.clamp(2.0, 8.0)),
            throughput_top: top,
            throughput_secs: seconds,
        };
        crate::jobs::spawn_probe(config, self.events_tx.clone(), cancel);
        self.notify_status(
            format!("段池粗测已启动 · 每段 {samples} 个抽样 · 并发 {concurrency}"),
            Tone::Accent,
            cx,
        );
        Ok(())
    }
}
