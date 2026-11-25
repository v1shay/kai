//! On-demand, process-isolated history analytics. The UI receives aggregates only.
use crate::appearance::GlassPopup as _;
use super::*;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default)]
struct Group {
    name: String,
    cost: f64,
    tokens: u64,
    sessions: u64,
    unpriced:u64,
}
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default)]
struct Bucket {
    at: f64,
    cost: f64,
    tokens: u64,
}
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default)]
struct Report {
    cost: f64,
    tokens: u64,
    input: u64,
    cache: u64,
    write: u64,
    output: u64,
    savings: f64,
    premium:f64,
    sessions: u64,
    unpriced: u64,
    providers: Vec<Group>,
    models: Vec<Group>,
    series: Vec<Bucket>,
    types: Vec<f64>,
    speeds: HashMap<String, f64>,
    warnings: Vec<String>,
}
pub(super) struct State {
    pub open: bool,
    busy: bool,
    environment: Option<String>,
    dropdown: Option<u8>,
    menu_at:Instant,menu_closing:bool,
    change_at: Instant,
    generation: u64,
    days: u32,
    provider: String,
    mode: u8,
    by_time: bool,
    report: Arc<Report>,
    error: Option<String>,
    at: Instant,
    closing: bool,
    hover: Option<usize>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            open: false,
            busy: false,
            environment: None,
            dropdown: None,menu_at:Instant::now(),menu_closing:false,
            change_at: Instant::now(),
            generation: 0,
            days: 7,
            provider: "All providers".into(),
            mode: 0,
            by_time: false,
            report: Arc::default(),
            error: None,
            at: Instant::now(),
            closing: false,
            hover: None,
        }
    }
}
fn compact(n: u64) -> String {
    if n >= 1_000_000_000 {
        format!("{:.2}B", n as f64 / 1e9)
    } else if n >= 1_000_000 {
        format!("{:.2}M", n as f64 / 1e6)
    } else if n >= 1000 {
        format!("{:.1}K", n as f64 / 1e3)
    } else {
        n.to_string()
    }
}
fn money(n: f64) -> String {
    format!("${n:.2}")
}
fn label(at: f64, hour: bool) -> String {
    chrono::DateTime::from_timestamp(at as i64, 0)
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format(if hour { "%I %p" } else { "%b %d" })
                .to_string()
        })
        .unwrap_or_default()
}
impl Orbit {
    pub(super) fn open_usage_dashboard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.usage_dashboard.open && !self.usage_dashboard.closing {
            self.close_usage_dashboard(cx);
            return;
        }
        self.dismiss_panel(window, cx);
        self.dismiss_agent_picker();
        self.action_menu = None;
        self.usage_dashboard.open = true;
        self.usage_dashboard.dropdown=None;self.usage_dashboard.menu_closing=false;
        self.usage_dashboard.closing = false;
        self.usage_dashboard.at = Instant::now();
        self.overlay_window = Some(window.window_handle().window_id());
        self.focus.focus(window);
        if !cfg!(test){self.load_usage_dashboard(true, cx);}
        cx.notify();
    }
    fn toggle_usage_filter(&mut self, filter: u8, cx: &mut Context<Self>) {
        if self.usage_dashboard.dropdown == Some(filter) {
            self.close_usage_filter(cx);
        } else {
            self.usage_dashboard.dropdown = Some(filter);
            self.usage_dashboard.menu_at = Instant::now();
            self.usage_dashboard.menu_closing = false;
            cx.notify();
        }
    }
    fn close_usage_filter(&mut self,cx:&mut Context<Self>){if self.usage_dashboard.dropdown.is_some()&&!self.usage_dashboard.menu_closing{self.usage_dashboard.menu_closing=true;self.usage_dashboard.menu_at=Instant::now();cx.notify();}}
    pub(super) fn close_usage_dashboard(&mut self, cx: &mut Context<Self>) {
        self.usage_dashboard.closing = true;
        self.usage_dashboard.at = Instant::now();
        cx.notify();
    }
    pub(super) fn tick_usage_dashboard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.usage_dashboard.open {
            return;
        }
        if self.usage_dashboard.dropdown.is_some(){
            let animating=self.motion&&!native::reduced_motion()&&self.usage_dashboard.menu_at.elapsed()<workspace_motion::pane_duration();
            if animating{request_motion_frame(window,cx);}else if self.usage_dashboard.menu_closing{self.usage_dashboard.dropdown=None;self.usage_dashboard.menu_closing=false;cx.notify();}
        }
        if (self.usage_dashboard.at.elapsed() < workspace_motion::pane_duration()
            || self.usage_dashboard.change_at.elapsed() < workspace_motion::pane_duration())
            && self.motion
            && !native::reduced_motion()
        {
            request_motion_frame(window, cx);
        } else if self.usage_dashboard.closing {
            self.usage_dashboard.open = false;
            self.usage_dashboard.closing = false;
            cx.notify();
        }
    }
    fn load_usage_dashboard(&mut self, scan: bool, cx: &mut Context<Self>) {
        // One scan at a time. Filter changes are applied after an in-flight scan completes.
        if self.usage_dashboard.busy {
            return;
        }
        self.usage_dashboard.busy = true;
        self.usage_dashboard.error = None;
        self.usage_dashboard.generation += 1;
        let generation = self.usage_dashboard.generation;
        let days = self.usage_dashboard.days;
        let provider = self.usage_dashboard.provider.clone();
        let cache = self.controls_home.join(".kai/usage-history");
        let environment = self.usage_dashboard.environment.clone();
        let host = environment
            .as_ref()
            .and_then(|id| self.ninjas.hosts.iter().find(|h| &h.id == id))
            .cloned();
        cx.spawn(async move|this,cx|{
            let filter=provider.clone();
            let result=cx.background_executor().spawn(async move{
                if let Some(host)=host {
                    let value=ninjas_ui::call(&host,&serde_json::json!({"action":"usage_summary","days":days,"provider":provider,"cached":!scan})).map_err(|e|e.to_string())?;
                    serde_json::from_value::<Report>(value).map_err(|e|e.to_string())
                } else {query_local(&cache,days,&provider,!scan).and_then(|v|Ok(serde_json::from_value::<Report>(v)?)).map_err(|e|e.to_string())}

            }).await;
            let _=this.update(cx,|app,cx|{
                if app.usage_dashboard.generation!=generation{return}
                app.usage_dashboard.busy=false;
                match result{Ok(r)=>{app.usage_dashboard.report=Arc::new(r);app.usage_dashboard.change_at=Instant::now();},Err(e)=>app.usage_dashboard.error=Some(e)}
                if app.usage_dashboard.days!=days || app.usage_dashboard.provider!=filter || app.usage_dashboard.environment!=environment{app.load_usage_dashboard(false,cx);}
                cx.notify();
            });
        }).detach();
    }
    pub(super) fn usage_dashboard_view(&self, window: &Window, cx: &Context<Self>) -> AnyElement {
        let state = &self.usage_dashboard;
        let report = state.report.clone();
        let t = if self.motion && !native::reduced_motion() {
            workspace_motion::ease_out(
                state.at.elapsed().as_secs_f32() / workspace_motion::pane_duration().as_secs_f32(),
            )
        } else {
            1.
        };
        let progress = if state.closing { 1. - t } else { t };
        let color = self
            .task_pet_visuals
            .get("selected")
            .and_then(|v| v.colors.first())
            .copied()
            .unwrap_or(0xb7c8dc);
        let button = |id: String, text: String, selected: bool| {
            div()
                .id(SharedString::from(id))
                .px_3()
                .py_1()
                .rounded_md()
                .cursor_pointer()
                .bg(if selected {
                    rgba((color << 8) | 0x30)
                } else {
                    rgba(0xffffff08)
                })
                .hover(|s| s.bg(rgba(0xffffff20)))
                .child(text)
        };
        let stat = |name: &str, value: String| {
            div().id(SharedString::from(format!("usage-total-{name}")))
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col().items_start().overflow_hidden().text_left()
                .gap_1()
                .child(
                    div()
                        .text_color(rgb(MUTED))
                        .text_size(px(11.))
                        .child(name.to_owned()),
                )
                .child(div().w_full().truncate().text_size(px(20.)).child(value))
        };
        let growth = if self.motion && !native::reduced_motion() {
            workspace_motion::ease_out(
                state.change_at.elapsed().as_secs_f32()
                    / workspace_motion::pane_duration().as_secs_f32(),
            )
        } else {
            1.
        };
        let mode = state.mode;
        let by_time = state.by_time;
        let series = report.series.clone();
        let hover = state.hover;
        let chart_series = series.clone();
        let chart = div()
            .id("usage-history-chart")
            .debug_selector(|| "usage-history-chart".into())
            .h(px(230.))
            .w_full()
            .relative()
            .on_mouse_move(cx.listener(move |app, e: &MouseMoveEvent, _, cx| {
                if let Some(bounds) = app.usage_chart_bounds.get() {
                    let x = f32::from(e.position.x - bounds.origin.x)
                        / f32::from(bounds.size.width).max(1.);
                    let i = (x.clamp(0., 0.9999) * app.usage_dashboard.report.series.len() as f32)
                        as usize;
                    if app.usage_dashboard.hover != Some(i) {
                        app.usage_dashboard.hover = Some(i);
                        cx.notify();
                    }
                }
            }))
            .child(
                canvas(
                    |b, _, _| b,
                    move |b, _, window, _| {
                        let max = chart_series
                            .iter()
                            .map(|p| if mode == 1 { p.tokens as f64 } else { p.cost })
                            .fold(0f64, f64::max)
                            .max(f64::EPSILON);
                        for j in 0..5 {
                            let mut p = PathBuilder::stroke(px(1.));
                            let y = b.top() + b.size.height * j as f32 / 4.;
                            p.move_to(point(b.left(), y));
                            p.line_to(point(b.right(), y));
                            if let Ok(p) = p.build() {
                                window.paint_path(p, rgba(0xffffff12));
                            }
                        }
                        let mut area=PathBuilder::fill();area.move_to(point(b.left(),b.bottom()));
                        let mut path = PathBuilder::stroke(px(2.));
                        let n = chart_series.len();
                        for (i, v) in chart_series.iter().enumerate() {
                            let value = if mode == 1 { v.tokens as f64 } else { v.cost };
                            let p = point(
                                b.left()
                                    + b.size.width
                                        * (i as f32 / (n.saturating_sub(1).max(1)) as f32),
                                b.bottom() - b.size.height * (value / max) as f32 * growth,
                            );
                            area.line_to(p);
                            if i == 0 {
                                path.move_to(p)
                            } else {
                                path.line_to(p)
                            }
                        }
                        if n > 1 {
                            area.line_to(point(b.right(),b.bottom()));area.close();if let Ok(area)=area.build(){window.paint_path(area,rgba((color<<8)|0x24));}
                            if let Ok(path) = path.build() {
                                window.paint_path(path, rgb(color));
                            }
                        }
                        if let Some(i) = hover.filter(|i| *i < n) {
                            let x = b.left()
                                + b.size.width * i as f32 / (n.saturating_sub(1).max(1)) as f32;
                            let mut p = PathBuilder::stroke(px(1.));
                            p.move_to(point(x, b.top()));
                            p.line_to(point(x, b.bottom()));
                            if let Ok(p) = p.build() {
                                window.paint_path(p, rgba(0xffffff60));
                            }
                        }
                    },
                )
                .size_full(),
            )
            .child({
                let bounds = self.usage_chart_bounds.clone();
                canvas(
                    move |b, _, _| {
                        bounds.set(Some(b));
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full()
            })
            .when_some(hover.and_then(|i| series.get(i)), |d, p| {
                d.child(
                    div()
                        .absolute()
                        .top_2()
                        .right_2()
                        .p_2()
                        .rounded_lg()
                        .bg(rgba(0x181818ee))
                        .child(format!(
                            "{}  ·  {}  ·  {} tokens",
                            label(p.at, state.days == 1),
                            money(p.cost),
                            compact(p.tokens)
                        )),
                )
            });
        let providers = report.providers.clone();
        let rows = if by_time {
            report
                .series
                .iter()
                .map(|b| Group {
                    name: label(b.at, state.days == 1),
                    cost: b.cost,
                    tokens: b.tokens,
                    sessions: 0,unpriced:0,
                })
                .collect::<Vec<_>>()
        } else {
            report.models.clone()
        };
        let body = div()
            .id("usage-history-scroll")
            .overflow_y_scroll()
            .flex_1()
            .min_h(px(0.))
            .p_6()
            .flex()
            .flex_col()
            .gap_5()
            .when_some(state.error.as_ref(), |d, e| {
                d.child(div().text_color(rgb(0xffb4a0)).child(e.clone()))
            })
            .child(
                div()
                    .flex()
                    .gap_6()
                    .child(
                        div()
                            .w(px(260.))
                            .flex_none()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(div().text_size(px(38.)).child(if mode == 1 {
                                compact(report.tokens)
                            } else {
                                money(report.cost)
                            }))
                            .child(
                                div()
                                    .text_color(rgb(MUTED))
                                    .child(format!("{} sessions · API estimate", report.sessions)),
                            )
                            .children(providers.into_iter().map(|p| {
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(div().flex().justify_between().child(p.name).child(
                                        if mode == 1 {
                                            compact(p.tokens)
                                        } else {
                                            money(p.cost)
                                        },
                                    ))
                                    .child(div().text_color(rgb(MUTED)).text_size(px(11.)).child(
                                        format!(
                                            "{} sessions · {} tokens",
                                            p.sessions,
                                            compact(p.tokens)
                                        ),
                                    ))
                            })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(if mode == 1 {
                                "Token history"
                            } else {
                                "API equivalent cost"
                            })
                            .child(chart)
                            .child(
                                div()
                                    .flex()
                                    .justify_between()
                                    .text_color(rgb(MUTED))
                                    .children(
                                        [
                                            series.first(),
                                            series.get(series.len() / 2),
                                            series.last(),
                                        ]
                                        .into_iter()
                                        .flatten()
                                        .map(|b| label(b.at, state.days == 1)),
                                    ),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap_4()
                    .child(stat("Processed tokens", compact(report.tokens)))
                    .child(stat("Cached input", compact(report.cache)))
                    .child(stat("Uncached input", compact(report.input)))
                    .child(stat("Output", compact(report.output)))
                    .child(stat("Cache savings", money(report.savings))),
            )
            .child(
                div()
                    .flex()
                    .gap_6()
                    .child(usage_bar(
                        "Cost by type",
                        ["Input / cache write", "Cache read", "Output"]
                            .into_iter()
                            .zip(report.types.iter().copied())
                            .map(|(n, v)| (n.to_string(), v))
                            .collect(),
                        color,
                    ))
                    .child(usage_bar(
                        &format!("Cost by speed · Premium {}",money(report.premium)),
                        report.speeds.iter().map(|(n, v)| (n.clone(), *v)).collect(),
                        color,
                    )),
            )
            .child(
                div().flex().justify_between().child("Breakdown").child(
                    button(
                        "usage-breakdown".into(),
                        if by_time {
                            if state.days == 1 { "Hour" } else { "Day" }
                        } else {
                            "Model"
                        }
                        .into(),
                        true,
                    )
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.usage_dashboard.by_time = !app.usage_dashboard.by_time;
                        cx.notify();
                    })),
                ),
            )
            .child(div().flex().gap_4().text_color(rgb(MUTED)).text_size(px(11.)).child(div().flex_1().child(if by_time{if state.days==1{"Hour"}else{"Day"}}else{"Model"})).child("Cost").child("Share").child(div().w(px(90.)).text_right().child("Tokens")))
            .children(rows.into_iter().enumerate().map(|(i, g)| {
                div()
                    .flex()
                    .gap_4()
                    .py_2()
                    .border_b_1()
                    .border_color(rgba(0xffffff10))
                    .child(
                        div()
                            .w(px(26.))
                            .text_color(rgb(MUTED))
                            .child((i + 1).to_string()),
                    )
                    .child(div().flex_1().child(g.name))
                    .child(if g.unpriced>0&&g.cost==0.{"Unpriced".into()}else{money(g.cost)})
                    .child(format!(
                        "{:.1}%",
                        if report.cost > 0. {
                            100. * g.cost / report.cost
                        } else {
                            0.
                        }
                    ))
                    .child(div().w(px(90.)).text_right().child(compact(g.tokens)))
            }))
            .children(report.warnings.iter().map(|s| {
                div()
                    .text_size(px(11.))
                    .text_color(rgb(MUTED))
                    .child(s.clone())
            }));
        let limits=div().flex().flex_col().gap_4().p_6().children(self.agents.iter().filter(|a|a.available && state.environment.is_none()).map(|a|{
            let windows=self.provider_limit_windows(a.kind);div().flex().gap_4().child(a.name.clone()).children(windows.iter().map(|w|w.compact_remaining(chrono::Utc::now().timestamp(),true))).when(windows.is_empty(),|d|d.child("No provider-reported limits available"))
        })).when(state.environment.is_some(),|d|d.child("Remote account limits are not reported by this host. Select Local environment to see this Mac’s limits."));
        let choices: Vec<(String, Option<String>)> = if state.dropdown == Some(0) {
            std::iter::once(("Local environment".into(), None))
                .chain(
                    self.ninjas
                        .hosts
                        .iter()
                        .filter(|h| h.endpoint.is_some())
                        .map(|h| (h.name.clone(), Some(h.id.clone()))),
                )
                .collect()
        } else {
            [
                "All providers",
                "Codex",
                "Claude",
                "Grok Build",
                "OpenCode",
                "Antigravity",
            ]
            .into_iter()
            .map(|p| (p.to_string(), Some(p.to_string())))
            .collect()
        };
        let mt=if self.motion&&!native::reduced_motion(){workspace_motion::ease_out(state.menu_at.elapsed().as_secs_f32()/workspace_motion::pane_duration().as_secs_f32())}else{1.};let menu_progress=if state.menu_closing{1.-mt}else{mt};
        let menu = div()
            .id("usage-filter-menu")
            .absolute()
            .left(px(if state.dropdown == Some(0) { 44. } else { 240. }))
            .top(px(76.+8.*(1.-menu_progress))).opacity(menu_progress)
            .w(px(240.))
            .p_2()
            .rounded_lg()
            .border_1()
            .border_color(rgba(0xffffff30))
            .bg(rgba(0x252525ee))
            .shadow_lg()
            .flex()
            .flex_col()
            .gap_1()
            .on_mouse_down_out(cx.listener(|app, _, _, cx| {
                app.close_usage_filter(cx);
                cx.notify();
            }))
            .children(choices.into_iter().enumerate().map(|(i, (name, value))| {
                button(format!("usage-filter-{i}"), name, false).on_click(cx.listener(
                    move |app, _, _, cx| {
                        if app.usage_dashboard.dropdown == Some(0) {
                            app.usage_dashboard.environment = value.clone();
                        } else {
                            app.usage_dashboard.provider =
                                value.clone().unwrap_or_else(|| "All providers".into());
                        }
                        app.close_usage_filter(cx);
                        app.load_usage_dashboard(true, cx);
                        cx.notify();
                    },
                ))
            }));
        div()
            .id("usage-dashboard-shield")
            .absolute()
            .inset_0()
            .bg(rgba(0x00000060))
            .on_mouse_down(MouseButton::Left, cx.listener(|app, _, _, cx| {
                app.close_usage_dashboard(cx);
                cx.stop_propagation();
            }))
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .id("usage-dashboard")
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .debug_selector(|| "usage-dashboard".into())
                    .absolute()
                    .left(px(32.))
                    .right(px(32.))
                    .top(px(32. + 10. * (1. - progress)))
                    .bottom(px(32.))
                    .opacity(progress)
                    .rounded_xl()
                    .border_1()
                    .border_color(rgba(0xffffff28))
                    .glass_popup()
                    .shadow_lg()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .text_size(px(13.))
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap_2()
                            .p_3()
                            .border_b_1()
                            .border_color(rgba(0xffffff18))
                            .child(
                                button(
                                    "usage-environment".into(),
                                    self.ninjas
                                        .hosts
                                        .iter()
                                        .find(|h| Some(&h.id) == state.environment.as_ref())
                                        .map_or_else(
                                            || "Usage / Local environment".into(),
                                            |h| format!("Usage / {}", h.name),
                                        ),
                                    false,
                                )
                                .on_click(cx.listener(
                                    |app, _, _, cx| {
                                        app.toggle_usage_filter(0, cx);
                                    },
                                )),
                            )
                            .child(
                                button("usage-provider".into(), state.provider.clone(), false)
                                    .on_click(cx.listener(|app, _, _, cx| {
                                        app.toggle_usage_filter(1, cx);
                                    })),
                            )
                            .child(div().flex_1())
                            .children(
                                [(1, "24h"), (7, "7d"), (30, "30d"), (90, "90d")]
                                    .into_iter()
                                    .map(|(days, text)| {
                                        button(
                                            format!("usage-days-{days}"),
                                            text.into(),
                                            state.days == days,
                                        )
                                        .on_click(
                                            cx.listener(move |app, _, _, cx| {
                                                app.usage_dashboard.days = days;
                                                app.load_usage_dashboard(false, cx);
                                                cx.notify();
                                            }),
                                        )
                                    }),
                            )
                            .children(["Cost", "Tokens", "Limits"].into_iter().enumerate().map(
                                |(mode, text)| {
                                    button(
                                        format!("usage-mode-{mode}"),
                                        text.into(),
                                        state.mode == mode as u8,
                                    )
                                    .on_click(cx.listener(
                                        move |app, _, _, cx| {
                                            app.usage_dashboard.mode = mode as u8;
                                            app.usage_dashboard.change_at = Instant::now();
                                            cx.notify();
                                        },
                                    ))
                                },
                            ))
                            .child(
                                button(
                                    "usage-refresh".into(),
                                    if state.busy { "Reading…" } else { "↻" }.into(),
                                    false,
                                )
                                .on_click(
                                    cx.listener(|app, _, _, cx| app.load_usage_dashboard(true, cx)),
                                ),
                            )
                            .child(button("usage-close".into(), "×".into(), false).on_click(
                                cx.listener(|app, _, _, cx| app.close_usage_dashboard(cx)),
                            )),
                    )
                    .when(mode != 2, |d| d.child(body))
                    .when(mode == 2, |d| d.child(limits)),
            )
            .when(state.dropdown.is_some(), |d| d.child(menu))
            .into_any_element()
    }
}
fn usage_bar(title: &str, values: Vec<(String, f64)>, color: u32) -> AnyElement {
    let total = values.iter().map(|(_, v)| v).sum::<f64>().max(0.000001);
    div()
        .flex_1()
        .flex()
        .flex_col()
        .gap_2()
        .child(title.to_owned())
        .child(
            div()
                .h(px(8.))
                .w_full()
                .flex()
                .rounded_md()
                .overflow_hidden()
                .bg(rgba(0xffffff12))
                .children(values.iter().enumerate().map(|(i, (_, v))| {
                    div()
                        .h_full()
                        .w(relative((*v / total) as f32))
                        .bg(rgba((color << 8) | [0xc0, 0x80, 0xff][i % 3]))
                })),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap_3()
                .text_size(px(11.))
                .text_color(rgb(MUTED))
                .children(values.into_iter().map(|(n, v)| format!("{n} {}", money(v)))),
        )
        .into_any_element()
}

