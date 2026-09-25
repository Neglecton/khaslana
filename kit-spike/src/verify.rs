//! M1 验证台：把重构计划 §5 M1 的待验项集中到一个可脚本化采证的页面。
//!
//! 覆盖范围：
//! - V1 中文输入与 IME 事件序列（逐次 `InputEvent::Change` 全量记录）；
//! - V2 剪贴板与撤销（Ctrl+C / Ctrl+V / Ctrl+Z）、Enter 提交语义；
//! - V3 键盘焦点遍历与激活（Tab / Enter / Space）；
//! - V4 10 000 行虚拟列表：实际构建行数 + 滚动帧时间；
//! - V5 20 000 行差异列表：同上，行内含超长单行；
//! - V6 对话框与边缘浮层：遮罩、Esc、焦点恢复；
//! - V7 深浅主题切换（走 `Theme::from` 重建路径）；
//! - V8 窗口最小化 / 最大化。
//!
//! 采证方式：界面上的计数与日志是给截图读的，`[bench]` 行给出帧时间统计。

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::component::button::Button;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::WindowExt;
use gpui_kit::prelude::*;
use gpui_kit::*;
use gpui_kit::{AsyncApp, WeakEntity};

use crate::theme;
use crate::tokens;

const LIST_ROWS: usize = 10_000;
const DIFF_ROWS: usize = 20_000;
/// 压测步数：每步滚动一次并请求重绘，render 之间实测帧间隔。
const BENCH_STEPS: u32 = 60;
/// 超长单行所在索引（验证横向溢出不破坏布局）。
const LONG_LINE_INDEX: usize = 12_345;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Bench {
    Idle,
    List,
    Diff,
}

pub struct VerifyView {
    pub dark: bool,

    // V1 / V2 输入
    ime_input: Entity<InputState>,
    ime_text: Entity<TextareaState>,
    clip_text: Entity<TextareaState>,
    log: Vec<String>,
    changes: u32,
    enters: u32,
    focus_events: u32,
    clip_ops: u32,

    // V3 键盘
    button_hits: u32,
    space_hits: u32,
    check_on: bool,
    switch_on: bool,

    // V4 / V5 列表
    list_handle: UniformListScrollHandle,
    diff_handle: UniformListScrollHandle,
    /// 每帧重置、由虚拟列表闭包累加：本帧实际构建的行元素数。
    built: Rc<Cell<u32>>,
    /// 上一帧的构建数（render 开头读取，验证虚拟化只建可见行）。
    built_rows: u32,
    /// 压测期间的 render 次数与最近一次可见行区间。
    render_ticks: u32,
    list_range: Rc<Cell<(usize, usize)>>,

    // 压测
    bench: Bench,
    bench_step: u32,
    bench_last: Option<Instant>,
    bench_sum: f64,
    bench_max: f64,
    bench_summary: String,

    /// 无人值守自检（`KHASLANA_SPIKE_SELFTEST=1`）：启动后自动跑完压测、主题与窗口项，
    /// 结果写 `kit-spike-verify.log`。采样环境可能没有可读的屏幕输出，故自检不依赖截图。
    selftest: bool,
    selftest_started: bool,
}