pub(crate) fn query_local(
    cache: &std::path::Path,
    days: u32,
    provider: &str,
    cached: bool,
) -> anyhow::Result<serde_json::Value> {
    let mut command = std::process::Command::new("/usr/bin/python3");
    command
        .arg("-c")
        .arg(include_str!("../native_helpers/kai_usage_history.py"))
        .arg("--cache")
        .arg(cache)
        .arg("--days")
        .arg(days.clamp(1, 90).to_string())
        .arg("--provider")
        .arg(provider);
    if cached {
        command.arg("--cached");
    }
    let out = command.output()?;
    anyhow::ensure!(
        out.status.success(),
        "The usage history index could not be read"
    );
    Ok(serde_json::from_slice(&out.stdout)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    #[gpui::test]
    fn usage_bar_and_shortcut_each_open_and_close_the_dashboard(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        view.update(cx, |app, cx| { app.motion = false; cx.notify(); });
        crate::ui_tests::draw_frame(&view, cx);
        let trigger = cx.debug_bounds("open-usage-history").expect("Usage trigger");
        let point = trigger.center();
        for expected in [true, false] {
            cx.simulate_click(point, Modifiers::none());
            crate::ui_tests::draw_frame(&view, cx);
            view.update(cx, |app, _| assert_eq!(app.usage_dashboard.open, expected));
        }
        cx.update(|window, cx| view.update(cx, |app, cx| {
            for expected in [true, false] {
                app.execute_shortcut("usage", window, cx);
                app.tick_usage_dashboard(window, cx);
                assert_eq!(app.usage_dashboard.open, expected);
            }
        }));
    }

    #[gpui::test]
    fn history_dashboard_draws_and_releases_terminal_input_after_profile_close(cx:&mut TestAppContext){
        let(view,cx)=crate::ui_tests::fixture(cx);
        cx.update(|window,cx|view.update(cx,|app,cx|{
            app.open_usage_dashboard(window,cx);
            app.usage_dashboard.report=Arc::new(serde_json::from_value(serde_json::json!({"cost":12.4,"tokens":1200000,"sessions":4,"series":[{"at":1800000000.,"cost":4.,"tokens":100},{"at":1800003600.,"cost":8.4,"tokens":200}],"models":[{"name":"fixture model","cost":12.4,"tokens":1200000}],"providers":[{"name":"Codex","cost":12.4,"tokens":1200000}]})).unwrap());
            app.usage_dashboard.at=Instant::now()-Duration::from_secs(1);
        }));
        crate::ui_tests::draw_frame(&view,cx);
        assert!(cx.debug_bounds("usage-dashboard").is_some());assert!(cx.debug_bounds("usage-history-chart").is_some());
        view.update(cx,|app,_|assert!(app.terminal_input_blocked));
        cx.update(|window,cx|view.update(cx,|app,cx|{
            app.close_usage_dashboard(cx);app.usage_dashboard.at=Instant::now()-Duration::from_secs(1);app.tick_usage_dashboard(window,cx);app.sync_overlay_input(window,cx);
            assert!(!app.usage_dashboard.open);assert!(!app.terminal_input_blocked);
        }));
    }
}