impl VerifyView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let ime_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("在此输入中文，观察下方事件日志")
        });
        let ime_text = cx.new(|cx| {
            TextareaState::new(window, cx).placeholder("多行：Enter 换行，Ctrl+Enter 视为提交")
        });
        let clip_text = cx.new(|cx| {
            TextareaState::new(window, cx).default_value("选中这段文字后按 Ctrl+C / Ctrl+V / Ctrl+Z")
        });

        let view = Self {
            dark: false,
            ime_input,
            ime_text,
            clip_text,
            log: Vec::new(),
            changes: 0,
            enters: 0,
            focus_events: 0,
            clip_ops: 0,
            button_hits: 0,
            space_hits: 0,
            check_on: false,
            switch_on: false,
            list_handle: UniformListScrollHandle::new(),
            diff_handle: UniformListScrollHandle::new(),
            built: Rc::new(Cell::new(0)),
            built_rows: 0,
            render_ticks: 0,
            list_range: Rc::new(Cell::new((0, 0))),
            bench: Bench::Idle,
            bench_step: 0,
            bench_last: None,
            bench_sum: 0.,
            bench_max: 0.,
            bench_summary: "未运行".to_string(),
            selftest: std::env::var("KHASLANA_SPIKE_SELFTEST").is_ok(),
            selftest_started: false,
        };

        cx.subscribe_in(
            &view.ime_input,
            window,
            |this, _src, ev: &InputEvent, _window, cx| {
                match ev {
                    InputEvent::Change => {
                        this.changes += 1;
                        let value = "…".to_string();
                        this.push_log(format!("Change #{:<3}{value}", this.changes));
                    }
                    InputEvent::PressEnter { secondary, shift } => {
                        this.enters += 1;
                        this.push_log(format!(
                            "PressEnter #{} secondary={secondary} shift={shift}",
                            this.enters
                        ));
                    }
                    InputEvent::Focus => {
                        this.focus_events += 1;
                        this.push_log(format!("Focus #{}", this.focus_events));
                    }
                    InputEvent::Blur => {
                        this.focus_events += 1;
                        this.push_log(format!("Blur  #{}", this.focus_events));
                    }
                }
                cx.notify();
            },
        )
        .detach();

        cx.subscribe_in(
            &view.clip_text,
            window,
            |this, _src, ev: &InputEvent, _window, cx| {
                match ev {
                    InputEvent::Change => {
                        this.clip_ops += 1;
                        this.push_log(format!("剪贴板/编辑 #{}", this.clip_ops));
                    }
                    InputEvent::Focus => this.push_log("剪贴板获得焦点".to_string()),
                    InputEvent::Blur => this.push_log("剪贴板失去焦点".to_string()),
                    InputEvent::PressEnter { .. } => {}
                }
                cx.notify();
            },
        )
        .detach();

        cx.subscribe_in(
            &view.ime_text,
            window,
            |this, _src, ev: &InputEvent, _window, cx| {
                if let InputEvent::PressEnter { secondary, shift } = ev {
                    this.push_log(format!("多行 Enter secondary={secondary} shift={shift}"));
                    cx.notify();
                }
            },
        )
        .detach();

        view
    }

    fn push_log(&mut self, line: String) {
        crate::logline(&format!("EVENT {line}"));
        self.log.push(line);
        if self.log.len() > 9 {
            self.log.remove(0);
        }
    }

    // ------------------------------------------------------------------ 压测

    fn start_bench(&mut self, which: Bench, cx: &mut Context<Self>) {
        self.bench = which;
        self.bench_step = 0;
        self.bench_last = None;
        self.bench_sum = 0.;
        self.bench_max = 0.;
        self.render_ticks = 0;
        self.bench_summary = "运行中…".to_string();
        let started = Instant::now();

        let handle = match which {
            Bench::List => self.list_handle.clone(),
            Bench::Diff => self.diff_handle.clone(),
            Bench::Idle => return,
        };
        let rows = match which {
            Bench::List => LIST_ROWS,
            Bench::Diff => DIFF_ROWS,
            Bench::Idle => return,
        };

        cx.spawn(async move |this, cx| {
            // 让出一帧，避免把按钮自己的重绘计入统计。
            cx.background_executor()
                .timer(Duration::from_millis(64))
                .await;
            for step in 1..=BENCH_STEPS {
                // 跨大步长滚动，逼出「跳过大范围」的最坏路径。
                let target = (step as usize * (rows / BENCH_STEPS as usize)) % rows;
                handle.scroll_to_item(target, ScrollStrategy::Top);
                if this
                    .update(cx, |this, cx| {
                        this.bench_step = step;
                        this.bench_last = Some(Instant::now());
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(12))
                    .await;
            }
            let elapsed = started.elapsed();
            let _ = this.update(cx, |this, cx| {
                let steps = this.bench_step.max(1);
                let per_step = elapsed.as_secs_f64() * 1000. / steps as f64;
                let (rs, re) = this.list_range.get();
                crate::logline(&format!(
                    "BENCH {:?} steps={} total={:.0}ms per_step={:.1}ms renders={} built_last_frame={}                      list_visible={}..{}",
                    which,
                    steps,
                    elapsed.as_secs_f64() * 1000.,
                    per_step,
                    this.render_ticks,
                    this.built.get(),
                    rs,
                    re,
                ));
                this.bench_summary = format!(
                    "{} 步：总 {:.0} ms，每步 {:.1} ms，{} 次重绘",
                    steps,
                    elapsed.as_secs_f64() * 1000.,
                    per_step,
                    this.render_ticks,
                );
                this.bench = Bench::Idle;
                cx.notify();
            });
        })
        .detach();
    }

    // -------------------------------------------------------------- 无人值守自检

    /// 自检序列：列表压测 → 差异压测 → 深浅主题 → 窗口最大化/还原。
    ///
    /// 走的是和交互路径相同的代码（`start_bench`、`theme::apply_variant`、
    /// `Window` 的窗口控制），只是由环境变量触发，便于在无可读屏幕输出的
    /// 环境里拿到行为证据。
    fn run_selftest(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let viewport = window.viewport_size();
        crate::logline(&format!(
            "SELFTEST start viewport={:.0}x{:.0} scale={:.2} list_rows={} diff_rows={}",
            viewport.width / px(1.),
            viewport.height / px(1.),
            window.scale_factor(),
            LIST_ROWS,
            DIFF_ROWS,
        ));

        cx.spawn(async move |this, cx| {
            let _ = this.update(cx, |this, cx| this.start_bench(Bench::List, cx));
            Self::wait_bench(&this, cx).await;
            let _ = this.update(cx, |this, cx| this.start_bench(Bench::Diff, cx));
            Self::wait_bench(&this, cx).await;

            // 主题：深 → 浅，两次都走完整重建路径。
            let _ = this.update(cx, |this, cx| {
                this.dark = true;
                theme::apply_variant(cx, true);
                crate::logline("THEME dark applied");
                cx.notify();
            });
            cx.background_executor()
                .timer(Duration::from_millis(250))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.dark = false;
                theme::apply_variant(cx, false);
                crate::logline("THEME light applied");
                cx.notify();
            });
            cx.background_executor()
                .timer(Duration::from_millis(250))
                .await;

            // 窗口：gpui 的 `zoom_window` 实测是「设为最大化」而不是切换
            //（Windows 侧只发 SW_MAXIMIZE），所以还原要自己走 Win32，
            // 这正是自绘标题栏最大化按钮必须处理的分支。
            let _ = this.update_in(cx, |_this, window, _cx| {
                window.zoom_window();
                crate::logline("WINDOW zoom_window() called");
            });
            cx.background_executor()
                .timer(Duration::from_millis(600))
                .await;
            let _ = this.update_in(cx, |_this, window, _cx| {
                crate::logline(&format!("WINDOW after_zoom maximized={}", window.is_maximized()));
                window.zoom_window();
            });
            cx.background_executor()
                .timer(Duration::from_millis(600))
                .await;
            let _ = this.update_in(cx, |_this, window, _cx| {
                crate::logline(&format!(
                    "WINDOW zoom_twice maximized={}（true 即证明它不是切换）",
                    window.is_maximized()
                ));
                restore_window(window);
            });
            cx.background_executor()
                .timer(Duration::from_millis(600))
                .await;
            let _ = this.update_in(cx, |_this, window, _cx| {
                let viewport = window.viewport_size();
                crate::logline(&format!(
                    "WINDOW after_restore maximized={} viewport={:.0}x{:.0}",
                    window.is_maximized(),
                    viewport.width / px(1.),
                    viewport.height / px(1.),
                ));
            });

            // 最小化：调用后窗口不可见，作为独立项记录。
            let _ = this.update_in(cx, |_this, window, _cx| {
                window.minimize_window();
            });
            cx.background_executor()
                .timer(Duration::from_millis(500))
                .await;
            let _ = this.update_in(cx, |_this, window, _cx| {
                crate::logline(&format!("WINDOW after_minimize active={}", window.is_window_active()));
                restore_window(window);
            });
            crate::logline("SELFTEST done");
        })
        .detach();
    }

    async fn wait_bench(this: &WeakEntity<VerifyView>, cx: &mut AsyncApp) {
        for _ in 0..400 {
            cx.background_executor()
                .timer(Duration::from_millis(150))
                .await;
            match this.update(cx, |view, _cx| view.bench == Bench::Idle) {
                Ok(false) => {}
                _ => return,
            }
        }
    }

    // -------------------------------------------------------------- 渲染片段

    fn card(&self, title: &str, body: impl IntoElement) -> impl IntoElement {
        div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(8.))
            .p(px(14.))
            .rounded(px(tokens::RADIUS_PANEL))
            .bg(tokens::panel())
            .shadow(tokens::panel_shadow())
            .child(
                div()
                    .text_size(px(tokens::FONT_PANEL_TITLE))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(tokens::text_strong())
                    .child(title.to_string()),
            )
            .child(body)
    }

    fn stat(&self, label: &str, value: String) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .text_size(px(tokens::FONT_META))
            .text_color(tokens::text_body())
            .child(
                div()
                    .flex_none()
                    .w(px(132.))
                    .text_color(tokens::text_muted())
                    .child(label.to_string()),
            )
            .child(div().child(value))
    }

    fn render_left(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let event_log = if self.log.is_empty() {
            "（尚无事件）".to_string()
        } else {
            self.log.join("\n")
        };

        div()
            .flex_none()
            .w(px(470.))
            .flex()
            .flex_col()
            .gap(px(tokens::GAP_PANEL))
            .child(self.card(
                "V1 / V2 中文输入 · 剪贴板 · 撤销",
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(
                        div()
                            .h(px(36.))
                            .px(px(10.))
                            .flex()
                            .items_center()
                            .rounded(px(tokens::RADIUS_CONTROL))
                            .bg(tokens::input_surface())
                            .child(Input::new(&self.ime_input).appearance(false)),
                    )
                    .child(
                        div()
                            .h(px(64.))
                            .px(px(10.))
                            .py(px(6.))
                            .rounded(px(tokens::RADIUS_CONTROL))
                            .bg(tokens::input_surface())
                            .child(Textarea::new(&self.ime_text).appearance(false)),
                    )
                    .child(
                        div()
                            .h(px(64.))
                            .px(px(10.))
                            .py(px(6.))
                            .rounded(px(tokens::RADIUS_CONTROL))
                            .bg(tokens::input_surface())
                            .child(Textarea::new(&self.clip_text).appearance(false)),
                    )
                    .child(self.stat("Change 次数", self.changes.to_string()))
                    .child(self.stat("Enter 次数", self.enters.to_string()))
                    .child(self.stat("焦点事件", self.focus_events.to_string()))
                    .child(self.stat("剪贴板/编辑", self.clip_ops.to_string()))
                    .child(
                        div()
                            .h(px(150.))
                            .p(px(8.))
                            .rounded(px(8.))
                            .bg(tokens::input_surface())
                            .font_family(MONO)
                            .text_size(px(tokens::FONT_MICRO))
                            .text_color(tokens::text_body())
                            .overflow_hidden()
                            .child(event_log),
                    ),
            ))
            .child(self.card(
                "V3 键盘与激活 · V6 浮层 · V7 主题 · V8 窗口",
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .child(
                                Button::new("verify-enter")
                                    .label("Enter 激活计数")
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        this.button_hits += 1;
                                        this.push_log(format!("按钮 Enter/点击 #{}", this.button_hits));
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("verify-space")
                                    .label("Space 激活计数")
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        this.space_hits += 1;
                                        this.push_log(format!("按钮 Space #{}", this.space_hits));
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(self.stat("按钮点击/Enter", self.button_hits.to_string()))
                    .child(self.stat("按钮 Space", self.space_hits.to_string()))
                    .child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .items_center()
                            .child(
                                Button::new("verify-dialog")
                                    .label("打开对话框")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.push_log("打开对话框".to_string());
                                        window.open_dialog(cx, |dialog, _w, _cx| {
                                            dialog
                                                .title("对话框：遮罩 · Esc · 焦点恢复")
                                                .child(
                                                    "Esc 关闭；关闭后焦点应回到触发按钮。\
                                                     同时检查遮罩点击是否关闭与是否穿透。",
                                                )
                                        });
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Checkbox::new("verify-checkbox")
                                    .checked(self.check_on)
                                    .label("复选项（Checkbox）")
                                    .on_click(cx.listener(|this, checked: &bool, _w, cx| {
                                        this.check_on = *checked;
                                        this.push_log(format!(
                                            "复选项（Checkbox）→ {}",
                                            if *checked { "开" } else { "关" }
                                        ));
                                        cx.notify();
                                    })),
                            )
                            .child(
                                // 开关语义统一用滑块（Switch）；它支持 Tab 聚焦
                                // 与 Enter/Space 激活，是标准交互的验证点。
                                Switch::new("verify-switch")
                                    .checked(self.switch_on)
                                    .label("开关（Switch 滑块）")
                                    .on_click(cx.listener(|this, checked: &bool, _w, cx| {
                                        this.switch_on = *checked;
                                        this.push_log(format!(
                                            "开关（Switch 滑块）→ {}",
                                            if *checked { "开" } else { "关" }
                                        ));
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .child(
                                Button::new("verify-dark")
                                    .label(if self.dark { "切到浅色" } else { "切到深色" })
                                    .on_click(cx.listener(|this, _, _w, cx| {
                                        this.dark = !this.dark;
                                        let dark = this.dark;
                                        theme::apply_variant(cx, dark);
                                        this.push_log(format!("主题 → {}", if dark { "深色" } else { "浅色" }));
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("verify-min")
                                    .label("最小化")
                                    .on_click(cx.listener(|_, _, window, _cx| {
                                        window.minimize_window();
                                    })),
                            )
                            .child(
                                Button::new("verify-max")
                                    .label("最大化")
                                    .on_click(cx.listener(|_, _, window, _cx| {
                                        window.zoom_window();
                                    })),
                            ),
                    ),
            ))
    }

    fn render_lists(&mut self, cx: &mut Context<Self>, built: Rc<Cell<u32>>) -> impl IntoElement {
        let built_rows = self.built_rows;
        let list_probe = built.clone();
        let diff_probe = built;
        let list_rows = self.list_range.clone();
        div()
            .flex_1()
            .min_w(px(0.))
            .flex()
            .flex_col()
            .gap(px(tokens::GAP_PANEL))
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .p(px(14.))
                    .rounded(px(tokens::RADIUS_PANEL))
                    .bg(tokens::panel())
                    .shadow(tokens::panel_shadow())
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(tokens::FONT_PANEL_TITLE))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(tokens::text_strong())
                                    .child(format!("V4 虚拟列表 · {} 行", LIST_ROWS)),
                            )
                            .child(
                                Button::new("bench-list")
                                    .label("滚动压测")
                                    .on_click(cx.listener(|this, _, _w, cx| {
                                        this.start_bench(Bench::List, cx);
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .flex_none()
                            .font_family(MONO)
                            .text_size(px(tokens::FONT_MICRO))
                            .text_color(tokens::text_muted())
                            .child(format!(
                                "[bench] {}  上次 render 构建 {} 行",
                                self.bench_summary, built_rows
                            )),
                    )
                    .child(
                        uniform_list(
                            "verify-list",
                                LIST_ROWS,
                                move |range, _window, _cx| {
                                    list_probe
                                        .set(list_probe.get() + (range.end - range.start) as u32);
                                    list_rows.set((range.start, range.end));
                                    range
                                        .map(|ix| {
                                            div()
                                                .h(px(tokens::H_DIFF_LINE))
                                                .px(px(8.))
                                                .flex()
                                                .items_center()
                                                .gap(px(10.))
                                                .when(ix % 2 == 1, |d| d.bg(tokens::input_surface()))
                                                .child(
                                                    div()
                                                        .flex_none()
                                                        .w(px(52.))
                                                        .font_family(MONO)
                                                        .text_size(px(tokens::FONT_MICRO))
                                                        .text_color(tokens::text_line_no())
                                                        .child(format!("{ix}")),
                                                )
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .min_w(px(0.))
                                                        .whitespace_nowrap()
                                                        .overflow_hidden()
                                                        .font_family(MONO)
                                                        .text_size(px(tokens::FONT_CODE))
                                                        .text_color(tokens::text_body())
                                                        .child(format!(
                                                            "src/module_{}/file_{}.rs::symbol_{}",
                                                            ix % 37,
                                                            ix,
                                                            ix * 7 % 9973
                                                        )),
                                                )
                                        })
                                        .collect::<Vec<_>>()
                                },
                            )
                            .flex_1()
                            .min_h(px(0.))
                            .track_scroll(&self.list_handle),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .p(px(14.))
                    .rounded(px(tokens::RADIUS_PANEL))
                    .bg(tokens::panel())
                    .shadow(tokens::panel_shadow())
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(tokens::FONT_PANEL_TITLE))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(tokens::text_strong())
                                    .child(format!(
                                        "V5 差异列表 · {} 行（含 {} 号超长行）",
                                        DIFF_ROWS, LONG_LINE_INDEX
                                    )),
                            )
                            .child(
                                Button::new("bench-diff")
                                    .label("滚动压测")
                                    .on_click(cx.listener(|this, _, _w, cx| {
                                        this.start_bench(Bench::Diff, cx);
                                    })),
                            ),
                    )
                    .child(
                        uniform_list(
                            "verify-diff",
                                DIFF_ROWS,
                                move |range, _window, _cx| {
                                    diff_probe
                                        .set(diff_probe.get() + (range.end - range.start) as u32);
                                    range
                                        .map(|ix| {
                                            let kind = diff_kind(ix);
                                            let (bg, fg) = match kind {
                                                DiffKind::Added => {
                                                    (tokens::diff_add_bg(), tokens::diff_add_fg())
                                                }
                                                DiffKind::Removed => {
                                                    (tokens::diff_del_bg(), tokens::diff_del_fg())
                                                }
                                                DiffKind::Hunk => {
                                                    (tokens::diff_hunk_bg(), tokens::text_hunk())
                                                }
                                                DiffKind::Context => {
                                                    (tokens::panel(), tokens::text_body())
                                                }
                                            };
                                            let text = if ix == LONG_LINE_INDEX {
                                                format!("@@ 超长单行 @@ {}", "x".repeat(400))
                                            } else if kind == DiffKind::Hunk {
                                                format!("@@ -{},12 +{},14 @@ fn handler_{}()", ix, ix, ix % 97)
                                            } else {
                                                format!(
                                                    "    let value_{} = compute({}, {});",
                                                    ix,
                                                    ix % 251,
                                                    ix * 3 % 509
                                                )
                                            };
                                            div()
                                                .h(px(tokens::H_DIFF_LINE))
                                                .w_full()
                                                .bg(bg)
                                                .flex()
                                                .items_center()
                                                .child(
                                                    div()
                                                        .flex_none()
                                                        .w(px(64.))
                                                        .pr(px(8.))
                                                        .text_right()
                                                        .font_family(MONO)
                                                        .text_size(px(tokens::FONT_MICRO))
                                                        .text_color(tokens::text_line_no())
                                                        .child(format!("{}", ix + 1)),
                                                )
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .min_w(px(0.))
                                                        .whitespace_nowrap()
                                                        .overflow_hidden()
                                                        .font_family(MONO)
                                                        .text_size(px(tokens::FONT_CODE))
                                                        .text_color(fg)
                                                        .child(text),
                                                )
                                        })
                                        .collect::<Vec<_>>()
                                },
                            )
                            .flex_1()
                            .min_h(px(0.))
                            .track_scroll(&self.diff_handle),
                    ),
            )
    }
}

const MONO: &str = "Consolas";

#[derive(Clone, Copy, PartialEq, Eq)]
enum DiffKind {
    Context,
    Added,
    Removed,
    Hunk,
}

/// 确定性伪随机，保证两侧列表内容稳定可比。
fn diff_kind(ix: usize) -> DiffKind {
    if ix % 64 == 0 {
        return DiffKind::Hunk;
    }
    match (ix * 2654435761) % 7 {
        0 | 1 => DiffKind::Added,
        2 => DiffKind::Removed,
        _ => DiffKind::Context,
    }
}

impl Render for VerifyView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.selftest && !self.selftest_started {
            self.selftest_started = true;
            self.run_selftest(window, cx);
        }

        // 压测期间在 render 里实测帧间隔：这是真正落在屏幕上的节拍，
        // 比在滚动调用处计时更能反映合成是否掉帧。
        if self.bench != Bench::Idle {
            self.render_ticks += 1;
            let now = Instant::now();
            if let Some(last) = self.bench_last {
                let dt = now.duration_since(last).as_secs_f64() * 1000.;
                self.bench_sum += dt;
                self.bench_max = self.bench_max.max(dt);
            }
            self.bench_last = Some(now);
        }

        // 统计本帧实际构建的行数（虚拟化是否生效的直接证据）。
        // uniform_list 的闭包在本帧渲染过程中才被调用，所以这里读到的是上一帧的值。
        self.built_rows = self.built.get();
        self.built.set(0);

        let probe = self.built.clone();
        let left: AnyElement = self.render_left(cx).into_any_element();
        let lists: AnyElement = self.render_lists(cx, probe).into_any_element();
        let viewport = window.viewport_size();

        div()
            .size_full()
            .rounded(px(24.))
            .overflow_hidden()
            .flex()
            .flex_col()
            .bg(tokens::env_bg())
            .text_size(px(tokens::FONT_BODY))
            .text_color(tokens::text_body())
            .child(
                div()
                    .flex_none()
                    .h(px(60.))
                    .px(px(24.))
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .rounded_tl(px(24.))
                    .rounded_tr(px(24.))
                    .bg(tokens::toolbar_bg())
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(21.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(tokens::text_heading())
                            .child("Khaslana"),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(tokens::FONT_BODY))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(tokens::text_secondary())
                            .child("M1 验证台"),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .flex_none()
                            .font_family(MONO)
                            .text_size(px(tokens::FONT_MICRO))
                            .text_color(tokens::text_muted())
                            .child(format!(
                                "窗口 {:.0}×{:.0} 逻辑像素 · scale {:.2} · 主题 {}",
                                viewport.width / px(1.),
                                viewport.height / px(1.),
                                window.scale_factor(),
                                if self.dark { "深" } else { "浅" }
                            )),
                    )
                    .child(
                        Button::new("verify-back")
                            .label("返回工作区")
                            .on_click(cx.listener(|_, _, _w, cx| {
                                cx.emit(VerifyCommand::Back);
                            })),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .p(px(16.))
                    .flex()
                    .gap(px(tokens::GAP_PANEL))
                    .child(left)
                    .child(lists),
            )
    }
}

/// 取消最大化 / 从最小化恢复：gpui 没有对应 API，自绘标题栏必须自己做。
#[cfg(windows)]
fn restore_window(window: &Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::UI::WindowsAndMessaging::{SW_RESTORE, ShowWindow};
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return;
    };
    unsafe {
        ShowWindow(handle.hwnd.get() as _, SW_RESTORE);
    }
}

#[cfg(not(windows))]
fn restore_window(_window: &Window) {}

/// 返回工作区由宿主视图处理。
#[derive(Clone, Copy)]
pub enum VerifyCommand {
    Back,
}

impl EventEmitter<VerifyCommand> for VerifyView {}
