//! Monocode chat background controls and pixel effects, ported from its MIT
//! appearance model, ProjectBackgroundDialog, worker, and gradient-blur CSS.
//! Source revision/provenance: assets/NOTICE.md; LICENSES/Monocode-MIT.txt.
//! Background I/O, decoding, GIF composition, and transforms stay on one worker.
use crate::theme::{rgb, rgba};
use gpui::{prelude::*, *};
use image::{AnimationDecoder, ImageDecoder, RgbaImage};
use serde::{Deserialize, Serialize};
use std::{
    cell::Cell,
    fs::{self, File},
    io::{Cursor, Read},
    path::{Path, PathBuf},
    rc::Rc,
    sync::{Arc, OnceLock, mpsc},
    time::{Duration, Instant},
};

const SOURCE_LIMIT: u64 = 32 * 1024 * 1024;
const SVG_LIMIT: usize = 4 * 1024 * 1024;
const STATIC_SIDE: u32 = 2048;
const ANIMATED_SIDE: u32 = 512;
const FADE_HAZE_SIDE: u32 = 256;
const RESIZE_SETTLE: Duration = Duration::from_millis(120);
const DEFAULT_WALLPAPER: &[u8] = include_bytes!("../assets/wallpapers/bonsai-sakura.png");

// Called only by the background worker. Share one managed source across projects;
// decoding and the existing crop/fade cache remain bounded and off the UI thread.
fn default_settings(data: &Path) -> std::io::Result<Settings> {
    let path = data.join("bonsai-sakura.png");
    if !path.exists() {
        fs::create_dir_all(data)?;
        let temporary = data.join("bonsai-sakura.png.next");
        fs::write(&temporary, DEFAULT_WALLPAPER)?;
        fs::rename(temporary, &path)?;
    }
    Ok(Settings {
        path: Some(path),
        empty_opacity: 0.40,
        session_opacity: 0.28,
        ..Settings::default()
    })
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Effect {
    #[default]
    None,
    Dither,
    Ascii,
    Halftone,
    Scanlines,
    GradientBlur,
}
impl Effect {
    const ALL: [Self; 6] = [
        Self::None,
        Self::Dither,
        Self::Ascii,
        Self::Halftone,
        Self::Scanlines,
        Self::GradientBlur,
    ];
    fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Dither => "Dither",
            Self::Ascii => "ASCII",
            Self::Halftone => "Halftone",
            Self::Scanlines => "Scanlines",
            Self::GradientBlur => "Haze",
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Fade {
    #[default]
    Classic,
    Down,
    Up,
    Left,
    Right,
    Full,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    path: Option<PathBuf>,
    effect: Effect,
    empty_only: bool,
    everywhere: bool,
    empty_opacity: f32,
    session_opacity: f32,
    fade: Fade,
    coverage: u8,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            path: None,
            effect: Effect::None,
            empty_only: false,
            everywhere: false,
            empty_opacity: 0.24,
            session_opacity: 0.24,
            fade: Fade::Classic,
            coverage: 100,
        }
    }
}
impl Settings {
    fn normalize(&mut self) {
        self.empty_opacity = visibility(self.empty_opacity);
        self.session_opacity = visibility(self.session_opacity);
        self.coverage = self.coverage.min(100);
    }
}
fn visibility(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0., 1.)
    } else {
        0.24
    }
}
#[derive(Clone, Copy)]
pub enum BackgroundEvent {
    Changed,
}
impl EventEmitter<BackgroundEvent> for ChatBackground {}

struct Config {
    generation: u64,
    project: PathBuf,
    settings: Settings,
    empty: bool,
    active: bool,
    viewport: Option<(u32, u32)>,
}
enum Command {
    Load {
        generation: u64,
        project: PathBuf,
        empty: bool,
        active: bool,
        viewport: Option<(u32, u32)>,
    },
    Configure(Config),
    Scope(Config),
    Save {
        project: PathBuf,
        settings: Settings,
        active: bool,
    },
    Import {
        config: Config,
        source: PathBuf,
    },
    Remove(Config),
    Default(Config),
    Active(bool),
    Shutdown,
}
enum Event {
    Ready {
        generation: u64,
        settings: Settings,
        image: Option<Arc<WallpaperFrame>>,
        animated: bool,
        empty: bool,
        viewport: Option<(u32, u32)>,
    },
    Frame(u64, Arc<WallpaperFrame>),
    Error(u64, String),
}

/// The effect is computed once; the notch reuses a bounded unfaded surface.
#[derive(Debug)]
struct WallpaperFrame {
    image: Arc<RenderImage>, native: Option<RgbaImage>,
}
impl std::ops::Deref for WallpaperFrame {type Target=RenderImage;fn deref(&self)->&RenderImage{&self.image}}
impl WallpaperFrame {
    #[cfg(test)]fn fixture(frames:Vec<image::Frame>)->Arc<Self>{Arc::new(Self{image:Arc::new(RenderImage::new(frames)),native:None})}
}

struct FadeTransition {
    previous: Arc<WallpaperFrame>,
    from: Fade,
    started: Instant,
}

pub struct ChatBackground {
    surfaces: (bool, bool),
    surface_bounds: Rc<Cell<Bounds<Pixels>>>,
    project: PathBuf,
    generation: u64,
    settings: Settings,
    image: Option<Arc<WallpaperFrame>>,
    presented_fade: Fade,
    fade_transition: Option<FadeTransition>,
    empty: bool,
    active: bool,
    native_active:bool,
    busy: bool,
    quitting: bool,
    error: Option<String>,
    animated: bool,
    tx: mpsc::Sender<Command>,
    empty_bounds: Rc<Cell<Bounds<Pixels>>>,
    session_bounds: Rc<Cell<Bounds<Pixels>>>,
    viewport: Option<(u32, u32)>,
    prepared_viewport: Option<(u32, u32)>,
    measured_viewport: Rc<Cell<Option<(u32, u32)>>>,
    resized_at: Instant,
    resize_pending: bool,
    opening_motion: bool,
    wallpaper_reveal: WallpaperReveal,
}
// Stable full-size artwork can reuse its scene during unrelated root motion.
// GPUI invalidates this cache on child notification, bounds, clip or text style.
pub(super) fn cached(view: Entity<ChatBackground>, stable_mask: bool) -> AnyView {
    if !stable_mask { return AnyView::from(view) }
    let mut style = StyleRefinement::default();
    style.size.width = Some(relative(1.).into());
    style.size.height = Some(relative(1.).into());
    AnyView::from(view).cached(style)
}
#[cfg(test)]
thread_local! { static BACKGROUND_RENDERS: Cell<usize> = const { Cell::new(0) }; }

impl ChatBackground {
    pub(super) fn set_surfaces(&mut self, sides: bool, bars: bool, cx: &mut Context<Self>) {
        if self.surfaces != (sides, bars) { self.surfaces = (sides, bars); cx.notify(); }
    }
    pub(super) fn surface_bounds(&self) -> Rc<Cell<Bounds<Pixels>>> { self.surface_bounds.clone() }
    pub fn new(
        data_dir: PathBuf,
        project: PathBuf,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let (events_tx, events_rx) = async_channel::bounded(1);
        std::thread::spawn(move || worker(data_dir.join("backgrounds"), rx, events_tx));
        cx.spawn(async move |this, cx| {
            while let Ok(event) = events_rx.recv().await {
                if this.update(cx, |this, cx| this.receive(event, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        cx.on_release(|this, cx| {
            let _ = this.tx.send(Command::Shutdown);
            this.clear_image(cx);
            this.clear_fade_transition(cx);
        })
        .detach();
        let this = Self {
            surfaces: (false, false),
            surface_bounds: Default::default(),
            project: project.clone(),
            generation: 1,
            settings: Settings::default(),
            image: None,
            empty: true,
            active: true,
            native_active:false,
            busy: true,
            quitting: false,
            error: None,
            animated: false,
            tx,
            empty_bounds: Rc::new(Cell::new(Bounds::default())),
            session_bounds: Rc::new(Cell::new(Bounds::default())),
            viewport: None,
            prepared_viewport: None,
            measured_viewport: Rc::new(Cell::new(None)),
            resized_at: Instant::now(),
            resize_pending: false,
            opening_motion: true, presented_fade: Fade::Classic, fade_transition: None,
            wallpaper_reveal: Default::default(),
        };
        let _ = this.tx.send(Command::Load {
            generation: 1,
            project,
            empty: true,
            active: true,
            viewport: None,
        });
        this
    }
    fn clear_image(&mut self, cx: &mut App) {
        if let Some(image) = self.image.take() {
            cx.drop_image(image.image.clone(), None);
        }
    }
    fn clear_fade_transition(&mut self, cx: &mut App) {
        if let Some(transition) = self.fade_transition.take() {
            cx.drop_image(transition.previous.image.clone(), None);
        }
    }

    fn replace_prepared_image(&mut self, image: Option<Arc<WallpaperFrame>>, cx: &mut Context<Self>) {
        let fade = self.settings.fade;
        if self.presented_fade != fade && self.opening_motion && !crate::native::reduced_motion()
            && self.active && image.is_some()
        {
            self.clear_fade_transition(cx);
            self.fade_transition = self.image.take().map(|previous| FadeTransition {
                previous,
                from: self.presented_fade,
                started: Instant::now(),
            });
        } else {
            self.clear_image(cx);
            if image.is_none() { self.clear_fade_transition(cx); }
        }
        self.presented_fade = fade;
        self.image = image;
    }

    fn receive(&mut self, event: Event, cx: &mut Context<Self>) {
        // Keep the final painted frame until the native window closes. Quit
        // cleanup can still deliver load/resize replies or GIF frames.
        if self.quitting {
            return;
        }
        match event {
            Event::Ready {
                generation,
                settings,
                image,
                animated,
                empty,
                viewport,
            } if generation == self.generation => {
                self.settings = settings;
                self.busy = false;
                self.error = None;
                // An unmeasured or superseded crop changes the apparent size
                // of pixel effects when stretched, then snaps on the next
                // Ready. Publish only the current layout to Code and the notch.
                // Retain an existing good frame while a resize is in flight.
                if self.settings.path.is_some()
                    && (viewport.is_none() || viewport != self.viewport || empty != self.empty)
                {
                    if self.viewport.is_some() {
                        if self.image.is_none() || empty != self.empty {self.prepare(cx)}
                        else {self.schedule_resize(cx)}
                    }
                    self.changed(cx);
                    return;
                }
                self.replace_prepared_image(image, cx);
                self.animated = animated;
                self.prepared_viewport = viewport;
                let _ = self.tx.send(Command::Active((self.active || self.native_active) && self.visible()));
                if self.empty != empty && self.settings.path.is_some() {
                    self.prepare(cx);
                } else {
                    if self.viewport != viewport && self.settings.path.is_some() {
                        self.schedule_resize(cx);
                    }
                    self.changed(cx);
                }
            }
            Event::Frame(generation, image) if generation == self.generation && self.prepared_viewport.is_some() => {
                self.clear_image(cx);
                self.image = Some(image);
                cx.notify();
            }
            Event::Error(generation, error) if generation == self.generation => {
                self.busy = false;
                self.error = Some(error);
                self.changed(cx);
            }
            _ => {}
        }
    }
    fn changed(&self, cx: &mut Context<Self>) {
        cx.notify();
        cx.emit(BackgroundEvent::Changed);
    }
    pub fn set_quitting(&mut self, quitting: bool, cx: &mut Context<Self>) {
        if self.quitting == quitting {
            return;
        }
        self.quitting = quitting;
        if quitting {
            let _ = self.tx.send(Command::Active(false));
        } else {
            // A cancelled quit resumes from persisted worker settings, including
            // any import that completed while its presentation was frozen.
            self.generation += 1;
            self.busy = true;
            let _ = self.tx.send(Command::Load {
                generation: self.generation,
                project: self.project.clone(),
                empty: self.empty,
                active: self.active || self.native_active,
                viewport: self.viewport,
            });
        }
        cx.notify();
    }
    pub fn set_project(&mut self, project: PathBuf, cx: &mut Context<Self>) {
        if self.quitting || self.project == project {
            return;
        }
        self.project = project.clone();
        // A global wallpaper belongs to the workspace, not its selected tab.
        // Retain its texture, decoder and effect instead of reloading it for
        // every project. Subsequent edits still use the newly selected project.
        if self.settings.everywhere && self.image.is_some() && !self.busy {
            self.changed(cx);
            return;
        }
        self.generation += 1;
        // Keep the last painted settings/frame until this generation's Ready
        // atomically replaces them. Resetting either here exposes a blank frame.
        self.busy = true;
        self.error = None;
        let _ = self.tx.send(Command::Load {
            generation: self.generation,
            project,
            empty: self.empty,
            active: self.active || self.native_active,
            viewport: self.viewport,
        });
        self.changed(cx);
    }
    pub fn set_context(&mut self, empty: bool, active: bool, cx: &mut Context<Self>) {
        if self.quitting {
            return;
        }
        let changed = self.empty != empty || self.active != active;
        if !changed {
            return;
        }
        let scope_changed = self.empty != empty;
        self.empty = empty;
        self.active = active;
        if scope_changed && self.settings.path.is_some() {
            self.prepare(cx);
        } else {
            let _ = self.tx.send(Command::Active((self.active || self.native_active) && self.visible()));
            self.changed(cx);
        }
    }
    pub(super) fn set_native_context(&mut self,empty:bool,active:bool,cx:&mut Context<Self>){
        if self.quitting{return}
        let changed=self.native_active!=active;self.native_active=active;
        self.set_context(empty,self.active,cx);
        if changed{let _=self.tx.send(Command::Active((self.active||self.native_active)&&self.visible()));cx.notify();}
    }
    pub(super) fn native_surface(&self)->Option<(&RgbaImage,f32,ImageId)>{
        if !self.visible(){return None}
        let frame=self.image.as_ref()?;
        Some((frame.native.as_ref()?,if self.empty{self.settings.empty_opacity}else{self.settings.session_opacity},frame.id))
    }
    pub(super) fn adaptive_frame_id(&self)->Option<ImageId>{self.image.as_ref().map(|frame|frame.id)}
    pub(super) fn adaptive_animated(&self)->bool{self.animated && self.visible() && (self.active || self.native_active)}
    pub(super) fn adaptive_sample(&self,x:f32,y:f32,width:f32,height:f32,empty:bool,mini:bool)->Option<([f32;3],f32)>{
        if !self.visible(){return None}let image=self.image.as_ref()?;let dimensions=image.image.size(0);let iw=dimensions.width.0 as f32;let ih=dimensions.height.0 as f32;
        if iw<1.||ih<1.||width<=0.||height<=0.{return None}
        let art_h=if mini || self.presented_fade != Fade::Classic{height}else{f32::from(wallpaper_bounds(Bounds::new(point(px(0.),px(0.)),size(px(width),px(height))),empty).size.height)};if y*height>=art_h{return None}
        let scale=(width/iw).max(art_h/ih);let sx=((x*width+(iw*scale-width)*0.5)/scale).clamp(0.,iw-1.)as usize;let sy=((y*height+(ih*scale-art_h)*0.5)/scale).clamp(0.,ih-1.)as usize;
        let pixels=image.image.as_bytes(0)?;let at=(sy*iw as usize+sx)*4;let p=pixels.get(at..at+4)?;let opacity=if empty{self.settings.empty_opacity}else{self.settings.session_opacity};
        Some(([p[2]as f32/255.,p[1]as f32/255.,p[0]as f32/255.],p[3]as f32/255.*opacity))
    }
    /// Reuse the prepared frame in a secondary window. Reading this layer never
    /// changes the main wallpaper viewport, starts a decode, or sends a command.
    pub fn snapshot_layer(&self, empty: bool) -> AnyElement {
        let visible = (!self.settings.empty_only || empty) && self.settings.path.is_some();
        let image = visible.then(|| self.image.clone()).flatten();
        let opacity = if empty {
            self.settings.empty_opacity
        } else {
            self.settings.session_opacity
        };
        div()
            .size_full()
            .overflow_hidden()
            .opacity(opacity)
            .when_some(image, |surface, image| {
                surface.child(img(image.image.clone()).size_full().object_fit(ObjectFit::Cover))
            })
            .into_any_element()
    }
    #[cfg(test)]
    #[allow(clippy::type_complexity)]
    pub(crate) fn snapshot_geometry(&self) -> (Option<(u32, u32)>, Option<(u32, u32)>, u64) {
        (self.viewport, self.prepared_viewport, self.generation)
    }

    fn visible(&self) -> bool {
        (!self.settings.empty_only || self.empty) && self.settings.path.is_some()
    }
    fn config(&self) -> Config {
        Config {
            generation: self.generation,
            project: self.project.clone(),
            settings: self.settings.clone(),
            empty: self.empty,
            active: self.active || self.native_active,
            viewport: self.viewport,
        }
    }
    fn set_viewport(&mut self, viewport: (u32, u32), cx: &mut Context<Self>) {
        if self.quitting || self.viewport == Some(viewport) {
            return;
        }
        let first_layout=self.viewport.is_none();
        self.viewport = Some(viewport);
        self.resized_at = Instant::now();
        if self.settings.path.is_some() {
            if first_layout && !self.busy {self.prepare(cx)}
            else {self.schedule_resize(cx)}
        }
    }
    fn schedule_resize(&mut self, cx: &mut Context<Self>) {
        if self.resize_pending {
            return;
        }
        self.resize_pending = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(RESIZE_SETTLE).await;
                let again = this.update(cx, |this, cx| {
                    if this.resized_at.elapsed() < RESIZE_SETTLE {
                        return true;
                    }
                    this.resize_pending = false;
                    // Ready schedules any geometry still outstanding. Do not
                    // wake repeatedly while another worker request is running.
                    if this.busy {
                        return false;
                    }
                    if this.settings.path.is_some() && this.viewport != this.prepared_viewport {
                        this.prepare(cx);
                    }
                    false
                });
                if !matches!(again, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }
    fn prepare(&mut self, cx: &mut Context<Self>) {
        if self.quitting {
            return;
        }
        self.generation += 1;
        self.busy = true;
        self.error = None;
        let _ = self.tx.send(Command::Configure(self.config()));
        self.changed(cx);
    }
    fn set_scope(&mut self, everywhere: bool, cx: &mut Context<Self>) {
        if self.busy || self.settings.everywhere == everywhere {
            return;
        }
        self.settings.everywhere = everywhere;
        if everywhere {
            self.settings.empty_only = false;
        }
        self.generation += 1;
        self.busy = true;
        self.error = None;
        let _ = self.tx.send(Command::Scope(self.config()));
        self.changed(cx);
    }
    fn save(&self, cx: &mut Context<Self>) {
        let _ = self.tx.send(Command::Save {
            project: self.project.clone(),
            settings: self.settings.clone(),
            active: (self.active || self.native_active) && self.visible(),
        });
        self.changed(cx);
    }
    fn choose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let selection = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose background image or GIF".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = selection.await
                && let Some(source) = paths.first()
            {
                let source = source.clone();
                let _ = this.update_in(cx, |this, _, cx| {
                    this.generation += 1;
                    this.busy = true;
                    this.error = None;
                    let _ = this.tx.send(Command::Import {
                        config: this.config(),
                        source,
                    });
                    this.changed(cx);
                });
            }
        })
        .detach();
    }
    fn remove(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        self.clear_image(cx);
        self.clear_fade_transition(cx);
        self.busy = true;
        let _ = self.tx.send(Command::Remove(self.config()));
        self.changed(cx);
    }
    fn opacity_at(&mut self, empty: bool, position: Point<Pixels>, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let bounds = if empty {
            self.empty_bounds.get()
        } else {
            self.session_bounds.get()
        };
        if bounds.size.width <= px(0.) {
            return;
        }
        let percent = ((position.x - bounds.origin.x) / bounds.size.width).clamp(0., 1.);
        let value = percent * 100.;
        let value = value.round() / 100.;
        if empty {
            if self.settings.empty_opacity == value {
                return;
            }
            self.settings.empty_opacity = value;
        } else {
            if self.settings.session_opacity == value {
                return;
            }
            self.settings.session_opacity = value;
        }
        self.save(cx);
    }
    fn slider(&self, empty: bool, cx: &Context<Self>) -> AnyElement {
        let value = if empty {
            self.settings.empty_opacity
        } else {
            self.settings.session_opacity
        };
        let bounds = if empty {
            self.empty_bounds.clone()
        } else {
            self.session_bounds.clone()
        };
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .id(if empty {
                        "background-empty-slider"
                    } else {
                        "background-session-slider"
                    })
                    .debug_selector(move || {
                        if empty {
                            "background-empty-slider".into()
                        } else {
                            "background-session-slider".into()
                        }
                    })
                    .w(px(170.))
                    .h(px(22.))
                    .relative()
                    .cursor_pointer()
                    .child(
                        canvas(
                            move |next, _, _| {
                                bounds.set(next);
                            },
                            |_, _, _, _| {},
                        )
                        .size_full(),
                    )
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .right_0()
                            .top(px(9.))
                            .h(px(4.))
                            .rounded_full()
                            .bg(rgba(0xebebeb20)),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(px(156. * value.clamp(0., 1.)))
                            .top(px(4.))
                            .size(px(14.))
                            .rounded_full()
                            .bg(rgb(0xebebeb)),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            this.opacity_at(empty, event.position, cx)
                        }),
                    )
                    .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
                        if event.pressed_button == Some(MouseButton::Left) {
                            this.opacity_at(empty, event.position, cx);
                        }
                    })),
            )
            .child(
                div()
                    .w(px(32.))
                    .text_size(crate::theme::ui_px(12.))
                    .child(format!("{}%", (value * 100.).round() as u32)),
            )
            .into_any_element()
    }
    pub fn settings_panel(&self, cx: &Context<Self>) -> AnyElement {
        let mut panel = div()
            .debug_selector(|| "background-settings".into())
            .w_full()
            .min_w(px(0.))
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .text_size(crate::theme::ui_px(12.))
            .line_height(px(18.))
            .text_color(rgb(0xebebeb));
        let mut preview = div()
            .debug_selector(|| "background-preview".into())
            .h(px(90.))
            .w_full()
            .flex_none()
            .relative()
            .rounded_md()
            .overflow_hidden()
            .bg(rgba(0xebebeb0a))
            .border_1()
            .border_color(rgba(0xebebeb18));
        if let Some(image) = &self.image {
            preview = preview.child(
                img(image.image.clone())
                    .size_full()
                    .object_fit(ObjectFit::Contain)
                    .opacity(self.settings.empty_opacity),
            );
        } else {
            preview = preview.flex().items_center().justify_center().child(
                div()
                    .text_color(rgb(0x929292))
                    .text_size(crate::theme::ui_px(11.))
                    .child(if self.busy {
                        "Preparing…"
                    } else {
                        "Choose artwork"
                    }),
            );
        }
        preview = preview.when(self.animated, |s| {
            s.child(
                div()
                    .absolute()
                    .right(px(6.))
                    .top(px(4.))
                    .px_1()
                    .rounded_sm()
                    .bg(rgba(0x17171788))
                    .text_size(crate::theme::ui_px(10.))
                    .child("GIF"),
            )
        });
        panel = panel.child(preview).child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .id("choose-background")
                        .debug_selector(|| "choose-background".into())
                        .flex_1()
                        .h(px(28.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_md()
                        .border_1()
                        .border_color(rgba(0xebebeb18))
                        .bg(rgba(0xebebeb0a))
                        .hover(|s| s.bg(rgba(0xebebeb14)))
                        .cursor_pointer()
                        .child(if self.busy {
                            "Preparing…"
                        } else if self.settings.path.is_some() {
                            "Replace image"
                        } else {
                            "Choose image"
                        })
                        .on_click(cx.listener(|this, _, window, cx| {
                            if !this.busy {
                                this.choose(window, cx);
                            }
                        })),
                )
                .child(
                    div()
                        .id("default-background")
                        .debug_selector(|| "default-background".into())
                        .flex_1()
                        .h(px(28.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_md()
                        .border_1()
                        .border_color(rgba(0xebebeb18))
                        .hover(|s| s.bg(rgba(0xebebeb14)))
                        .cursor_pointer()
                        .child("Use Bonsai")
                        .on_click(cx.listener(|this, _, _, cx| {
                            if !this.busy {
                                this.generation += 1;
                                this.busy = true;
                                this.error = None;
                                let _ = this.tx.send(Command::Default(this.config()));
                                this.changed(cx);
                            }
                        })),
                ),
        );
        if let Some(error) = &self.error {
            panel = panel.child(
                div()
                    .id("background-error")
                    .max_h(px(54.))
                    .overflow_y_scroll()
                    .text_size(crate::theme::ui_px(11.))
                    .text_color(rgb(0xee9494))
                    .child(error.clone()),
            );
        }
        panel = panel.child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_size(crate::theme::ui_px(11.))
                        .text_color(rgb(0x929292))
                        .child("Effect"),
                )
                .child(div().flex().flex_wrap().gap_1().children(
                    Effect::ALL.into_iter().enumerate().map(|(ix, effect)| {
                        div()
                            .id(("background-effect", ix))
                            .debug_selector(move || format!("background-effect-{ix}"))
                            .w(px(96.))
                            .h(px(24.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(crate::theme::ui_px(11.))
                            .rounded_md()
                            .cursor_pointer()
                            .border_1()
                            .border_color(rgba(0xebebeb18))
                            .when(effect == self.settings.effect, |s| s.bg(rgba(0x459bf733)))
                            .hover(|s| s.bg(rgba(0xebebeb14)))
                            .child(effect.label())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.busy && this.settings.effect != effect {
                                    this.settings.effect = effect;
                                    if this.settings.path.is_some() {
                                        this.prepare(cx);
                                    } else {
                                        this.save(cx);
                                    }
                                }
                            }))
                    }),
                )),
        );
        panel = panel.child(
            div()
                .h(px(26.))
                .flex()
                .items_center()
                .justify_between()
                .child("Apply to")
                .child(
                    div().flex().gap_1().children(
                        [(false, "This project"), (true, "All projects")]
                            .into_iter()
                            .enumerate()
                            .map(|(ix, (everywhere, label))| {
                                div()
                                    .id(("background-application", ix))
                                    .debug_selector(move || format!("background-application-{ix}"))
                                    .px_2()
                                    .py_1()
                                    .text_size(crate::theme::ui_px(11.))
                                    .rounded_md()
                                    .cursor_pointer()
                                    .when(self.settings.everywhere == everywhere, |s| {
                                        s.bg(rgba(0xebebeb1a))
                                    })
                                    .hover(|s| s.bg(rgba(0xebebeb14)))
                                    .child(label)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.set_scope(everywhere, cx)
                                    }))
                            }),
                    ),
                ),
        );
        panel = panel.child(
            div()
                .h(px(26.))
                .flex()
                .items_center()
                .justify_between()
                .child("Show on")
                .child(
                    div().flex().gap_1().children(
                        [(true, "Empty chats"), (false, "All chats")]
                            .into_iter()
                            .enumerate()
                            .map(|(ix, (empty_only, label))| {
                                div()
                                    .id(("background-scope", ix))
                                    .debug_selector(move || format!("background-scope-{ix}"))
                                    .px_2()
                                    .py_1()
                                    .text_size(crate::theme::ui_px(11.))
                                    .rounded_md()
                                    .cursor_pointer()
                                    .when(self.settings.empty_only == empty_only, |s| {
                                        s.bg(rgba(0xebebeb1a))
                                    })
                                    .hover(|s| s.bg(rgba(0xebebeb14)))
                                    .child(label)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if !this.busy {
                                            this.settings.empty_only = empty_only;
                                            this.save(cx);
                                        }
                                    }))
                            }),
                    ),
                ),
        );
        panel = panel.child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child("Empty chat")
                        .child(self.slider(true, cx)),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child("Sessions")
                        .child(self.slider(false, cx)),
                ),
        );
        panel = panel.child(div().flex().flex_col().gap_1()
            .child("Fade direction")
            .child(div().flex().flex_wrap().gap_1().children([
                (Fade::Classic, "Classic"), (Fade::Down, "Down"), (Fade::Up, "Up"),
                (Fade::Left, "Left"), (Fade::Right, "Right"), (Fade::Full, "Full"),
            ].into_iter().enumerate().map(|(index, (fade, label))| {
                div().id(("wallpaper-fade", index)).px_2().py_1().rounded_md().cursor_pointer()
                    .when(self.settings.fade == fade, |d| d.bg(rgba(0xebebeb24)))
                    .hover(|d| d.bg(rgba(0xebebeb14))).child(label)
                    .on_click(cx.listener(move |app, _, _, cx| {
                        if app.settings.fade == fade { return; }
                        let changes_extent = (app.settings.fade == Fade::Classic) != (fade == Fade::Classic);
                        app.settings.fade = fade;
                        if !changes_extent || app.viewport.is_none() {
                            app.prepare(cx);
                        } else {
                            // Measure the new extent before asking the worker to recrop.
                            app.prepared_viewport = None;
                            app.generation += 1;
                            app.busy = false;
                            app.resized_at = Instant::now();
                            app.schedule_resize(cx);
                        }
                        app.save(cx);
                    }))
            })))
            .child("Wallpaper coverage")
            .child(div().flex().flex_wrap().gap_1().children([0u8, 10, 25, 50, 75, 100].into_iter().map(|coverage| {
                div().id(("wallpaper-coverage", usize::from(coverage))).px_2().py_1().rounded_md().cursor_pointer()
                    .when(self.settings.coverage == coverage, |d| d.bg(rgba(0xebebeb24)))
                    .hover(|d| d.bg(rgba(0xebebeb14))).child(format!("{coverage}%"))
                    .on_click(cx.listener(move |app, _, _, cx| {
                        app.settings.coverage = coverage;
                        app.prepare(cx);
                        app.save(cx);
                    }))
            }))));
        if self.settings.path.is_some() {
            panel = panel.child(
                div()
                    .id("remove-background")
                    .debug_selector(|| "remove-background".into())
                    .h(px(26.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .text_size(crate::theme::ui_px(11.))
                    .text_color(rgb(0xee9494))
                    .hover(|s| s.bg(rgba(0xee949414)))
                    .cursor_pointer()
                    .child("Remove wallpaper")
                    .on_click(cx.listener(|this, _, _, cx| {
                        if !this.busy {
                            this.remove(cx);
                        }
                    })),
            );
        }
        panel.into_any_element()
    }
}
impl ChatBackground {
    /// Keep the real initial loader pending without exposing its private
    /// commands or depending on decoder/file-system timing. Dropping the guard
    /// closes the replacement queue; entity teardown still owns normal cleanup.
    #[cfg(test)]
    pub(super) fn suspend_initial_loader_for_test(&mut self, cx: &mut Context<Self>) -> Box<dyn std::any::Any> {
        let (tx, held_receiver) = mpsc::channel();
        // Existing Ready/Frame/Error replies become stale before the old
        // sender is released. Reveal replies also require the current image ID.
        self.generation = self.generation.checked_add(1).expect("test background generation overflow");
        self.tx = tx;
        if let Some(image) = self.image.take() {
            // The test can enter through a leased Window. Retire after its
            // outer update restores every surviving window to App.windows.
            cx.defer(move |cx| cx.drop_image(image.image.clone(), None));
        }
        self.wallpaper_reveal = WallpaperReveal::default();
        self.settings.path = None;
        self.busy = true;
        self.error = None;
        self.animated = false;
        self.prepared_viewport = None;
        self.resize_pending = false;
        cx.notify();
        Box::new(held_receiver)
    }

    /// Exercise the actual Ready publication and paint path with the bundled
    /// wallpaper, while tests independently hold the asynchronous initial loader.
    #[cfg(test)]
    pub(super) fn accept_wallpaper_for_test(&mut self, effect: Effect, cx: &mut Context<Self>) {
        let mut settings = default_settings(&self.project.join("wallpaper-ready-fixture")).unwrap();
        settings.effect = effect;
        let bytes = fs::read(settings.path.as_ref().unwrap()).unwrap();
        let pixels = image::load_from_memory(&bytes)
            .unwrap()
            .thumbnail(64, 64)
            .into_rgba8();
        let image = render_configured(pixels, effect, self.empty, settings.fade, settings.coverage);
        let viewport = self.viewport.unwrap_or((64, 64));
        self.viewport = Some(viewport);
        self.receive(
            Event::Ready {
                generation: self.generation,
                settings,
                image: Some(image),
                animated: false,
                empty: self.empty,
                viewport: Some(viewport),
            },
            cx,
        );
    }

    #[cfg(test)]
    pub(super) fn prepared_wallpaper_image_for_test(&self) -> Option<Arc<RenderImage>> {
        self.image.as_ref().map(|frame| frame.image.clone())
    }

    #[cfg(test)]
    pub(super) fn wallpaper_effect_for_test(&self) -> Effect {
        self.settings.effect
    }

    #[cfg(test)]
    pub(super) fn wallpaper_reveal_not_before_for_test(&self) -> Option<Instant> {
        self.wallpaper_reveal.reveal.as_ref().map(ImageDitherReveal::not_before)
    }

    pub(super) fn awaiting_opening_wallpaper(&self) -> bool {
        self.image.is_none()
            && (self.busy || self.resize_pending)
            && (self.settings.path.is_none() || self.visible())
    }

    pub(super) fn workspace_presentation_changed(&mut self, motion: bool, cx: &mut Context<Self>) {
        self.opening_motion = motion;
        cx.notify();
    }

    fn opening_wallpaper(
        &mut self,
        image: Option<Arc<WallpaperFrame>>,
    ) -> Option<(Arc<RenderImage>, Option<ImageDitherReveal>)> {
        let opening = crate::unified::workspace_frame(self.opening_motion).wallpaper_opening();
        let reveal = self.wallpaper_reveal.resolve(
            opening,
            image.is_some(),
            crate::workspace_motion::now(),
            Instant::now(),
        );
        image.map(|frame| (frame.image.clone(), reveal))
    }
}

/// One finite opening per native presentation. Image loading never delays the
/// workspace, and a current image joins with a full reveal when it can paint.
#[derive(Default)]
struct WallpaperReveal {
    opening: Option<u64>,
    reveal: Option<ImageDitherReveal>,
    completed: bool,
}
impl WallpaperReveal {
    fn resolve(
        &mut self,
        opening: Option<(f64, bool)>,
        image_ready: bool,
        raw_now: f64,
        now: Instant,
    ) -> Option<ImageDitherReveal> {
        let Some((origin, motion)) = opening else {
            *self = Self::default();
            return None;
        };
        if self.opening != Some(origin.to_bits()) {
            *self = Self {
                opening: Some(origin.to_bits()),
                ..Self::default()
            };
        }
        if !motion {
            self.reveal = None;
            self.completed = true;
            return None;
        }
        if self.completed || !image_ready {
            return None;
        }
        let reveal = self.reveal.get_or_insert_with(|| {
            // Native contour origins share this clock. Clamp future/malformed
            // values so incoming presentation metadata cannot overflow Instant.
            let delay = (origin + 0.08 - raw_now).max(0.).min(0.08);
            ImageDitherReveal::new(
                now + Duration::from_secs_f64(delay),
                Duration::from_millis(600),
            )
        });
        if reveal
            .started()
            .is_some_and(|started| now.saturating_duration_since(started) >= reveal.duration())
        {
            self.reveal = None;
            self.completed = true;
            return None;
        }
        Some(reveal.clone())
    }
}

#[cfg(test)]

fn dither_reveal_frame(source: &RenderImage, progress: f32) -> Arc<RenderImage> {
    let size = source.size(0);
    let (width, height) = (size.width.0 as u32, size.height.0 as u32);
    let mut pixels = RgbaImage::from_raw(width, height, source.as_bytes(0).unwrap().to_vec())
        .expect("wallpaper frame contains one BGRA pixel per texel");

    // Keep the same 2×2 cells as the wallpaper's dither pass. A stable
    // threshold gives each cell its own arrival time and keeps it visible.
    for (y, row) in pixels.chunks_mut(width as usize * 8).enumerate() {
        let (top, bottom) = row.split_at_mut(width as usize * 4);
        for (x, cell) in top.chunks_mut(8).enumerate() {
            if quantization_threshold(x, y, 0x71ea1439) >= progress {
                for alpha in cell.iter_mut().skip(3).step_by(4) {
                    *alpha = 0;
                }
                for alpha in bottom.iter_mut().skip(x * 8 + 3).take(5).step_by(4) {
                    *alpha = 0;
                }
            }
        }
    }
    Arc::new(RenderImage::new(vec![image::Frame::new(pixels)]))
}

impl Render for ChatBackground {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        BACKGROUND_RENDERS.with(|count| count.set(count.get() + 1));
        let this = cx.entity().downgrade();
        let measured = self.measured_viewport.clone();
        let surfaces = self.surfaces;
        let surface_bounds = self.surface_bounds.clone();
        let empty = self.empty;
        let image = self.visible().then(|| self.image.clone()).flatten();
        let image = self.opening_wallpaper(image);
        let opacity = if empty {
            self.settings.empty_opacity
        } else {
            self.settings.session_opacity
        };
        let classic = self.settings.fade == Fade::Classic;
        let presented = self.presented_fade;
        let duration = crate::workspace_motion::pane_duration();
        let transition = self.fade_transition.as_ref().and_then(|transition| {
            let elapsed = transition.started.elapsed();
            (elapsed < duration && !crate::native::reduced_motion()).then(|| (
                transition.previous.image.clone(), transition.from,
                crate::workspace_motion::ease_out(elapsed.as_secs_f32() / duration.as_secs_f32()),
            ))
        });
        if transition.is_some() {
            let owner = cx.weak_entity();
            window.on_next_frame(move |_, cx| {
                let _ = owner.update(cx, |_, cx| cx.notify());
            });
        } else {
            self.clear_fade_transition(cx);
        }
        div()
            .size_full()
            .debug_selector(|| "chat-background-artwork".into())
            // Canvas::paint does not establish element opacity in GPUI 0.2.2;
            // Div does. Fade the image's parent without re-decoding its pixels.
            .opacity(opacity)
            .child(
                canvas(
                    move |bounds, window, cx| {
                        // Fit once against the window, then let the workspace clip reveal
                        // the image as sidebars, docks, and chat panes change.
                        let viewport = window.viewport_size();
                        let origin = point(px(0.), bounds.top());
                        let window_bounds = Bounds::new(origin, size(viewport.width, (viewport.height - origin.y).max(px(0.))));
                        let bounds = fade_bounds(window_bounds, if classic { Fade::Classic } else { Fade::Full });
                        if bounds.size.width <= px(0.) || bounds.size.height <= px(0.) {
                            return window_bounds;
                        }
                        let scale = window.scale_factor();
                        let viewport = (
                            (f32::from(bounds.size.width) * scale).round().max(1.) as u32,
                            (f32::from(bounds.size.height) * scale).round().max(1.) as u32,
                        );
                        if measured.replace(Some(viewport)) != Some(viewport) {
                            cx.defer(move |cx| {
                                let _ = this.update(cx, |this, cx| this.set_viewport(viewport, cx));
                            });
                        }
                        window_bounds
                    },
                    move |_, viewport, window, _| {
                        if let Some((image, reveal)) = image {
                            let mut bounds = fade_bounds(viewport, presented);
                            if let Some((_, from, progress)) = &transition {
                                let previous = fade_bounds(viewport, *from);
                                bounds.size.height = previous.size.height
                                    + (bounds.size.height - previous.size.height) * *progress;
                            }
                            let paint = |window: &mut Window| {
                                if let Some((previous, _, progress)) = &transition {
                                    window.with_element_opacity(Some(1. - progress), |window| {
                                        let _ = window.paint_image(bounds, Corners::default(), previous.clone(), 0, false);
                                    });
                                    window.with_element_opacity(Some(*progress), |window| {
                                        window.paint_image(bounds, Corners::default(), image.clone(), 0, false)
                                    })
                                } else if let Some(reveal) = reveal.as_ref() {
                                    window.paint_image_with_dither_reveal(bounds, Corners::default(), image.clone(), 0, false, reveal.clone())
                                } else {
                                    window.paint_image(bounds, Corners::default(), image.clone(), 0, false)
                                }
                            };
                            if surfaces.0 || surfaces.1 {
                                let main = surface_bounds.get();
                                // One image in window coordinates; only its reveal mask changes.
                                let viewport = window.viewport_size();
                                let mut masks = vec![main];
                                if surfaces.0 {
                                    masks.push(Bounds::new(point(px(0.), px(0.)), size(main.left(), main.bottom())));
                                    masks.push(Bounds::new(point(main.right(), px(0.)), size((viewport.width-main.right()).max(px(0.)), main.bottom())));
                                }
                                if surfaces.1 {
                                    masks.push(Bounds::new(point(main.left(), px(0.)), size(main.size.width, main.top())));
                                    masks.push(Bounds::new(point(px(0.), main.bottom()), size(viewport.width, (viewport.height-main.bottom()).max(px(0.)))));
                                }
                                for mask in masks {
                                    if mask.size.width <= px(0.) || mask.size.height <= px(0.) { continue; }
                                    window.with_content_mask(Some(ContentMask { bounds: mask }), |window| {
                                        let _ = paint(window);
                                    });
                                }
                            } else {
                                let _ = paint(window);
                            }
                        }
                    },
                )
                .size_full(),
            )
    }
}
fn fade_bounds(bounds: Bounds<Pixels>, fade: Fade) -> Bounds<Pixels> {
    if fade == Fade::Classic { wallpaper_bounds(bounds, false) } else { bounds }
}

fn wallpaper_bounds(mut bounds: Bounds<Pixels>, empty: bool) -> Bounds<Pixels> {
    // Keep artwork local to the top of the pane, clearing the composer/transcript.
    bounds.size.height = (bounds.size.height * if empty { 0.78 } else { 0.72 }).min(px(if empty {
        780.
    } else {
        700.
    }));
    bounds
}

fn key(project: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut hash = 0xcbf29ce484222325u64;
    for byte in project.as_os_str().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}
fn settings_path(data: &Path, project: &Path) -> PathBuf {
    data.join(format!("{}.json", key(project)))
}
fn read_settings(data: &Path, project: &Path) -> std::io::Result<Settings> {
    // A global choice deliberately takes precedence over existing project
    // preferences, so Apply to all really updates every chat without copying
    // settings or decoded images into each project.
    let global = data.join("global.json");
    let everywhere = global.exists();
    let path = if everywhere {
        global
    } else {
        settings_path(data, project)
    };
    let mut settings = if path.exists() {
        let mut bytes = vec![];
        File::open(path)?
            .take(64 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > 64 * 1024 {
            return Err(std::io::Error::other("Background settings are too large"));
        }
        serde_json::from_slice::<Settings>(&bytes).map_err(std::io::Error::other)?
    } else {
        default_settings(data)?
    };
    settings.everywhere = everywhere;
    settings.normalize();
    if settings
        .path
        .as_ref()
        .is_some_and(|path| path.parent() != Some(data))
    {
        settings.path = None;
    }
    Ok(settings)
}
fn save_settings(data: &Path, project: &Path, settings: &Settings) -> std::io::Result<()> {
    fs::create_dir_all(data)?;
    let path = if settings.everywhere {
        data.join("global.json")
    } else {
        settings_path(data, project)
    };
    let tmp = path.with_extension("next");
    fs::write(
        &tmp,
        serde_json::to_vec(settings).map_err(std::io::Error::other)?,
    )?;
    fs::rename(tmp, path)
}
// Scope changes share the managed source instead of duplicating image data.
// Only inspect small settings documents when replacing/removing artwork, never
// on rendering or project switches. Unreadable settings conservatively retain it.
fn remove_unused_source(data: &Path, source: &Path) {
    if source.parent() != Some(data) || source == data.join("bonsai-sakura.png") {
        return;
    }
    let unreferenced = (|| -> std::io::Result<bool> {
        for entry in fs::read_dir(data)? {
            let path = entry?.path();
            if path.extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            let mut bytes = Vec::new();
            File::open(path)?
                .take(64 * 1024 + 1)
                .read_to_end(&mut bytes)?;
            if bytes.len() > 64 * 1024 {
                return Ok(false);
            }
            let settings: Settings =
                serde_json::from_slice(&bytes).map_err(std::io::Error::other)?;
            if settings.path.as_deref() == Some(source) {
                return Ok(false);
            }
        }
        Ok(true)
    })();
    if matches!(unreferenced, Ok(true)) {
        let _ = fs::remove_file(source);
    }
}
fn read_source(path: &Path) -> std::io::Result<Arc<[u8]>> {
    let mut file = File::open(path)?;
    let metadata = file.metadata()?;
    if metadata.len() > SOURCE_LIMIT {
        return Err(std::io::Error::other(
            "Background images can be up to 32 MB",
        ));
    }
    if !metadata.is_file() {
        return Err(std::io::Error::other("Choose an image file"));
    }
    let mut bytes = vec![];
    file.by_ref()
        .take(SOURCE_LIMIT + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > SOURCE_LIMIT {
        return Err(std::io::Error::other(
            "Background images can be up to 32 MB",
        ));
    }
    Ok(Arc::from(bytes))
}
fn limits() -> image::Limits {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16384);
    limits.max_image_height = Some(16384);
    limits.max_alloc = Some(128 * 1024 * 1024);
    limits
}
fn frame_stream(bytes: Arc<[u8]>) -> std::io::Result<image::Frames<'static>> {
    let mut decoder =
        image::codecs::gif::GifDecoder::new(Cursor::new(bytes)).map_err(std::io::Error::other)?;
    if decoder.dimensions().0 > 2048 || decoder.dimensions().1 > 2048 {
        return Err(std::io::Error::other(
            "Animated GIF canvases can be up to 2048 × 2048 pixels",
        ));
    }
    decoder
        .set_limits(limits())
        .map_err(std::io::Error::other)?;
    Ok(decoder.into_frames())
}
fn resize(pixels: &RgbaImage, side: u32) -> RgbaImage {
    let (w, h) = pixels.dimensions();
    if w <= side && h <= side {
        return pixels.clone();
    }
    let scale = (side as f64 / w as f64).min(side as f64 / h as f64);
    image::imageops::resize(
        pixels,
        (w as f64 * scale).round().max(1.) as u32,
        (h as f64 * scale).round().max(1.) as u32,
        image::imageops::FilterType::Triangle,
    )
}
fn resize_owned(pixels: RgbaImage, side: u32) -> RgbaImage {
    if pixels.width() <= side && pixels.height() <= side {
        pixels
    } else {
        resize(&pixels, side)
    }
}
fn cover_pixels(pixels: &RgbaImage, viewport: Option<(u32, u32)>, side: u32) -> RgbaImage {
    let Some((width, height)) = viewport else {
        return resize(pixels, side);
    };
    let width = width.max(1);
    let height = height.max(1);
    let aspect = width as f64 / height as f64;
    let source_aspect = pixels.width() as f64 / pixels.height().max(1) as f64;
    let (crop_width, crop_height) = if source_aspect > aspect {
        (
            (pixels.height() as f64 * aspect)
                .round()
                .clamp(1., pixels.width() as f64) as u32,
            pixels.height(),
        )
    } else {
        (
            pixels.width(),
            (pixels.width() as f64 / aspect)
                .round()
                .clamp(1., pixels.height() as f64) as u32,
        )
    };
    let scale = (side as f64 / width as f64)
        .min(side as f64 / height as f64)
        .min(1.);
    let crop = image::imageops::crop_imm(
        pixels,
        (pixels.width() - crop_width) / 2,
        (pixels.height() - crop_height) / 2,
        crop_width,
        crop_height,
    );
    image::imageops::resize(
        &*crop,
        (width as f64 * scale).round().max(1.) as u32,
        (height as f64 * scale).round().max(1.) as u32,
        image::imageops::FilterType::Triangle,
    )
}
fn wallpaper_alpha(position: f32, empty: bool) -> f32 {
    // A long, continuous tail: zero slope/curvature at both ends avoids the
    // visible seams of a piecewise-linear mask, including in occupied chats.
    smootherstep((1. - position) / if empty { 0.92 } else { 0.96 })
}
fn smootherstep(t: f32) -> f32 {
    let t = f64::from(t.clamp(0., 1.));
    (t * t * t * (t * (t * 6. - 15.) + 10.)) as f32
}
fn fade_haze(source: &RgbaImage) -> image::Rgba32FImage {
    // One small worker-side raster, regardless of source size. Premultiplying
    // before filtering prevents transparent artwork from acquiring dark halos.
    if source.width() == 0 || source.height() == 0 {
        return image::Rgba32FImage::new(1, 1);
    }
    let scale = (FADE_HAZE_SIDE as f32 / source.width().max(source.height()).max(1) as f32).min(1.);
    let width = (source.width() as f32 * scale).round().max(1.) as u32;
    let height = (source.height() as f32 * scale).round().max(1.) as u32;
    let small = image::Rgba32FImage::from_fn(width, height, |x, y| {
        let left = x * source.width() / width;
        let right = ((x + 1) * source.width() / width).max(left + 1);
        let top = y * source.height() / height;
        let bottom = ((y + 1) * source.height() / height).max(top + 1);
        let mut rgba = [0.; 4];
        for row in top..bottom {
            for col in left..right {
                let pixel = source.get_pixel(col, row);
                for channel in 0..3 {
                    rgba[channel] += pixel[channel] as f32 * pixel[3] as f32 / 255.;
                }
                rgba[3] += pixel[3] as f32;
            }
        }
        let count = ((right - left) * (bottom - top)) as f32;
        image::Rgba(rgba.map(|value| value / count))
    });
    image::imageops::blur(&small, 3.5)
}
fn sample_haze(image: &image::Rgba32FImage, x: f32, y: f32) -> [f32; 3] {
    let left = x.floor() as u32;
    let top = y.floor() as u32;
    let right = (left + 1).min(image.width() - 1);
    let bottom = (top + 1).min(image.height() - 1);
    let dx = x - left as f32;
    let dy = y - top as f32;
    let corners = [
        (image.get_pixel(left, top), (1. - dx) * (1. - dy)),
        (image.get_pixel(right, top), dx * (1. - dy)),
        (image.get_pixel(left, bottom), (1. - dx) * dy),
        (image.get_pixel(right, bottom), dx * dy),
    ];
    let mut rgba = [0.; 4];
    for (pixel, weight) in corners {
        for channel in 0..4 {
            rgba[channel] += pixel[channel] * weight;
        }
    }
    if rgba[3] <= 0. {
        return [0.; 3];
    }
    std::array::from_fn(|channel| (rgba[channel] * 255. / rgba[3]).min(255.))
}
#[cfg(test)]
fn fade_alpha(pixels: &mut RgbaImage, empty: bool) {
    fade_pixels::<false>(pixels, empty, None, Fade::Classic, 100);
}
// Stable, spatially decorrelated thresholds for the final 8-bit conversion.
// Stochastic rounding retains fractional coverage across adjacent pixels instead
// of turning the smooth fade into entire rows of identical 8-bit alpha. Unlike
// animated grain, the pattern is unchanged across GIF frames and redraws.
pub(super) fn quantization_threshold(x: usize, y: usize, salt: u32) -> f32 {
    let mut hash = (x as u32).wrapping_mul(0x9e3779b9)
        ^ (y as u32).wrapping_mul(0x85ebca6b) ^ salt;
    hash ^= hash >> 16;
    hash = hash.wrapping_mul(0x7feb352d);
    hash ^= hash >> 15;
    hash = hash.wrapping_mul(0x846ca68b);
    hash ^= hash >> 16;
    (hash >> 8) as f32 / 16_777_216.
}
fn quantize_coverage(value: f32, threshold: f32) -> u8 {
    (value.clamp(0., 255.) + threshold).floor().min(255.) as u8
}
fn fade_pixels<const BGRA: bool>(
    pixels: &mut RgbaImage,
    empty: bool,
    haze: Option<&image::Rgba32FImage>,
    fade: Fade,
    coverage: u8,
) {
    let width = pixels.width() as usize * 4;
    if width == 0 {
        return;
    }
    let last = pixels.height().saturating_sub(1).max(1) as f32;
    let x_scale = haze.map_or(0., |haze| {
        haze.width().saturating_sub(1) as f32 / pixels.width().saturating_sub(1).max(1) as f32
    });
    for (y, row) in pixels.chunks_exact_mut(width).enumerate() {
        let position = y as f32 / last;
        let last_x = (width / 4).saturating_sub(1).max(1) as f32;
        for (x, pixel) in row.chunks_exact_mut(4).enumerate() {
            let axis = match fade {
                Fade::Classic | Fade::Down | Fade::Full => position,
                Fade::Up => 1. - position,
                Fade::Right => x as f32 / last_x,
                Fade::Left => 1. - x as f32 / last_x,
            };
            let extent = f32::from(coverage) / 100.;
            let alpha = if coverage == 0 { 0. } else if fade == Fade::Full && coverage == 100 { 1. }
                else { wallpaper_alpha(axis / extent, empty) };
            let soften = if fade == Fade::Full { 0. } else { smootherstep((axis - 0.18) / 0.66) };
            if soften > 0.
                && alpha > 0.
                && pixel[3] > 0
                && let Some(haze) = haze
            {
                let soft = sample_haze(
                    haze,
                    x as f32 * x_scale,
                    position * haze.height().saturating_sub(1) as f32,
                );
                let threshold = quantization_threshold(x, y, 0x68bc21eb);
                for channel in 0..3 {
                    pixel[channel] = quantize_coverage(pixel[channel] as f32 * (1. - soften)
                        + soft[channel] * soften, threshold);
                }
            }
            pixel[3] = quantize_coverage(pixel[3] as f32 * alpha,
                quantization_threshold(x, y, 0x02e5be93));
            if BGRA {
                pixel.swap(0, 2);
            }
        }
    }
}
#[cfg(test)]
fn render_image(pixels: RgbaImage, effect: Effect, empty: bool) -> Arc<WallpaperFrame> {
    render_configured(pixels, effect, empty, Fade::Classic, 100)
}
fn render_configured(mut pixels: RgbaImage, effect: Effect, empty: bool, fade: Fade, coverage: u8) -> Arc<WallpaperFrame> {
    // None already owns its pixels. Avoid another full-size raster allocation.
    if effect != Effect::None {
        pixels = effects(&pixels, effect, empty);
    }
    let mut native = resize(&pixels, 512);
    if fade != Fade::Classic || coverage != 100 {
        fade_pixels::<false>(&mut native, empty, None, fade, coverage);
    }
    let native = Some(native);
    // Haze belongs to the fade, independently of the selected artwork effect.
    // Blend, alpha fade, and upload conversion share the existing in-place pass.
    let haze = fade_haze(&pixels);
    fade_pixels::<true>(&mut pixels, empty, Some(&haze), fade, coverage);
    Arc::new(WallpaperFrame{image:Arc::new(RenderImage::new(vec![image::Frame::new(pixels)])),native})
}
struct Animation {
    viewport: Option<(u32, u32)>,
    current: RgbaImage,
    finished: bool,
    bytes: Arc<[u8]>,
    frames: image::Frames<'static>,
    remaining: Option<u16>,
    delay: Duration,
}
fn gif_repeats(bytes: &[u8]) -> Option<u16> {
    let at = bytes
        .windows(11)
        .position(|window| window == b"NETSCAPE2.0")?
        + 11;
    if bytes.get(at..at + 3) == Some(&[3, 1, 0]) && bytes.get(at + 3) == Some(&0) {
        return Some(0);
    }
    if bytes.get(at) == Some(&3) && bytes.get(at + 1) == Some(&1) {
        return Some(u16::from_le_bytes([
            *bytes.get(at + 2)?,
            *bytes.get(at + 3)?,
        ]));
    }
    None
}
fn frame_delay(frame: &image::Frame) -> Duration {
    let (n, d) = frame.delay().numer_denom_ms();
    Duration::from_secs_f64((n as f64 / d.max(1) as f64 / 1000.).max(0.01))
}
#[cfg(test)]
thread_local! {
    static PREPARE_CALLS: Cell<usize> = const { Cell::new(0) };
}
#[cfg(test)]
fn prepare(
    bytes: Arc<[u8]>,
    settings: &Settings,
    empty: bool,
) -> std::io::Result<(Arc<WallpaperFrame>, Option<Animation>)> {
    prepare_cached(bytes, settings, empty, None, &mut None)
}
fn prepare_cached(
    bytes: Arc<[u8]>,
    settings: &Settings,
    empty: bool,
    viewport: Option<(u32, u32)>,
    decoded: &mut Option<RgbaImage>,
) -> std::io::Result<(Arc<WallpaperFrame>, Option<Animation>)> {
    #[cfg(test)]
    PREPARE_CALLS.with(|calls| calls.set(calls.get() + 1));
    if image::guess_format(&bytes).ok() == Some(image::ImageFormat::Gif) {
        let mut frames = frame_stream(bytes.clone())?;
        let first = frames
            .next()
            .ok_or_else(|| std::io::Error::other("GIF has no frames"))?
            .map_err(std::io::Error::other)?;
        let delay = frame_delay(&first);
        let current = resize_owned(first.into_buffer(), ANIMATED_SIDE);
        let image = render_configured(
            cover_pixels(&current, viewport, ANIMATED_SIDE),
            settings.effect,
            empty, settings.fade, settings.coverage,
        );
        let repeats = gif_repeats(&bytes);
        let remaining = match repeats {
            Some(0) => None,
            Some(n) => Some(n),
            None => Some(0),
        };
        Ok((
            image,
            Some(Animation {
                viewport,
                current,
                finished: false,
                bytes,
                frames,
                remaining,
                delay,
            }),
        ))
    } else {
        if decoded.is_none() {
            *decoded = Some(static_pixels(&bytes)?);
        }
        let pixels = cover_pixels(decoded.as_ref().unwrap(), viewport, STATIC_SIDE);
        Ok((render_configured(pixels, settings.effect, empty, settings.fade, settings.coverage), None))
    }
}
fn next_frame(
    animation: &mut Animation,
    settings: &Settings,
    empty: bool,
) -> std::io::Result<Option<Arc<WallpaperFrame>>> {
    if animation.finished {
        return Ok(None);
    }
    let frame = match animation.frames.next() {
        Some(frame) => frame,
        None => {
            if animation.remaining == Some(0) {
                animation.finished = true;
                // Release the decoder's large composition canvas, retaining
                // only the bounded last frame for later scope/resize changes.
                animation.frames = image::Frames::new(Box::new(std::iter::empty()));
                return Ok(None);
            }
            if let Some(remaining) = &mut animation.remaining {
                *remaining -= 1;
            }
            animation.frames = frame_stream(animation.bytes.clone())?;
            let Some(frame) = animation.frames.next() else {
                return Ok(None);
            };
            frame
        }
    }
    .map_err(std::io::Error::other)?;
    animation.delay = frame_delay(&frame);
    animation.current = resize_owned(frame.into_buffer(), ANIMATED_SIDE);
    Ok(Some(render_configured(
        cover_pixels(&animation.current, animation.viewport, ANIMATED_SIDE),
        settings.effect,
        empty, settings.fade, settings.coverage,
    )))
}

fn worker(data: PathBuf, rx: mpsc::Receiver<Command>, events: async_channel::Sender<Event>) {
    let mut config_state: Option<Config> = None;
    let mut animation: Option<Animation> = None;
    let mut source: Option<(PathBuf, Arc<[u8]>, Option<RgbaImage>)> = None;
    let mut active = false;
    let mut next_frame_at = Instant::now();
    let mut pending = None;
    loop {
        let command = if pending.is_some() {
            pending.take()
        } else if active
            && animation
                .as_ref()
                .is_some_and(|animation| !animation.finished)
        {
            match rx.recv_timeout(next_frame_at.saturating_duration_since(Instant::now())) {
                Ok(command) => Some(command),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(_) => break,
            }
        } else {
            match rx.recv() {
                Ok(command) => Some(command),
                Err(_) => break,
            }
        };
        let Some(command) = command else {
            let Some(current) = &config_state else {
                continue;
            };
            match next_frame(
                animation.as_mut().unwrap(),
                &current.settings,
                current.empty,
            ) {
                Ok(Some(image)) => {
                    next_frame_at += animation.as_ref().unwrap().delay;
                    if events
                        .send_blocking(Event::Frame(current.generation, image))
                        .is_err()
                    {
                        break;
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    let _ =
                        events.send_blocking(Event::Error(current.generation, error.to_string()));
                    animation = None;
                }
            }
            continue;
        };
        let mut persist = false;
        let mut prepared = None;
        let mut current = match command {
            Command::Shutdown => break,
            Command::Active(visible) => {
                if visible
                    && !active
                    && let Some(animation) = &animation
                {
                    next_frame_at = Instant::now() + animation.delay;
                }
                active = visible;
                continue;
            }
            Command::Save {
                project,
                mut settings,
                active: mut playback_active,
            } => {
                // Continuous pointer drags retain only the newest consecutive
                // revision. Stop at other commands to preserve actor ordering.
                while let Ok(command) = rx.try_recv() {
                    match command {
                        Command::Save {
                            project: next_project,
                            settings: next_settings,
                            active: next_active,
                        } if next_project == project => {
                            settings = next_settings;
                            playback_active = next_active;
                        }
                        other => {
                            pending = Some(other);
                            break;
                        }
                    }
                }
                if let Err(error) = save_settings(&data, &project, &settings)
                    && let Some(current) = &config_state
                {
                    let _ =
                        events.send_blocking(Event::Error(current.generation, error.to_string()));
                }
                if let Some(current) = &mut config_state
                    && current.project == project
                {
                    current.settings = settings;
                }
                if playback_active
                    && !active
                    && let Some(animation) = &animation
                {
                    next_frame_at = Instant::now() + animation.delay;
                }
                active = playback_active;
                continue;
            }
            Command::Load {
                generation,
                project,
                empty,
                active: window_active,
                viewport,
            } => match read_settings(&data, &project) {
                Ok(settings) => Config {
                    generation,
                    project,
                    settings,
                    empty,
                    active: window_active,
                    viewport,
                },
                Err(error) => {
                    // The UI has switched projects and discarded the old image.
                    // Stop its decoder too rather than emit stale GIF frames.
                    animation = None;
                    source = None;
                    config_state = None;
                    active = false;
                    let _ = events.send_blocking(Event::Error(generation, error.to_string()));
                    continue;
                }
            },
            Command::Scope(config) => {
                let result =
                    save_settings(&data, &config.project, &config.settings).and_then(|_| {
                        if !config.settings.everywhere {
                            match fs::remove_file(data.join("global.json")) {
                                Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                                    return Err(error);
                                }
                                _ => {}
                            }
                        }
                        Ok(())
                    });
                if let Err(error) = result {
                    let _ =
                        events.send_blocking(Event::Error(config.generation, error.to_string()));
                    continue;
                }
                config
            }
            Command::Configure(mut config) => {
                // Resize/empty-pane updates with the same persisted settings
                // collapse to their latest geometry before doing pixel work.
                while let Ok(command) = rx.try_recv() {
                    match command {
                        Command::Configure(next)
                            if next.project == config.project
                                && next.settings == config.settings =>
                        {
                            config = next
                        }
                        other => {
                            pending = Some(other);
                            break;
                        }
                    }
                }
                persist = true;
                config
            }
            Command::Import {
                mut config,
                source: original,
            } => {
                match (|| {
                    let bytes = read_source(&original)?;
                    let mut decoded = None;
                    let image = prepare_cached(
                        bytes.clone(),
                        &config.settings,
                        config.empty,
                        config.viewport,
                        &mut decoded,
                    )?;
                    fs::create_dir_all(&data)?;
                    let format = image::guess_format(&bytes).ok();
                    let extension = match format {
                        Some(image::ImageFormat::Gif) => "gif",
                        Some(image::ImageFormat::Jpeg) => "jpg",
                        Some(image::ImageFormat::WebP) => "webp",
                        Some(image::ImageFormat::Bmp) => "bmp",
                        Some(image::ImageFormat::Tiff) => "tiff",
                        _ => original
                            .extension()
                            .and_then(|extension| extension.to_str())
                            .filter(|extension| {
                                matches!(
                                    extension.to_ascii_lowercase().as_str(),
                                    "png"
                                        | "heic"
                                        | "heif"
                                        | "avif"
                                        | "ico"
                                        | "svg"
                                        | "svgz"
                                        | "tif"
                                        | "tiff"
                                )
                            })
                            .unwrap_or("image"),
                    };
                    let path = data.join(format!("{}.{}", uuid::Uuid::new_v4(), extension));
                    fs::write(&path, &bytes)?;
                    let old = config.settings.path.replace(path.clone());
                    if let Err(error) = save_settings(&data, &config.project, &config.settings) {
                        let _ = fs::remove_file(path);
                        return Err(error);
                    }
                    if let Some(old) = old {
                        remove_unused_source(&data, &old);
                    }
                    source = Some((path, bytes, decoded));
                    Ok::<_, std::io::Error>(image)
                })() {
                    Ok(image) => {
                        prepared = Some(image);
                        config
                    }
                    Err(error) => {
                        // Keep the previous artwork alive after a bad selection,
                        // and tag its frames with the UI's current generation.
                        if let Some(previous) = &mut config_state {
                            previous.generation = config.generation;
                            active =
                                config.active && (!previous.settings.empty_only || config.empty);
                            previous.empty = config.empty;
                            previous.active = config.active;
                            if let Some(animation) = &animation {
                                next_frame_at = Instant::now() + animation.delay;
                            }
                        }
                        let _ = events
                            .send_blocking(Event::Error(config.generation, error.to_string()));
                        continue;
                    }
                }
            }
            Command::Default(mut current) => {
                match default_settings(&data) {
                    Ok(mut settings) => {
                        settings.everywhere = current.settings.everywhere;
                        current.settings = settings;
                    }
                    Err(error) => {
                        let _ = events
                            .send_blocking(Event::Error(current.generation, error.to_string()));
                        continue;
                    }
                }
                persist = true;
                current
            }
            Command::Remove(mut current) => {
                let old = current.settings.path.take();
                current.settings = Settings {
                    everywhere: current.settings.everywhere,
                    ..Settings::default()
                };
                if let Err(error) = save_settings(&data, &current.project, &current.settings) {
                    let _ =
                        events.send_blocking(Event::Error(current.generation, error.to_string()));
                    continue;
                }
                if let Some(path) = old {
                    remove_unused_source(&data, &path);
                }
                current
            }
        };
        current.settings.normalize();
        active = current.active && (!current.settings.empty_only || current.empty);
        let result = (|| {
            if persist
                && !config_state.as_ref().is_some_and(|previous| {
                    previous.project == current.project && previous.settings == current.settings
                })
            {
                save_settings(&data, &current.project, &current.settings)?;
            }
            let Some(path) = &current.settings.path else {
                source = None;
                return Ok((None, None));
            };
            if !matches!(&source, Some((loaded, _, _)) if loaded == path) {
                source = Some((path.clone(), read_source(path)?, None));
            }
            let (_, bytes, decoded) = source.as_mut().unwrap();
            let (image, frames) = match prepared {
                Some(image) => image,
                None if animation.is_some()
                    && config_state.as_ref().is_some_and(|previous| {
                        (previous.project == current.project
                            || (previous.settings.everywhere && current.settings.everywhere))
                            && previous.settings.path == current.settings.path
                    }) =>
                {
                    let mut frames = animation.take().unwrap();
                    frames.viewport = current.viewport;
                    let pixels = cover_pixels(&frames.current, current.viewport, ANIMATED_SIDE);
                    (
                        render_configured(pixels, current.settings.effect, current.empty, current.settings.fade, current.settings.coverage),
                        Some(frames),
                    )
                }
                None => prepare_cached(
                    bytes.clone(),
                    &current.settings,
                    current.empty,
                    current.viewport,
                    decoded,
                )?,
            };
            Ok::<_, std::io::Error>((Some(image), frames))
        })();
        match result {
            Ok((image, frames)) => {
                animation = frames;
                if let Some(animation) = &animation {
                    next_frame_at = Instant::now() + animation.delay;
                }
                let animated = animation.is_some();
                if events
                    .send_blocking(Event::Ready {
                        generation: current.generation,
                        settings: current.settings.clone(),
                        image,
                        animated,
                        empty: current.empty,
                        viewport: current.viewport,
                    })
                    .is_err()
                {
                    break;
                }
                config_state = Some(current);
            }
            Err(error) => {
                let _ = events.send_blocking(Event::Error(current.generation, error.to_string()));
                animation = None;
                config_state = Some(current);
            }
        }
    }
}

fn static_pixels(bytes: &[u8]) -> std::io::Result<RgbaImage> {
    thumbnail_pixels(bytes, STATIC_SIDE)
}
fn thumbnail_pixels(bytes: &[u8], side:u32) -> std::io::Result<RgbaImage> {
    let side=side.clamp(1,STATIC_SIDE);
    // Raster metadata may contain an SVG thumbnail. Classify the actual file
    // signature first so an embedded XML fragment cannot misroute a PNG.
    if image::guess_format(bytes).is_ok() {
        return native_image::thumbnail(bytes, side);
    }
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]);
    if bytes.starts_with(&[0x1f, 0x8b]) {
        return Err(std::io::Error::other(
            "Compressed SVG backgrounds are not supported; choose an SVG file",
        ));
    }
    if head.contains("<svg") {
        if bytes.len() > SVG_LIMIT {
            return Err(std::io::Error::other("SVG backgrounds can be up to 4 MB"));
        }
        let options = svg_options();
        let tree = resvg::usvg::Tree::from_data(bytes, &options).map_err(std::io::Error::other)?;
        let size = tree.size();
        let scale = (side as f32 / size.width())
            .min(side as f32 / size.height())
            .min(1.);
        let width = (size.width() * scale).ceil().clamp(1., side as f32) as u32;
        let height = (size.height() * scale).ceil().clamp(1., side as f32) as u32;
        let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)
            .ok_or_else(|| std::io::Error::other("SVG has invalid dimensions"))?;
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::from_scale(scale, scale),
            &mut pixmap.as_mut(),
        );
        let mut pixels = pixmap.take();
        unpremultiply(&mut pixels);
        return RgbaImage::from_raw(width, height, pixels)
            .ok_or_else(|| std::io::Error::other("SVG raster conversion failed"));
    }
    native_image::thumbnail(bytes, side)
}
fn svg_options() -> resvg::usvg::Options<'static> {
    let embedded_pixels = std::sync::atomic::AtomicU64::new(0);
    static FONTS: OnceLock<Arc<resvg::usvg::fontdb::Database>> = OnceLock::new();
    let fonts = FONTS.get_or_init(|| {
        let mut fonts = resvg::usvg::fontdb::Database::new();
        // Known system fonts only: no project, user-font, or filesystem scan.
        for path in [
            "/System/Library/Fonts/Helvetica.ttc",
            "/System/Library/Fonts/Times.ttc",
            "/System/Library/Fonts/Menlo.ttc",
        ] {
            let _ = fonts.load_font_file(path);
        }
        fonts.set_sans_serif_family("Helvetica");
        fonts.set_serif_family("Times");
        fonts.set_monospace_family("Menlo");
        Arc::new(fonts)
    });
    resvg::usvg::Options {
        font_family: "Helvetica".into(),
        fontdb: fonts.clone(),
        image_href_resolver: resvg::usvg::ImageHrefResolver {
            resolve_string: Box::new(|_, _| None),
            resolve_data: Box::new(move |mime, data, options| {
                // Embedded rasters are allowed only when the renderer's decode
                // cannot exceed the same bounded background canvas.
                if data.len() > SVG_LIMIT {
                    return None;
                }
                let reader = image::ImageReader::new(Cursor::new(data.as_slice()))
                    .with_guessed_format()
                    .ok()?;
                let (width, height) = reader.into_dimensions().ok()?;
                if width > STATIC_SIDE || height > STATIC_SIDE {
                    return None;
                }
                // Bound the total raster work as well as each embedded image.
                let pixels = u64::from(width) * u64::from(height);
                embedded_pixels
                    .fetch_update(
                        std::sync::atomic::Ordering::Relaxed,
                        std::sync::atomic::Ordering::Relaxed,
                        |used| {
                            used.checked_add(pixels)
                                .filter(|sum| *sum <= 16 * 1024 * 1024)
                        },
                    )
                    .ok()?;
                resvg::usvg::ImageHrefResolver::default_data_resolver()(mime, data, options)
            }),
        },
        ..Default::default()
    }
}
fn unpremultiply(pixels: &mut [u8]) {
    for pixel in pixels.chunks_exact_mut(4) {
        let alpha = pixel[3];
        if alpha > 0 && alpha < 255 {
            for color in &mut pixel[..3] {
                *color = (*color as f32 * 255. / alpha as f32).round().min(255.) as u8;
            }
        }
    }
}

/// Exact dark-theme ports of Monocode's dither/ascii/halftone/scanline worker.
fn effects(source: &RgbaImage, effect: Effect, empty: bool) -> RgbaImage {
    if effect == Effect::GradientBlur {
        return haze(source, empty);
    }
    let (w, h) = source.dimensions();
    let mut output = source.clone();
    let index = |x: u32, y: u32| -> usize { ((y.min(h - 1) * w + x.min(w - 1)) * 4) as usize };
    let pixels = source.as_raw();
    let luma = |at: usize| -> f32 {
        (pixels[at] as f32 * 0.2126
            + pixels[at + 1] as f32 * 0.7152
            + pixels[at + 2] as f32 * 0.0722)
            .round()
    };
    match effect {
        Effect::None => {}
        Effect::Dither => {
            let bayer = [
                [0., 8., 2., 10.],
                [12., 4., 14., 6.],
                [3., 11., 1., 9.],
                [15., 7., 13., 5.],
            ];
            for y in (0..h).step_by(2) {
                for x in (0..w).step_by(2) {
                    let at = index(x + 1, y + 1);
                    let peak = pixels[at..at + 3].iter().max().copied().unwrap_or(0) as f32;
                    let gain = if peak / 255.
                        > (bayer[((y / 2) % 4) as usize][((x / 2) % 4) as usize] + 0.5) / 16.
                    {
                        255. / peak.max(1.)
                    } else {
                        0.08
                    };
                    let color = image::Rgba([
                        (pixels[at] as f32 * gain).round() as u8,
                        (pixels[at + 1] as f32 * gain).round() as u8,
                        (pixels[at + 2] as f32 * gain).round() as u8,
                        pixels[at + 3],
                    ]);
                    for dy in 0..2.min(h - y) {
                        for dx in 0..2.min(w - x) {
                            output.put_pixel(x + dx, y + dy, color);
                        }
                    }
                }
            }
        }
        Effect::Ascii => {
            let glyphs = [
                [0, 0, 0, 0, 0, 0, 0],
                [0, 0, 0, 0, 0, 4, 0],
                [0, 4, 0, 0, 4, 0, 0],
                [0, 0, 0, 14, 0, 0, 0],
                [0, 0, 14, 0, 14, 0, 0],
                [0, 4, 4, 31, 4, 4, 0],
                [0, 21, 14, 31, 14, 21, 0],
                [10, 10, 31, 10, 31, 10, 10],
                [17, 2, 4, 4, 8, 16, 17],
                [14, 17, 23, 21, 23, 16, 14],
            ];
            for y in 0..h {
                for x in 0..w {
                    let sample = index(x / 6 * 6 + 3, y / 8 * 8 + 4);
                    let glyph = glyphs[((luma(sample) / 255.).sqrt() * 9.).floor() as usize];
                    let ink = x % 6 < 5
                        && y % 8 < 7
                        && (glyph[(y % 8) as usize] & (1 << (4 - x % 6))) != 0;
                    let at = index(x, y);
                    let dest = output.get_pixel_mut(x, y);
                    for color in 0..3 {
                        dest[color] = (pixels[at + color] as f32 * 0.6
                            + if ink {
                                pixels[sample + color] as f32 * 0.4
                            } else {
                                0.
                            })
                        .round() as u8;
                    }
                }
            }
        }
        Effect::Halftone => {
            for y in (0..h).step_by(4) {
                for x in (0..w).step_by(4) {
                    let radius = 2. * (0.3 + 0.7 * (luma(index(x, y)) / 255.).sqrt());
                    let dot = index(x + 2, y + 2);
                    for dy in 0..4.min(h - y) {
                        for dx in 0..4.min(w - x) {
                            let distance =
                                ((dx as f32 - 1.5).powi(2) + (dy as f32 - 1.5).powi(2)).sqrt();
                            let coverage = (radius + 0.5 - distance).clamp(0., 1.)
                                * pixels[dot + 3] as f32
                                / 255.;
                            let at = index(x + dx, y + dy);
                            let dest = output.get_pixel_mut(x + dx, y + dy);
                            for color in 0..3 {
                                dest[color] = (pixels[at + color] as f32 * 0.6
                                    + pixels[dot + color] as f32 * coverage * 0.4)
                                    .round() as u8;
                            }
                        }
                    }
                }
            }
        }
        Effect::Scanlines => {
            for y in 0..h {
                if y % 3 == 0 {
                    for x in 0..w {
                        let dest = output.get_pixel_mut(x, y);
                        for color in 0..3 {
                            dest[color] = (dest[color] as f32 * 0.52).round() as u8;
                        }
                    }
                }
            }
        }
        Effect::GradientBlur => unreachable!("haze is handled before allocating output"),
    }
    output
}
fn ramp(y: f32, stops: &[(f32, f32)]) -> f32 {
    for pair in stops.windows(2) {
        if y <= pair[1].0 {
            let t = ((y - pair[0].0) / (pair[1].0 - pair[0].0)).clamp(0., 1.);
            return pair[0].1 + (pair[1].1 - pair[0].1) * smootherstep(t);
        }
    }
    stops.last().map(|s| s.1).unwrap_or(0.)
}
fn haze(source: &RgbaImage, empty: bool) -> RgbaImage {
    let source = resize(source, 1024);
    // Keep both blur passes in floating point, premultiplied for transparent
    // artwork. Quantize only after mixing the progressive blur/tint, never
    // between the horizontal/vertical filters or at intermediate blur levels.
    let floating = image::Rgba32FImage::from_fn(source.width(), source.height(), |x, y| {
        let pixel = source.get_pixel(x, y);
        let alpha = pixel[3] as f32 / 255.;
        image::Rgba([pixel[0] as f32 * alpha, pixel[1] as f32 * alpha,
            pixel[2] as f32 * alpha, alpha])
    });
    let soft = image::imageops::blur(&floating, if empty { 7. } else { 5. });
    let strong = image::imageops::blur(&floating, if empty { 18. } else { 12. });
    drop(floating);
    let mut output = source.clone();
    let (_, height) = source.dimensions();
    for (x, y, pixel) in output.enumerate_pixels_mut() {
        let p = y as f32 / height.max(1) as f32;
        let a = ramp(p, &[(0.18, 0.), (if empty { 0.60 } else { 0.70 }, 1.)]);
        let b = ramp(p, &[(if empty { 0.28 } else { 0.22 }, 0.), (0.76, 1.)]);
        let overlay = if empty {
            ramp(p, &[(0.16, 0.), (0.38, 0.26), (0.55, 0.55), (0.75, 0.78)])
        } else {
            ramp(p, &[(0.15, 0.), (0.76, 0.70)])
        };
        let unpremultiply = |blur: &image::Rgba32FImage, color: usize| {
            let sample = blur.get_pixel(x, y);
            if sample[3] > 0. { sample[color] / sample[3] } else { 0. }
        };
        let threshold = quantization_threshold(x as usize, y as usize, 0x68bc21eb);
        for color in 0..3 {
            let value = source.get_pixel(x, y)[color] as f32 * (1. - a)
                + unpremultiply(&soft, color) * a;
            let value = value * (1. - b) + unpremultiply(&strong, color) * b;
            pixel[color] = quantize_coverage(value * (1. - overlay) + 23. * overlay, threshold);
        }
        pixel[3] = source.get_pixel(x, y)[3];
    }
    output
}

#[cfg(test)]
mod tests {
    use super::{
        Command, Config, Effect, Event, Fade, SOURCE_LIMIT, Settings, effects, frame_stream, gif_repeats,
        haze, next_frame, prepare, read_settings, save_settings, visibility, worker,
    };
    use image::{
        Delay, Frame, Rgba, RgbaImage,
        codecs::gif::{GifEncoder, Repeat},
    };
    use std::{
        fs,
        path::PathBuf,
        sync::{Arc, mpsc},
        time::Duration,
    };
    fn fixture() -> PathBuf {
        let root = std::env::temp_dir().join(format!("orbit-background-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        root
    }
    fn gif(repeat: Option<Repeat>) -> Arc<[u8]> {
        let mut bytes = vec![];
        {
            let mut encoder = GifEncoder::new(&mut bytes);
            if let Some(repeat) = repeat {
                encoder.set_repeat(repeat).unwrap();
            }
            for color in [[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255]] {
                encoder
                    .encode_frame(Frame::from_parts(
                        RgbaImage::from_pixel(16, 16, Rgba(color)),
                        0,
                        0,
                        Delay::from_numer_denom_ms(40, 1),
                    ))
                    .unwrap();
            }
        }
        Arc::from(bytes)
    }
    #[test]
    fn monocode_effect_pixels_match_dark_worker_values_and_preserve_alpha() {
        let source = RgbaImage::from_pixel(16, 16, Rgba([128, 128, 128, 255]));
        let dither = effects(&source, Effect::Dither, true);
        assert_eq!(dither.get_pixel(0, 0).0, [255, 255, 255, 255]);
        assert_eq!(dither.get_pixel(2, 0).0, [10, 10, 10, 255]);
        assert_eq!(dither.get_pixel(0, 0), dither.get_pixel(1, 1));
        let halftone = effects(&source, Effect::Halftone, true);
        assert_eq!(halftone.get_pixel(0, 0)[0], 77);
        assert_eq!(halftone.get_pixel(1, 1)[0], 128);
        let scan = effects(&source, Effect::Scanlines, true);
        assert_eq!(scan.get_pixel(0, 0)[0], 67);
        assert_eq!(scan.get_pixel(0, 1)[0], 128);
        let white = RgbaImage::from_pixel(12, 8, Rgba([255, 255, 255, 99]));
        let ascii = effects(&white, Effect::Ascii, true);
        assert_eq!(ascii.get_pixel(0, 0).0, [153, 153, 153, 99]);
        assert_eq!(ascii.get_pixel(1, 0).0, [255, 255, 255, 99]);
        assert_eq!(effects(&source, Effect::None, true), source);
    }
    #[test]
    fn haze_has_real_blur_gradient_mask_and_distinct_session_treatment() {
        let mut source = RgbaImage::from_pixel(64, 128, Rgba([0, 0, 0, 255]));
        source.put_pixel(32, 64, Rgba([255, 0, 0, 255]));
        let mut empty = haze(&source, true);
        let mut session = haze(&source, false);
        super::fade_alpha(&mut empty, true);
        super::fade_alpha(&mut session, false);
        assert_eq!(empty.get_pixel(10, 127)[3], 0);
        assert_eq!(session.get_pixel(10, 127)[3], 0);
        assert_eq!(empty.get_pixel(10, 0)[3], 255);
        assert_ne!(empty, session);
        assert!(empty.get_pixel(33, 64)[0] > 0);
    }
    #[test]
    fn custom_fade_controls_direction_and_coverage() {
        let source = RgbaImage::from_pixel(32, 32, Rgba([90, 120, 180, 255]));
        for (fade, start, end) in [
            (super::Fade::Down, (15, 0), (15, 31)), (super::Fade::Up, (15, 31), (15, 0)),
            (super::Fade::Right, (0, 15), (31, 15)), (super::Fade::Left, (31, 15), (0, 15)),
        ] {
            let mut image = source.clone();
            super::fade_pixels::<false>(&mut image, false, None, fade, 25);
            assert_eq!(image.get_pixel(start.0, start.1)[3], 255);
            assert_eq!(image.get_pixel(end.0, end.1)[3], 0);
            assert_eq!(image.pixels().filter(|p| p[3] > 0).count() / 32, 8);
        }
        for (coverage, expected) in [(0, 0), (100, 255)] {
            let mut image = source.clone();
            super::fade_pixels::<false>(&mut image, false, None, super::Fade::Full, coverage);
            assert!(image.pixels().all(|p| p[3] == expected));
        }
    }

    #[test]
    fn fade_keeps_fractional_coverage_instead_of_full_width_quantization_bands() {
        let source = RgbaImage::from_pixel(1024, 512, Rgba([70, 140, 210, 123]));
        for empty in [true, false] {
            let frame = super::render_image(source.clone(), Effect::None, empty);
            let bytes = frame.as_bytes(0).unwrap();
            let again = super::render_image(source.clone(), Effect::None, empty);
            assert_eq!(bytes, again.as_bytes(0).unwrap(), "No time-varying grain/flicker");
            let mut checked = 0;
            for y in (40..480).step_by(7) {
                let expected = 123. * super::wallpaper_alpha(y as f32 / 511., empty);
                if !(0.2..0.8).contains(&expected.fract()) { continue; }
                let alphas: Vec<_> = bytes[y * 1024 * 4..(y + 1) * 1024 * 4]
                    .chunks_exact(4).map(|p| p[3]).collect();
                let low = *alphas.iter().min().unwrap();
                let high = *alphas.iter().max().unwrap();
                assert_eq!(high - low, 1, "A fractional fade must not become one solid-opacity band");
                let mean = alphas.iter().map(|a| *a as f32).sum::<f32>() / 1024.;
                assert!((mean - expected).abs() < 0.065, "Fractional coverage must preserve brightness: {mean} vs {expected}");
                checked += 1;
            }
            assert!(checked >= 20);
        }
    }
    #[test]
    fn gradient_blur_is_smooth_without_hidden_scanlines_and_preserves_transparent_hues() {
        let source = RgbaImage::from_pixel(160, 160, Rgba([100, 100, 100, 255]));
        let blurred = super::haze(&source, true);
        for y in 0..24 {
            let mean = (0..160).map(|x| blurred.get_pixel(x, y)[0] as f32).sum::<f32>() / 160.;
            assert!((mean - 100.).abs() < 0.02, "Gradient blur must not secretly darken every third row: {y}: {mean}");
        }
        let transparent = RgbaImage::from_fn(80, 80, |x, _| {
            if x < 40 { Rgba([240, 120, 60, 255]) } else { Rgba([0, 0, 0, 0]) }
        });
        let blurred = super::haze(&transparent, true);
        // At the same progressive tint, transparent black must not darken
        // the Gaussian's neighboring orange compared with opaque orange.
        let opaque = super::haze(&RgbaImage::from_pixel(80, 80, Rgba([240, 120, 60, 255])), true);
        let edge = blurred.get_pixel(39, 48);
        let reference = opaque.get_pixel(39, 48);
        for color in 0..3 {
            assert!((edge[color] as i16 - reference[color] as i16).abs() <= 1,
                "Transparent black must not contaminate blurred artwork: {edge:?} vs {reference:?}");
        }
        assert_eq!(blurred.get_pixel(40, 48)[3], 0);
        // Scanlines remain a separate, explicitly selected artwork effect.
        assert_ne!(effects(&source, Effect::Scanlines, true).get_pixel(0, 0), source.get_pixel(0, 0));
    }
    #[test]
    fn shared_fade_softens_detail_for_every_effect_and_preserves_the_crisp_top() {
        let source =
            RgbaImage::from_fn(80, 101, |x, _| Rgba([if x % 2 == 0 { 255 } else { 0 }; 4]));
        // Keep the striped artwork opaque so contrast measures RGB softening.
        let mut source = source;
        for pixel in source.pixels_mut() {
            pixel[3] = 255;
        }
        for effect in Effect::ALL {
            let base = effects(&source, effect, true);
            let rendered = super::render_image(source.clone(), effect, true);
            let pixels = rendered.as_bytes(0).unwrap();
            let at = |x: usize, y: usize| (y * 80 + x) * 4;
            for x in 0..80 {
                let pixel = base.get_pixel(x, 0);
                assert_eq!(
                    &pixels[at(x as usize, 0)..at(x as usize, 0) + 3],
                    &[pixel[2], pixel[1], pixel[0]]
                );
            }
            let original =
                (base.get_pixel(40, 82)[0] as i16 - base.get_pixel(41, 82)[0] as i16).abs();
            let softened = (pixels[at(40, 82) + 2] as i16 - pixels[at(41, 82) + 2] as i16).abs();
            assert!(
                softened <= original / 8 + 1,
                "detail must dissolve near the tail for {effect:?}"
            );
            assert!(
                pixels[at(40, 82) + 3] > 0,
                "the tail remains gradual rather than cutting off"
            );
        }
    }
    #[test]
    fn shared_haze_is_bounded_and_handles_transparent_edges_without_dark_halos() {
        let source = RgbaImage::from_fn(768, 300, |x, _| {
            if x < 384 {
                Rgba([240, 120, 60, 255])
            } else {
                Rgba([0, 0, 0, 0])
            }
        });
        let soft = super::fade_haze(&source);
        assert!(soft.width() <= super::FADE_HAZE_SIDE && soft.height() <= super::FADE_HAZE_SIDE);
        let rgb = super::sample_haze(&soft, 127., 50.);
        for (actual, expected) in rgb.into_iter().zip([240., 120., 60.]) {
            assert!(
                (actual - expected).abs() < 0.01,
                "transparent samples must not darken the artwork"
            );
        }
        let small = RgbaImage::from_pixel(1, 1, Rgba([1, 2, 3, 255]));
        assert_eq!(super::fade_haze(&small).dimensions(), (1, 1));
    }
    #[test]
    fn shared_fade_has_a_long_monotonic_tail_and_flat_endpoints() {
        for empty in [true, false] {
            let alpha = |p| super::wallpaper_alpha(p, empty);
            assert_eq!(alpha(0.), 1.);
            assert_eq!(alpha(1.), 0.);
            assert!(alpha(0.8) > 0. && alpha(0.8) < 0.1);
            assert!(alpha(0.999) < 0.00001);
            let samples: Vec<_> = (0..=1000).map(|i| alpha(i as f32 / 1000.)).collect();
            assert!(
                samples
                    .windows(2)
                    .all(|pair| pair[0] >= pair[1] && pair[0] - pair[1] < 0.003)
            );
        }
    }
    #[test]
    fn gif_stream_preserves_every_frame_delay_and_finite_loop_count() {
        let bytes = gif(Some(Repeat::Finite(1)));
        assert_eq!(gif_repeats(&bytes), Some(1));
        let settings = Settings::default();
        let (first, animation) = prepare(bytes, &settings, true).unwrap();
        let mut animation = animation.unwrap();
        assert_eq!(animation.delay, Duration::from_millis(40));
        let mut frames = vec![first];
        while let Some(frame) = next_frame(&mut animation, &settings, true).unwrap() {
            frames.push(frame);
            assert!(frames.len() <= 6);
        }
        assert_eq!(frames.len(), 6);
        assert_ne!(frames[0].as_bytes(0), frames[1].as_bytes(0));
        assert_eq!(frames[0].as_bytes(0), frames[3].as_bytes(0));
        assert_eq!(gif_repeats(&gif(None)), None);
        assert_eq!(gif_repeats(&gif(Some(Repeat::Infinite))), Some(0));
    }
    #[test]
    fn background_actor_imports_managed_artwork_and_pauses_animation() {
        let root = fixture();
        let data = root.join("managed");
        let original = root.join("source.gif");
        fs::write(&original, gif(Some(Repeat::Infinite))).unwrap();
        let (tx, rx) = mpsc::channel();
        let (events, received) = async_channel::bounded(1);
        let data_worker = data.clone();
        let handle = std::thread::spawn(move || {
            worker(data_worker, rx, events);
            super::PREPARE_CALLS.with(|calls| calls.get())
        });
        let project = root.join("workspace");
        tx.send(Command::Import {
            config: Config {
                generation: 7,
                project: project.clone(),
                settings: Settings::default(),
                empty: true,
                active: false,
                viewport: None,
            },
            source: original.clone(),
        })
        .unwrap();
        let saved = match received.recv_blocking().unwrap() {
            Event::Ready {
                generation,
                settings,
                image,
                animated,
                ..
            } => {
                assert_eq!(generation, 7);
                assert!(image.is_some() && animated);
                settings
            }
            Event::Error(_, error) => panic!("{error}"),
            _ => panic!("Expected prepared image"),
        };
        assert_eq!(saved.path.as_ref().unwrap().parent(), Some(data.as_path()));
        assert!(original.exists());
        std::thread::sleep(Duration::from_millis(70));
        assert!(received.try_recv().is_err());
        tx.send(Command::Active(true)).unwrap();
        assert!(matches!(
            received.recv_blocking().unwrap(),
            Event::Frame(7, _)
        ));
        tx.send(Command::Active(false)).unwrap();
        std::thread::sleep(Duration::from_millis(70));
        assert!(received.try_recv().is_err());
        assert_eq!(read_settings(&data, &project).unwrap().path, saved.path);
        let invalid = root.join("bad.gif");
        fs::write(&invalid, b"this is not an image").unwrap();
        tx.send(Command::Import {
            config: Config {
                generation: 8,
                project: project.clone(),
                settings: saved.clone(),
                empty: true,
                active: true,
                viewport: None,
            },
            source: invalid,
        })
        .unwrap();
        assert!(matches!(
            received.recv_blocking().unwrap(),
            Event::Error(8, _)
        ));
        assert!(matches!(
            received.recv_blocking().unwrap(),
            Event::Frame(8, _)
        ));
        let broken_project = root.join("broken-project");
        fs::write(
            super::settings_path(&data, &broken_project),
            b"invalid json",
        )
        .unwrap();
        tx.send(Command::Load {
            generation: 9,
            project: broken_project.clone(),
            empty: true,
            active: true,
            viewport: None,
        })
        .unwrap();
        assert!(matches!(
            received.recv_blocking().unwrap(),
            Event::Error(9, _)
        ));
        std::thread::sleep(Duration::from_millis(70));
        assert!(
            received.try_recv().is_err(),
            "failed project load must stop old GIF"
        );
        tx.send(Command::Remove(Config {
            generation: 10,
            project: project.clone(),
            settings: saved.clone(),
            empty: true,
            active: true,
            viewport: None,
        }))
        .unwrap();
        assert!(matches!(
            received.recv_blocking().unwrap(),
            Event::Ready {
                generation: 10,
                image: None,
                animated: false,
                ..
            }
        ));
        // Unreadable preferences could still reference shared artwork. Retain
        // the source until that document is repaired or removed.
        assert!(saved.path.as_ref().unwrap().exists());
        fs::remove_file(super::settings_path(&data, &broken_project)).unwrap();
        super::remove_unused_source(&data, saved.path.as_ref().unwrap());
        assert!(!saved.path.as_ref().unwrap().exists());
        assert!(original.exists());
        std::thread::sleep(Duration::from_millis(70));
        assert!(received.try_recv().is_err());
        tx.send(Command::Shutdown).unwrap();
        assert_eq!(
            handle.join().unwrap(),
            2,
            "valid import decodes once; invalid selection validates once"
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn unchanged_background_loads_do_not_rewrite_settings_and_saves_coalesce() {
        use std::os::unix::fs::MetadataExt;
        let root = fixture();
        let data = root.join("managed");
        let project = root.join("workspace");
        let (tx, rx) = mpsc::channel();
        let (events, received) = async_channel::bounded(1);
        // Queue real continuous-drag settings before starting the worker.
        for step in 0..=100 {
            tx.send(Command::Save {
                project: project.clone(),
                settings: Settings {
                    empty_opacity: step as f32 / 100.,
                    ..Default::default()
                },
                active: false,
            })
            .unwrap();
        }
        tx.send(Command::Load {
            generation: 1,
            project: project.clone(),
            empty: true,
            active: false,
            viewport: None,
        })
        .unwrap();
        let worker_data = data.clone();
        let handle = std::thread::spawn(move || worker(worker_data, rx, events));
        let settings = match received.recv_blocking().unwrap() {
            Event::Ready {
                generation: 1,
                settings,
                image: None,
                ..
            } => settings,
            _ => panic!("expected latest persisted settings"),
        };
        assert_eq!(settings.empty_opacity, 1.);
        let path = super::settings_path(&data, &project);
        let inode = fs::metadata(&path).unwrap().ino();
        tx.send(Command::Configure(Config {
            generation: 2,
            project: project.clone(),
            settings,
            empty: false,
            active: false,
            viewport: None,
        }))
        .unwrap();
        assert!(matches!(
            received.recv_blocking().unwrap(),
            Event::Ready { generation: 2, .. }
        ));
        assert_eq!(fs::metadata(&path).unwrap().ino(), inode);
        let untouched = root.join("untouched");
        tx.send(Command::Load {
            generation: 3,
            project: untouched.clone(),
            empty: true,
            active: false,
            viewport: None,
        })
        .unwrap();
        assert!(matches!(
            received.recv_blocking().unwrap(),
            Event::Ready {
                generation: 3,
                image: Some(_),
                ..
            }
        ));
        assert!(!super::settings_path(&data, &untouched).exists());
        tx.send(Command::Shutdown).unwrap();
        handle.join().unwrap();
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn unmodified_native_rasters_keep_their_existing_pixel_allocation() {
        let pixels = RgbaImage::from_pixel(512, 512, Rgba([10, 20, 30, 255]));
        let pointer = pixels.as_raw().as_ptr();
        let resized = super::resize_owned(pixels, 512);
        assert_eq!(resized.as_raw().as_ptr(), pointer);
        let image = super::render_image(resized, Effect::None, true);
        assert_eq!(image.as_bytes(0).unwrap().as_ptr(), pointer);
        assert_eq!(&image.as_bytes(0).unwrap()[..4], &[30, 20, 10, 255]);
        assert_eq!(image.as_bytes(0).unwrap().last(), Some(&0));
    }
    #[test]
    fn every_effect_fades_to_real_transparency_without_darkening_original_colors() {
        let source = RgbaImage::from_pixel(80, 101, Rgba([70, 140, 210, 123]));
        for effect in Effect::ALL {
            for empty in [true, false] {
                let image = super::render_image(source.clone(), effect, empty);
                let pixels = image.as_bytes(0).unwrap();
                let alpha = |y: usize| pixels[(y * 80 + 20) * 4 + 3];
                assert_eq!(
                    alpha(0),
                    123,
                    "top alpha must preserve original transparency"
                );
                assert_eq!(alpha(100), 0, "every effect clears the bottom");
                assert!(alpha(80) > 0, "the soft tail extends beyond the old cutoff");
                assert!(
                    (0..100).all(|y| alpha(y).saturating_add(1) >= alpha(y + 1)),
                    "coverage rounding must stay within one alpha level of the monotonic fade"
                );
                assert!(alpha(30) > 100);
                assert!((40..90).contains(&alpha(54)));
                if effect == Effect::None {
                    // Transparency blends with native glass rather than painting black.
                    assert_eq!(
                        &pixels[(54 * 80 + 20) * 4..(54 * 80 + 20) * 4 + 3],
                        &[210, 140, 70]
                    );
                }
            }
        }
    }
    #[test]
    fn portrait_cover_crop_keeps_fade_in_destination_coordinates() {
        let portrait = RgbaImage::from_fn(300, 900, |_, y| {
            if y < 300 {
                Rgba([255, 0, 0, 255])
            } else if y < 600 {
                Rgba([0, 255, 0, 255])
            } else {
                Rgba([0, 0, 255, 255])
            }
        });
        let cropped = super::cover_pixels(&portrait, Some((900, 300)), super::STATIC_SIDE);
        assert_eq!(cropped.dimensions(), (900, 300));
        assert_eq!(cropped.get_pixel(0, 0).0, [0, 255, 0, 255]);
        let image = super::render_image(cropped, Effect::None, true);
        let bytes = image.as_bytes(0).unwrap();
        assert!(bytes.chunks_exact(4).all(|pixel| pixel[..3] == [0, 255, 0]));
        assert!(
            bytes[(299 * 900 * 4)..]
                .chunks_exact(4)
                .all(|pixel| pixel[3] == 0)
        );
        let bounded = super::cover_pixels(&portrait, Some((100_000, 30_000)), super::STATIC_SIDE);
        assert!(bounded.width() <= super::STATIC_SIDE && bounded.height() <= super::STATIC_SIDE);
    }
    #[test]
    fn resizing_background_uses_cached_decode_and_streams_faded_gif_frames() {
        let mut decoded = Some(RgbaImage::from_pixel(80, 80, Rgba([1, 2, 3, 255])));
        let pointer = decoded.as_ref().unwrap().as_raw().as_ptr();
        // If resize tries to decode compressed bytes again this deliberately
        // invalid source fails. The existing decoded source remains immutable.
        let (image, animation) = super::prepare_cached(
            Arc::from(b"not-image".as_slice()),
            &Settings::default(),
            true,
            Some((300, 100)),
            &mut decoded,
        )
        .unwrap();
        assert!(animation.is_none());
        assert_eq!(image.size(0).width.0, 300);
        assert_eq!(decoded.as_ref().unwrap().as_raw().as_ptr(), pointer);
        assert!(
            decoded
                .as_ref()
                .unwrap()
                .pixels()
                .all(|pixel| pixel[3] == 255)
        );
        let (first, animation) = super::prepare_cached(
            gif(Some(Repeat::Finite(1))),
            &Settings::default(),
            false,
            Some((1500, 900)),
            &mut None,
        )
        .unwrap();
        let mut animation = animation.unwrap();
        let mut frames = vec![first];
        while let Some(frame) = next_frame(&mut animation, &Settings::default(), false).unwrap() {
            frames.push(frame);
            assert!(frames.len() <= 6);
        }
        assert_eq!(frames.len(), 6);
        for frame in frames {
            assert!(frame.size(0).width.0 <= super::ANIMATED_SIDE as i32);
            assert_eq!(frame.as_bytes(0).unwrap().last(), Some(&0));
            assert_eq!(frame.as_bytes(0).unwrap()[3], 255);
        }
    }
    #[test]
    fn wallpaper_extent_stays_clear_of_bottom_controls() {
        let bounds = gpui::Bounds::new(
            gpui::point(gpui::px(42.), gpui::px(20.)),
            gpui::size(gpui::px(1500.), gpui::px(1100.)),
        );
        assert_eq!(
            super::wallpaper_bounds(bounds, true).size.height,
            gpui::px(780.)
        );
        assert_eq!(
            super::wallpaper_bounds(bounds, false).size.height,
            gpui::px(700.)
        );
        assert_eq!(super::wallpaper_bounds(bounds, true).origin, bounds.origin);
    }
    #[test]
    fn gif_viewport_and_scope_changes_preserve_playhead_and_finished_loops() {
        let root = fixture();
        let data = root.join("managed");
        let original = root.join("finite.gif");
        fs::write(&original, gif(Some(Repeat::Finite(1)))).unwrap();
        let project = root.join("project");
        let (tx, rx) = mpsc::channel();
        let (events, received) = async_channel::bounded(1);
        let handle = std::thread::spawn(move || worker(data, rx, events));
        tx.send(Command::Import {
            config: Config {
                generation: 1,
                project: project.clone(),
                settings: Settings::default(),
                empty: true,
                active: false,
                viewport: None,
            },
            source: original,
        })
        .unwrap();
        let settings = match received.recv_blocking().unwrap() {
            Event::Ready { settings, .. } => settings,
            _ => panic!("expected imported GIF"),
        };
        tx.send(Command::Active(true)).unwrap();
        let green = match received.recv_blocking().unwrap() {
            Event::Frame(1, image) => image,
            _ => panic!("expected second GIF frame"),
        };
        assert_eq!(&green.as_bytes(0).unwrap()[..3], &[0, 255, 0]);
        tx.send(Command::Active(false)).unwrap();
        tx.send(Command::Configure(Config {
            generation: 2,
            project: project.clone(),
            settings: settings.clone(),
            empty: false,
            active: false,
            viewport: Some((500, 300)),
        }))
        .unwrap();
        let resized = match received.recv_blocking().unwrap() {
            Event::Ready {
                generation: 2,
                image: Some(image),
                ..
            } => image,
            _ => panic!("expected remasked current frame"),
        };
        assert_eq!(
            &resized.as_bytes(0).unwrap()[..3],
            &[0, 255, 0],
            "resizing must not restart at red"
        );
        tx.send(Command::Active(true)).unwrap();
        for expected in [[255, 0, 0], [0, 0, 255], [0, 255, 0], [255, 0, 0]] {
            let frame = match received.recv_blocking().unwrap() {
                Event::Frame(2, image) => image,
                _ => panic!("expected remaining GIF frame"),
            };
            assert_eq!(&frame.as_bytes(0).unwrap()[..3], &expected);
        }
        std::thread::sleep(Duration::from_millis(70));
        assert!(received.try_recv().is_err());
        tx.send(Command::Configure(Config {
            generation: 3,
            project,
            settings,
            empty: true,
            active: true,
            viewport: Some((400, 300)),
        }))
        .unwrap();
        let last = match received.recv_blocking().unwrap() {
            Event::Ready {
                generation: 3,
                image: Some(image),
                ..
            } => image,
            _ => panic!("expected remasked final frame"),
        };
        assert_eq!(&last.as_bytes(0).unwrap()[..3], &[255, 0, 0]);
        std::thread::sleep(Duration::from_millis(70));
        assert!(
            received.try_recv().is_err(),
            "a completed finite GIF must remain stopped"
        );
        tx.send(Command::Shutdown).unwrap();
        handle.join().unwrap();
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    #[ignore = "focused decode workload; run explicitly with --ignored --nocapture"]
    fn import_decode_workload() {
        use image::ImageEncoder;
        let source = RgbaImage::from_fn(1280, 720, |x, y| {
            Rgba([x as u8, y as u8, (x ^ y) as u8, 255])
        });
        let mut bytes = vec![];
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(source.as_raw(), 1280, 720, image::ExtendedColorType::Rgba8)
            .unwrap();
        let bytes: Arc<[u8]> = Arc::from(bytes);
        let settings = Settings::default();
        // Warm ImageIO so both paths measure decode/transform work, not setup.
        std::hint::black_box(prepare(bytes.clone(), &settings, true).unwrap());
        let before = std::time::Instant::now();
        for _ in 0..20 {
            std::hint::black_box(prepare(bytes.clone(), &settings, true).unwrap());
            std::hint::black_box(prepare(bytes.clone(), &settings, true).unwrap());
        }
        let before = before.elapsed();
        let after = std::time::Instant::now();
        for _ in 0..20 {
            std::hint::black_box(prepare(bytes.clone(), &settings, true).unwrap());
        }
        eprintln!(
            "debug warmed 20×1280×720 PNG imports, old two-preparation path: {before:?}; reused-preparation path: {:?}",
            after.elapsed()
        );
    }
    #[test]
    fn global_wallpaper_overrides_old_projects_and_preserves_shared_artwork() {
        let root = fixture();
        let one = root.join("one");
        let two = root.join("two");
        let source = root.join("shared.gif");
        fs::write(&source, gif(None)).unwrap();
        let local = Settings {
            path: Some(source.clone()),
            effect: Effect::Ascii,
            ..Settings::default()
        };
        save_settings(&root, &one, &local).unwrap();
        save_settings(
            &root,
            &two,
            &Settings {
                effect: Effect::Dither,
                ..local.clone()
            },
        )
        .unwrap();
        let global = Settings {
            everywhere: true,
            effect: Effect::Scanlines,
            session_opacity: 0.4,
            ..local.clone()
        };
        save_settings(&root, &one, &global).unwrap();
        for project in [&one, &two, &root.join("future-project")] {
            assert_eq!(read_settings(&root, project).unwrap(), global);
        }
        // Removing the global artwork cannot delete a source still referenced
        // by a project preference that will become visible when global is off.
        save_settings(
            &root,
            &one,
            &Settings {
                everywhere: true,
                ..Settings::default()
            },
        )
        .unwrap();
        super::remove_unused_source(&root, &source);
        assert!(source.exists());
        assert!(read_settings(&root, &two).unwrap().path.is_none());
        fs::remove_file(root.join("global.json")).unwrap();
        assert_eq!(read_settings(&root, &one).unwrap(), local);
        assert_eq!(read_settings(&root, &two).unwrap().effect, Effect::Dither);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn worker_applies_global_scope_across_chat_switches_and_can_return_to_local() {
        let root = fixture();
        let one = root.join("one");
        let two = root.join("two");
        let source = root.join("shared.gif");
        fs::write(&source, gif(None)).unwrap();
        let local = Settings {
            path: Some(source),
            ..Settings::default()
        };
        save_settings(
            &root,
            &two,
            &Settings {
                effect: Effect::Dither,
                ..local.clone()
            },
        )
        .unwrap();
        let (tx, rx) = mpsc::channel();
        let (events, received) = async_channel::bounded(1);
        let data = root.clone();
        let handle = std::thread::spawn(move || worker(data, rx, events));
        let config = |settings| Config {
            generation: 1,
            project: one.clone(),
            settings,
            empty: false,
            active: false,
            viewport: Some((32, 32)),
        };
        tx.send(Command::Scope(config(Settings {
            everywhere: true,
            ..local.clone()
        })))
        .unwrap();
        assert!(matches!(
            received.recv_blocking().unwrap(),
            Event::Ready {
                animated: true,
                settings: Settings {
                    everywhere: true,
                    ..
                },
                ..
            }
        ));
        tx.send(Command::Load {
            generation: 2,
            project: two.clone(),
            empty: false,
            active: false,
            viewport: Some((32, 32)),
        })
        .unwrap();
        assert!(matches!(
            received.recv_blocking().unwrap(),
            Event::Ready {
                generation: 2,
                settings: Settings {
                    everywhere: true,
                    effect: Effect::None,
                    ..
                },
                ..
            }
        ));
        tx.send(Command::Remove(config(Settings {
            everywhere: true,
            ..local.clone()
        })))
        .unwrap();
        assert!(matches!(
            received.recv_blocking().unwrap(),
            Event::Ready {
                image: None,
                settings: Settings {
                    everywhere: true,
                    ..
                },
                ..
            }
        ));
        assert!(read_settings(&root, &two).unwrap().path.is_none());
        tx.send(Command::Scope(config(local.clone()))).unwrap();
        assert!(matches!(
            received.recv_blocking().unwrap(),
            Event::Ready {
                settings: Settings {
                    everywhere: false,
                    ..
                },
                image: Some(_),
                ..
            }
        ));
        assert!(!root.join("global.json").exists());
        assert_eq!(read_settings(&root, &one).unwrap(), local);
        assert_eq!(read_settings(&root, &two).unwrap().effect, Effect::Dither);
        tx.send(Command::Shutdown).unwrap();
        handle.join().unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn bundled_wallpaper_is_shared_and_removal_stays_removed_after_reload() {
        let root = fixture();
        let data = root.join("managed");
        let one = root.join("one");
        let two = root.join("two");
        let default = read_settings(&data, &one).unwrap();
        let path = default.path.clone().unwrap();
        assert_eq!(fs::read(&path).unwrap(), super::DEFAULT_WALLPAPER);
        assert_eq!(read_settings(&data, &two).unwrap().path, Some(path.clone()));
        let (tx, rx) = mpsc::channel();
        let (events, received) = async_channel::bounded(1);
        let data_worker = data.clone();
        let handle = std::thread::spawn(move || worker(data_worker, rx, events));
        tx.send(Command::Remove(Config {
            generation: 1,
            project: one.clone(),
            settings: default,
            empty: true,
            active: false,
            viewport: Some((320, 180)),
        }))
        .unwrap();
        assert!(matches!(
            received.recv_blocking().unwrap(),
            Event::Ready { image: None, .. }
        ));
        assert!(read_settings(&data, &one).unwrap().path.is_none());
        assert!(
            path.exists(),
            "removing one project's wallpaper must retain the shared source"
        );
        assert_eq!(read_settings(&data, &two).unwrap().path, Some(path.clone()));
        tx.send(Command::Default(Config {
            generation: 2,
            project: one.clone(),
            settings: Settings::default(),
            empty: true,
            active: false,
            viewport: Some((320, 180)),
        }))
        .unwrap();
        match received.recv_blocking().unwrap() {
            Event::Ready {
                image: Some(_),
                animated: false,
                ..
            } => {}
            Event::Error(_, error) => panic!("Default wallpaper failed: {error}"),
            _ => panic!("Expected static default wallpaper"),
        }
        assert_eq!(read_settings(&data, &one).unwrap().path, Some(path));
        tx.send(Command::Shutdown).unwrap();
        handle.join().unwrap();
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn settings_are_project_scoped_atomic_and_cannot_import_external_paths() {
        let root = fixture();
        let one = root.join("one");
        let two = root.join("two");
        let settings = Settings {
            path: Some(root.join("image.png")),
            effect: Effect::Ascii,
            empty_only: true,
            everywhere: false,
            empty_opacity: 0.3,
            session_opacity: 0.1,
            ..Settings::default()
        };
        save_settings(&root, &one, &settings).unwrap();
        let loaded = read_settings(&root, &one).unwrap();
        assert_eq!(loaded.effect, Effect::Ascii);
        assert!(loaded.empty_only);
        assert_eq!(
            read_settings(&root, &two).unwrap().path,
            Some(root.join("bonsai-sakura.png"))
        );
        let settings = Settings {
            path: Some(PathBuf::from("/tmp/external-image.png")),
            ..settings
        };
        save_settings(&root, &one, &settings).unwrap();
        assert!(read_settings(&root, &one).unwrap().path.is_none());
        assert!(!fs::read_dir(&root).unwrap().any(|entry| {
            entry
                .unwrap()
                .path()
                .extension()
                .is_some_and(|ext| ext == "next")
        }));
        assert_eq!(visibility(f32::NAN), 0.24);
        assert_eq!(visibility(9.), 1.);
        assert_eq!(visibility(-1.), 0.);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn native_png_decode_preserves_color_orientation_and_transparency() {
        use image::ImageEncoder;
        let mut source = RgbaImage::from_pixel(8, 4, Rgba([0, 255, 0, 255]));
        for y in 0..2 {
            for x in 0..8 {
                source.put_pixel(x, y, Rgba([255, 0, 0, 128]));
            }
        }
        let mut bytes = vec![];
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(source.as_raw(), 8, 4, image::ExtendedColorType::Rgba8)
            .unwrap();
        let decoded = super::static_pixels(&bytes).unwrap();
        assert_eq!(decoded.dimensions(), (8, 4));
        assert!(decoded.get_pixel(0, 0)[0] > 250 && decoded.get_pixel(0, 0)[1] < 5);
        assert_eq!(decoded.get_pixel(0, 0)[3], 128);
        assert!(decoded.get_pixel(0, 3)[1] > 250);
    }
    #[test]
    fn svg_background_raster_is_bounded_and_actual_geometry_is_rendered() {
        let svg=br##"<svg xmlns="http://www.w3.org/2000/svg" width="4000" height="1000"><rect width="4000" height="1000" fill="#ff0000"/><rect x="2000" width="2000" height="1000" fill="#0000ff"/></svg>"##;
        let raster = super::static_pixels(svg).unwrap();
        assert_eq!(raster.dimensions(), (2048, 512));
        assert_eq!(raster.get_pixel(10, 10).0, [255, 0, 0, 255]);
        assert_eq!(raster.get_pixel(2000, 10).0, [0, 0, 255, 255]);
    }
    #[test]
    fn svg_renders_text_and_blocks_external_resources_and_compressed_inputs() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="180" height="50"><text x="5" y="35" font-family="sans-serif" font-size="30" fill="red">Orbit</text></svg>"##;
        let raster = super::static_pixels(svg).unwrap();
        assert!(
            raster
                .pixels()
                .filter(|pixel| pixel[3] > 0 && pixel[0] > 200)
                .count()
                > 100
        );
        let options = super::svg_options();
        assert!(
            (options.image_href_resolver.resolve_string)("/tmp/arbitrary.png", &options).is_none()
        );
        let embedded = RgbaImage::from_pixel(2048, 2048, image::Rgba([255, 0, 0, 255]));
        let mut png = std::io::Cursor::new(Vec::new());
        embedded
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let png = Arc::new(png.into_inner());
        for _ in 0..4 {
            assert!(
                (options.image_href_resolver.resolve_data)("image/png", png.clone(), &options)
                    .is_some()
            );
        }
        assert!((options.image_href_resolver.resolve_data)("image/png", png, &options).is_none());
        let external = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><image href="/System/Library/CoreServices/DefaultDesktop.heic" width="10" height="10"/></svg>"##;
        assert!(
            super::static_pixels(external)
                .unwrap()
                .pixels()
                .all(|pixel| pixel[3] == 0)
        );
        let mut oversized = svg.to_vec();
        oversized.resize(super::SVG_LIMIT + 1, b' ');
        assert!(
            super::static_pixels(&oversized)
                .unwrap_err()
                .to_string()
                .contains("4 MB")
        );
        assert!(super::static_pixels(&[0x1f, 0x8b, 0, 0]).is_err());
    }
    #[gpui::test]
    fn inactive_or_reduced_motion_window_keeps_native_background_artwork_visible(
        cx: &mut gpui::TestAppContext,
    ) {
        use gpui::IntoElement;
        let (tx, _rx) = mpsc::channel();
        let image = super::WallpaperFrame::fixture(vec![Frame::new(RgbaImage::from_pixel(
            8,
            8,
            Rgba([255, 0, 0, 255]),
        ))]);
        let (view, cx) = cx.add_window_view(move |_, _| super::ChatBackground {
            surfaces:(false,false),surface_bounds:Default::default(),
            project: PathBuf::from("/project"),
            generation: 1,
            settings: Settings {
                path: Some(PathBuf::from("/managed/image.png")),
                ..Settings::default()
            },
            image: Some(image),
            empty: true,
            active: false,
            native_active:false,
            busy: false,
            quitting: false,
            error: None,
            animated: false,
            tx,
            empty_bounds: std::rc::Rc::new(std::cell::Cell::new(gpui::Bounds::default())),
            session_bounds: std::rc::Rc::new(std::cell::Cell::new(gpui::Bounds::default())),
            viewport: None,
            prepared_viewport: None,
            measured_viewport: std::rc::Rc::new(std::cell::Cell::new(None)),
            resized_at: std::time::Instant::now(),
            resize_pending: false,
            opening_motion: true, presented_fade: Fade::Classic, fade_transition: None,
            wallpaper_reveal: Default::default(),
        });
        cx.simulate_resize(gpui::size(gpui::px(500.), gpui::px(350.)));
        cx.draw(
            gpui::point(gpui::px(0.), gpui::px(0.)),
            gpui::size(gpui::px(500.), gpui::px(350.)),
            |_, _| view.clone().into_any_element(),
        );
        let bounds = cx.debug_bounds("chat-background-artwork").unwrap();
        assert_eq!(bounds.size.width, gpui::px(500.));
        assert_eq!(bounds.size.height, gpui::px(350.));
        // This fixture injects an already decoded image without a worker.
        // Mark it prepared for the measured canvas before exercising controls.
        view.update(cx,|this,_|{this.prepared_viewport=this.viewport;this.busy=false;});
        // The same controls fit inside the anchored wallpaper dropdown without
        // clipping the preview, effects, scope, or either opacity track.
        struct SettingsPanel(gpui::Entity<super::ChatBackground>);
        impl gpui::Render for SettingsPanel {
            fn render(
                &mut self,
                _: &mut gpui::Window,
                cx: &mut gpui::Context<Self>,
            ) -> impl IntoElement {
                self.0.update(cx, |this, cx| this.settings_panel(cx))
            }
        }
        let settings =
            cx.update(|window, cx| window.replace_root(cx, |_, _| SettingsPanel(view.clone())));
        cx.simulate_resize(gpui::size(gpui::px(340.), gpui::px(450.)));
        cx.draw(
            gpui::point(gpui::px(0.), gpui::px(0.)),
            gpui::size(gpui::px(340.), gpui::px(450.)),
            |_, _| settings.clone().into_any_element(),
        );
        let panel = cx.debug_bounds("background-settings").unwrap();
        assert_eq!(panel.size.width, gpui::px(340.));
        // The popup scrolls when the expanded wallpaper controls exceed its viewport.
        assert!(panel.size.height <= gpui::px(500.));
        assert_eq!(
            cx.debug_bounds("background-preview").unwrap().size.height,
            gpui::px(90.)
        );
        for control in [
            "choose-background",
            "default-background",
            "remove-background",
            "background-empty-slider",
            "background-session-slider",
            "background-scope-0",
            "background-scope-1",
            "background-application-0",
            "background-application-1",
            "background-effect-0",
            "background-effect-1",
            "background-effect-2",
            "background-effect-3",
            "background-effect-4",
            "background-effect-5",
        ] {
            let bounds = cx.debug_bounds(control).unwrap();
            assert!(
                bounds.left() >= panel.left() && bounds.right() <= panel.right(),
                "{control} exceeds dropdown width"
            );
            assert!(
                bounds.top() >= panel.top() && bounds.bottom() <= panel.bottom(),
                "{control} exceeds dropdown height"
            );
        }
        let generation = cx.update(|_, cx| view.read(cx).generation);
        for (selector, empty, fraction) in [
            ("background-empty-slider", true, 0.9),
            ("background-session-slider", false, 0.1),
        ] {
            let bounds = cx.debug_bounds(selector).unwrap();
            cx.simulate_click(
                gpui::point(
                    bounds.left() + bounds.size.width * fraction,
                    bounds.center().y,
                ),
                gpui::Modifiers::none(),
            );
            view.update(cx, |this, _| {
                let value = if empty {
                    this.settings.empty_opacity
                } else {
                    this.settings.session_opacity
                };
                assert!((value - fraction).abs() < 0.011);
                assert_eq!(
                    this.generation, generation,
                    "opacity changes must reuse the prepared image"
                );
                assert!(!this.busy);
            });
        }
        view.update(cx, |this, _| {
            assert!(this.settings.empty_opacity > this.settings.session_opacity);
            this.settings.empty_only = true;
        });
        let all = cx.debug_bounds("background-application-1").unwrap();
        cx.simulate_click(all.center(), gpui::Modifiers::none());
        view.update(cx, |this, _| {
            assert!(this.settings.everywhere);
            assert!(!this.settings.empty_only);
            assert!(this.busy);
        });
        cx.update(|_, cx| {
            view.update(cx, |this, _| {
                assert!(this.visible());
                this.settings.empty_only = true;
                this.empty = false;
                assert!(!this.visible());
            })
        });
    }
    #[gpui::test]
    fn opening_never_publishes_an_unmeasured_or_stale_effect_bitmap(cx:&mut gpui::TestAppContext){
        use gpui::Bounds;
        use std::{cell::Cell,rc::Rc,time::Instant};
        let(tx,rx)=mpsc::channel();
        let(view,cx)=cx.add_window_view(move|_,_|super::ChatBackground{
            surfaces:(false,false),surface_bounds:Default::default(),
            project:PathBuf::from("/project"),generation:1,settings:Settings::default(),image:None,
            empty:true,active:true,native_active:false,busy:true,quitting:false,error:None,animated:false,tx,
            empty_bounds:Rc::new(Cell::new(Bounds::default())),session_bounds:Rc::new(Cell::new(Bounds::default())),
            viewport:Some((1400,800)),prepared_viewport:None,measured_viewport:Rc::new(Cell::new(None)),
            resized_at:Instant::now(),resize_pending:false,
            opening_motion: true, presented_fade: Fade::Classic, fade_transition: None,
            wallpaper_reveal: Default::default(),
        });
        let settings=Settings{path:Some(PathBuf::from("/managed/image.png")),..Settings::default()};
        let wrong=super::render_image(RgbaImage::from_pixel(32,64,Rgba([0,0,255,255])),Effect::Dither,true);
        let correct=super::render_image(RgbaImage::from_pixel(140,80,Rgba([255,0,0,255])),Effect::Dither,true);
        view.update(cx,|this,cx|{
            for _ in rx.try_iter(){}
            this.viewport=Some((1400,800));
            this.receive(super::Event::Ready{generation:1,settings:settings.clone(),image:Some(wrong.clone()),animated:false,empty:true,viewport:None},cx);
            assert!(this.image.is_none());assert!(this.native_surface().is_none());
            assert!(matches!(rx.try_recv().unwrap(),Command::Configure(super::Config{viewport:Some((1400,800)),..})));
            let generation=this.generation;
            this.receive(super::Event::Ready{generation,settings:settings.clone(),image:Some(correct.clone()),animated:false,empty:true,viewport:Some((1400,800))},cx);
            assert!(Arc::ptr_eq(this.image.as_ref().unwrap(),&correct));assert!(this.native_surface().is_some());
            this.receive(super::Event::Ready{generation,settings,image:Some(wrong),animated:false,empty:true,viewport:Some((800,500))},cx);
            assert!(Arc::ptr_eq(this.image.as_ref().unwrap(),&correct),"A late crop must not replace the fine-grained frame");
        });
    }
    #[gpui::test]
    fn tab_switch_keeps_wallpaper_until_current_project_is_ready(cx:&mut gpui::TestAppContext) {
        let (tx,rx)=mpsc::channel();
        let (view,cx)=cx.add_window_view(move|_,_|super::ChatBackground{
            surfaces:(false,false),surface_bounds:Default::default(),project:PathBuf::from("/a"),generation:1,
            settings:Settings::default(),image:None,empty:false,active:true,native_active:false,busy:false,quitting:false,error:None,animated:false,tx,
            empty_bounds:Default::default(),session_bounds:Default::default(),viewport:Some((8,8)),prepared_viewport:Some((8,8)),measured_viewport:Default::default(),
            resized_at:std::time::Instant::now(),resize_pending:false,opening_motion:true, presented_fade: Fade::Classic, fade_transition: None,wallpaper_reveal:Default::default(),
        });
        let original=super::WallpaperFrame::fixture(vec![Frame::new(RgbaImage::from_pixel(8,8,Rgba([80,120,180,255])))]);
        let replacement=super::WallpaperFrame::fixture(vec![Frame::new(RgbaImage::from_pixel(8,8,Rgba([180,120,80,255])))]);
        view.update(cx,|this,cx|{
            // This test drives worker replies directly, independently of the
            // test window's automatically measured initial viewport.
            this.viewport=Some((8,8));this.prepared_viewport=Some((8,8));
            this.settings=Settings{path:Some(PathBuf::from("/managed/wallpaper.png")),everywhere:true,effect:super::Effect::Dither,..Settings::default()};
            this.image=Some(original.clone());
            this.set_project(PathBuf::from("/b"),cx);
            assert!(this.visible());assert!(Arc::ptr_eq(this.image.as_ref().unwrap(),&original));
            assert_eq!(this.generation,1);assert!(!this.busy);assert!(rx.try_recv().is_err(),"Global wallpaper reloaded on tab switch");
            this.settings.everywhere=false;
            this.set_project(PathBuf::from("/c"),cx);
            let current=this.generation;
            assert!(matches!(rx.try_recv().unwrap(),Command::Load{generation,..} if generation==current));
            assert!(this.visible());assert!(Arc::ptr_eq(this.image.as_ref().unwrap(),&original),"Tab switch exposed a blank frame");
            this.receive(super::Event::Ready{generation:current-1,settings:Settings::default(),image:None,animated:false,empty:false,viewport:Some((8,8))},cx);
            assert!(Arc::ptr_eq(this.image.as_ref().unwrap(),&original),"Stale load cleared the wallpaper");
            this.receive(super::Event::Ready{generation:current,settings:this.settings.clone(),image:Some(replacement.clone()),animated:false,empty:false,viewport:Some((8,8))},cx);
            assert!(Arc::ptr_eq(this.image.as_ref().unwrap(),&replacement));
            this.set_project(PathBuf::from("/d"),cx);
            assert!(Arc::ptr_eq(this.image.as_ref().unwrap(),&replacement));
            this.receive(super::Event::Ready{generation:this.generation,settings:Settings::default(),image:None,animated:false,empty:false,viewport:Some((8,8))},cx);
            assert!(!this.visible());assert!(this.image.is_none(),"An explicitly empty destination retained the prior project's wallpaper");
        });
    }

    #[gpui::test]
    fn quitting_retains_wallpaper_and_ignores_late_replies_until_cancelled(
        cx: &mut gpui::TestAppContext,
    ) {
        use gpui::Bounds;
        use std::{cell::Cell, rc::Rc, time::Instant};
        let data = PathBuf::from("/managed");
        let (tx, rx) = mpsc::channel();
        let (view, cx) = cx.add_window_view(move |_, _| super::ChatBackground {
            surfaces:(false,false),surface_bounds:Default::default(),
            project: PathBuf::from("/project"),
            generation: 1,
            settings: Settings::default(),
            image: None,
            empty: true,
            active: true,
            native_active:false,
            busy: false,
            quitting: false,
            error: None,
            animated: false,
            tx,
            empty_bounds: Rc::new(Cell::new(Bounds::default())),
            session_bounds: Rc::new(Cell::new(Bounds::default())),
            viewport: None,
            prepared_viewport: None,
            measured_viewport: Rc::new(Cell::new(None)),
            resized_at: Instant::now(),
            resize_pending: false,
            opening_motion: true, presented_fade: Fade::Classic, fade_transition: None,
            wallpaper_reveal: Default::default(),
        });
        let image = super::WallpaperFrame::fixture(vec![Frame::new(
            RgbaImage::from_pixel(8, 8, Rgba([255, 0, 0, 255])),
        )]);
        view.update(cx, |this, cx| {
            this.settings = Settings {
                path: Some(data.join("backgrounds/image.png")),
                everywhere: true,
                ..Settings::default()
            };
            this.image = Some(image.clone());
            let project = this.project.clone();
            let settings = this.settings.clone();
            let viewport = this.viewport;
            this.set_quitting(true, cx);
            this.set_project(data.join("another-project"), cx);
            this.set_context(false, false, cx);
            this.set_viewport((120, 80), cx);
            this.prepare(cx);
            this.receive(
                super::Event::Ready {
                    generation: this.generation,
                    settings: Settings::default(),
                    image: None,
                    animated: false,
                    empty: false,
                    viewport: None,
                },
                cx,
            );
            this.receive(
                super::Event::Frame(
                    this.generation,
                    super::WallpaperFrame::fixture(vec![Frame::new(RgbaImage::new(
                        1, 1,
                    ))]),
                ),
                cx,
            );
            assert!(this.visible());
            assert!(Arc::ptr_eq(this.image.as_ref().unwrap(), &image));
            assert_eq!(this.project, project);
            assert_eq!(this.settings, settings);
            assert_eq!(this.viewport, viewport);
            assert!(this.empty);
            let generation = this.generation;
            this.set_quitting(false, cx);
            assert!(!this.quitting);
            assert!(this.busy);
            assert!(this.generation > generation);
            assert!(Arc::ptr_eq(this.image.as_ref().unwrap(), &image));
        });
        assert!(matches!(rx.try_recv().unwrap(), Command::Active(false)));
        assert!(matches!(rx.try_recv().unwrap(), Command::Load { .. }));
        assert!(rx.try_recv().is_err());
    }
    #[test]
    fn oversized_sources_reject_before_decoding_or_reading_all_data() {
        let root = fixture();
        let path = root.join("huge.png");
        let file = fs::File::create(&path).unwrap();
        file.set_len(SOURCE_LIMIT + 1).unwrap();
        assert!(super::read_source(&path).is_err());
        let mut gif = gif(None).to_vec();
        gif[6..8].copy_from_slice(&4096u16.to_le_bytes());
        assert!(frame_stream(Arc::from(gif)).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}

/// Bounded native decode shared with explicit composer image imports.
pub(crate) fn attachment_thumbnail(bytes: &[u8], side: u32) -> std::io::Result<RgbaImage> {
    thumbnail_pixels(bytes, side)
}

#[cfg(target_os = "macos")]
mod native_image {
    use std::{ffi::c_void, ptr};
    type Ref = *mut c_void;
    #[repr(C)]
    struct Point {
        x: f64,
        y: f64,
    }
    #[repr(C)]
    struct Size {
        width: f64,
        height: f64,
    }
    #[repr(C)]
    struct Rect {
        origin: Point,
        size: Size,
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        static kCFAllocatorNull: Ref;
        static kCFBooleanTrue: Ref;
        fn CFDataCreateWithBytesNoCopy(
            allocator: Ref,
            bytes: *const u8,
            len: isize,
            deallocator: Ref,
        ) -> Ref;
        fn CFNumberCreate(allocator: Ref, number_type: i32, value: *const c_void) -> Ref;
        fn CFDictionaryCreate(
            allocator: Ref,
            keys: *const Ref,
            values: *const Ref,
            count: isize,
            key_callbacks: Ref,
            value_callbacks: Ref,
        ) -> Ref;
        fn CFRelease(value: Ref);
    }
    #[link(name = "ImageIO", kind = "framework")]
    unsafe extern "C" {
        static kCGImageSourceCreateThumbnailFromImageAlways: Ref;
        static kCGImageSourceCreateThumbnailWithTransform: Ref;
        static kCGImageSourceThumbnailMaxPixelSize: Ref;
        static kCGImageSourceShouldCacheImmediately: Ref;
        fn CGImageSourceCreateWithData(data: Ref, options: Ref) -> Ref;
        fn CGImageSourceCreateThumbnailAtIndex(source: Ref, index: usize, options: Ref) -> Ref;
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGImageGetWidth(image: Ref) -> usize;
        fn CGImageGetHeight(image: Ref) -> usize;
        fn CGImageRelease(image: Ref);
        fn CGColorSpaceCreateDeviceRGB() -> Ref;
        fn CGColorSpaceRelease(space: Ref);
        fn CGBitmapContextCreate(
            data: Ref,
            width: usize,
            height: usize,
            bits: usize,
            row_bytes: usize,
            color_space: Ref,
            bitmap_info: u32,
        ) -> Ref;
        fn CGContextRelease(context: Ref);
        fn CGContextDrawImage(context: Ref, rect: Rect, image: Ref);
    }
    struct Owned {
        pointer: Ref,
        release: unsafe extern "C" fn(Ref),
    }
    impl Owned {
        fn new(pointer: Ref, release: unsafe extern "C" fn(Ref)) -> std::io::Result<Self> {
            if pointer.is_null() {
                Err(std::io::Error::other(
                    "macOS could not decode this background image",
                ))
            } else {
                Ok(Self { pointer, release })
            }
        }
    }
    impl Drop for Owned {
        fn drop(&mut self) {
            unsafe { (self.release)(self.pointer) }
        }
    }
    pub fn thumbnail(bytes: &[u8], side: u32) -> std::io::Result<image::RgbaImage> {
        // ImageIO requests a thumbnail directly, avoiding a full-resolution
        // decode for large photos. Borrowed bytes/option values outlive calls.
        unsafe {
            let data = Owned::new(
                CFDataCreateWithBytesNoCopy(
                    ptr::null_mut(),
                    bytes.as_ptr(),
                    bytes.len() as isize,
                    kCFAllocatorNull,
                ),
                CFRelease,
            )?;
            let source = Owned::new(
                CGImageSourceCreateWithData(data.pointer, ptr::null_mut()),
                CFRelease,
            )?;
            let side = side as i32;
            let number = Owned::new(
                CFNumberCreate(ptr::null_mut(), 3, &side as *const i32 as *const c_void),
                CFRelease,
            )?;
            let keys = [
                kCGImageSourceCreateThumbnailFromImageAlways,
                kCGImageSourceCreateThumbnailWithTransform,
                kCGImageSourceThumbnailMaxPixelSize,
                kCGImageSourceShouldCacheImmediately,
            ];
            let values = [
                kCFBooleanTrue,
                kCFBooleanTrue,
                number.pointer,
                kCFBooleanTrue,
            ];
            let options = Owned::new(
                CFDictionaryCreate(
                    ptr::null_mut(),
                    keys.as_ptr(),
                    values.as_ptr(),
                    keys.len() as isize,
                    ptr::null_mut(),
                    ptr::null_mut(),
                ),
                CFRelease,
            )?;
            let image = Owned::new(
                CGImageSourceCreateThumbnailAtIndex(source.pointer, 0, options.pointer),
                CGImageRelease,
            )?;
            let width = CGImageGetWidth(image.pointer);
            let height = CGImageGetHeight(image.pointer);
            if width == 0 || height == 0 || width > side as usize || height > side as usize {
                return Err(std::io::Error::other(
                    "Background thumbnail exceeds image limits",
                ));
            }
            let space = Owned::new(CGColorSpaceCreateDeviceRGB(), CGColorSpaceRelease)?;
            let mut pixels = vec![0u8; width * height * 4];
            let context = Owned::new(
                CGBitmapContextCreate(
                    pixels.as_mut_ptr() as Ref,
                    width,
                    height,
                    8,
                    width * 4,
                    space.pointer,
                    1 | 0x4000,
                ),
                CGContextRelease,
            )?;
            CGContextDrawImage(
                context.pointer,
                Rect {
                    origin: Point { x: 0., y: 0. },
                    size: Size {
                        width: width as f64,
                        height: height as f64,
                    },
                },
                image.pointer,
            );
            drop(context);
            super::unpremultiply(&mut pixels);
            image::RgbaImage::from_raw(width as u32, height as u32, pixels)
                .ok_or_else(|| std::io::Error::other("Unable to convert background pixels"))
        }
    }
}
#[cfg(not(target_os = "macos"))]
mod native_image {
    pub fn thumbnail(bytes: &[u8], side: u32) -> std::io::Result<image::RgbaImage> {
        let mut reader =
            image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format()?;
        reader.limits(super::limits());
        let pixels = reader.decode().map_err(std::io::Error::other)?.into_rgba8();
        Ok(super::resize(&pixels, side))
    }
}

#[cfg(test)]mod native_notch_tests{
 use super::*;use std::prelude::v1::test;
 #[test]fn native_wallpaper_retains_real_transparency_and_uses_the_same_effect(){
  let mut pixels=RgbaImage::from_pixel(16,16,image::Rgba([20,40,80,160]));pixels.put_pixel(0,0,image::Rgba([0,0,0,0]));
  for effect in Effect::ALL{
   let expected=if effect==Effect::None{pixels.clone()}else{effects(&pixels,effect,false)};
   let frame=render_image(pixels.clone(),effect,false);let native=frame.native.as_ref().unwrap();
   assert_eq!(native,&expected,"The notch must share the actual effect pixels before Code's downward fade");
   assert!(frame.as_bytes(0).unwrap().chunks_exact(4).skip(16*15).all(|p|p[3]==0));
  }
 }
 #[test]fn secondary_surface_is_bounded_without_changing_the_workspace_raster(){
  let frame=render_image(RgbaImage::from_pixel(1600,900,image::Rgba([30,60,90,255])),Effect::None,true);
  assert_eq!(frame.size(0).width.0,1600);let native=frame.native.as_ref().unwrap();assert_eq!(native.width(),512);assert!(native.height()<=512);
 }
}

#[cfg(test)]
mod scene_cache_tests {
    use super::*;
    use std::prelude::v1::test;

    #[gpui::test]
    fn cached_artwork_reuses_motion_frames_and_invalidates_for_changes_and_resize(cx: &mut TestAppContext) {
        let (app, cx) = crate::ui_tests::fixture(cx);
        let background = app.update(cx, |app, cx| {
            app.theme.wallpaper_side_surfaces = true;
            app.theme.wallpaper_top_bottom = true;
            app.background.update(cx, |art, cx| {
                art.opening_motion = false;
                art.settings.path = Some("/fixture/art.png".into());
                art.settings.everywhere = true;
                art.image = Some(WallpaperFrame::fixture(vec![image::Frame::new(
                    RgbaImage::from_pixel(8, 8, image::Rgba([80, 120, 180, 255])),
                )]));
                cx.notify();
            });
            app.background.clone()
        });
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear();
            BACKGROUND_RENDERS.with(|count| count.set(0));
            for _ in 0..3 {
                window.invalidate_root_for_paint();
                window.draw(cx).clear();
            }
            assert_eq!(BACKGROUND_RENDERS.with(Cell::get), 0,
                "Unrelated root motion rebuilt unchanged artwork");
            background.update(cx, |art, cx| { art.settings.empty_opacity = 0.5; cx.notify(); });
        });
        // Flush the notify effect before drawing, as the real event loop does.
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.draw(cx).clear();
            assert!(BACKGROUND_RENDERS.with(Cell::get) > 0, "Artwork setting change used a stale scene");
            BACKGROUND_RENDERS.with(|count| count.set(0));
        });
        cx.simulate_resize(size(px(900.), px(600.)));
        cx.update(|window, cx| {
            window.draw(cx).clear();
            assert!(BACKGROUND_RENDERS.with(Cell::get) > 0, "Resize used stale artwork geometry");
        });
    }
}

#[cfg(test)]
mod wallpaper_reveal_tests {
    use super::*;
    use std::prelude::v1::test;

    #[test]
    fn current_image_records_native_delay_and_waits_for_submission_anchor() {
        let mut state = WallpaperReveal::default();
        let now = Instant::now();
        let first = state.resolve(Some((100., true)), true, 100., now).unwrap();
        assert_eq!(first.not_before(), now + Duration::from_millis(80));
        assert_eq!(first.duration(), Duration::from_millis(600));
        let second = state
            .resolve(
                Some((100., true)),
                true,
                100.3,
                now + Duration::from_millis(300),
            )
            .unwrap();
        assert_eq!(second.not_before(), first.not_before());
        assert!(
            state
                .resolve(
                    Some((100., true)),
                    true,
                    100.7,
                    now + Duration::from_millis(700)
                )
                .is_some()
        );
        assert!(
            state
                .resolve(Some((100., true)), true, 101., now + Duration::from_secs(1))
                .is_some()
        );
    }
    #[test]
    fn late_current_image_receives_the_entire_reveal_after_ready() {
        let mut state = WallpaperReveal::default();
        let now = Instant::now();
        assert!(
            state
                .resolve(Some((100., true)), false, 103., now)
                .is_none()
        );
        let first = state
            .resolve(Some((100., true)), true, 104., now + Duration::from_secs(1))
            .unwrap();
        assert_eq!(first.not_before(), now + Duration::from_secs(1));
        assert!(
            state
                .resolve(
                    Some((100., true)),
                    true,
                    104.3,
                    now + Duration::from_millis(1300)
                )
                .is_some()
        );
    }
    #[test]
    fn hide_reopen_and_reduced_motion_retire_old_reveals() {
        let mut state = WallpaperReveal::default();
        let now = Instant::now();
        state.resolve(Some((100., true)), true, 100., now).unwrap();
        assert!(
            state
                .resolve(None, true, 100.1, now + Duration::from_millis(100))
                .is_none()
        );
        assert!(state.reveal.is_none());
        let reopened = state
            .resolve(Some((102., true)), true, 102., now + Duration::from_secs(2))
            .unwrap();
        assert_eq!(reopened.not_before(), now + Duration::from_millis(2080));
        assert!(
            state
                .resolve(
                    Some((102., false)),
                    true,
                    102.1,
                    now + Duration::from_millis(2100)
                )
                .is_none()
        );
        assert!(
            state
                .resolve(
                    Some((102., true)),
                    true,
                    102.2,
                    now + Duration::from_millis(2200)
                )
                .is_none()
        );
        assert!(
            state
                .resolve(Some((105., true)), true, 105., now + Duration::from_secs(5))
                .is_some()
        );
    }
    #[test]
    fn prepared_image_replacements_and_temporary_absence_preserve_one_epoch() {
        let mut state = WallpaperReveal::default();
        let now = Instant::now();
        let first = state.resolve(Some((100., true)), true, 100.2, now).unwrap();
        assert!(
            state
                .resolve(
                    Some((100., true)),
                    false,
                    100.3,
                    now + Duration::from_millis(100)
                )
                .is_none()
        );
        let replacement = state
            .resolve(
                Some((100., true)),
                true,
                100.4,
                now + Duration::from_millis(200),
            )
            .unwrap();
        assert_eq!(replacement.not_before(), first.not_before());
    }
    #[test]
    fn extreme_and_malformed_future_origins_have_a_bounded_not_before() {
        let now = Instant::now();
        for (origin, raw) in [
            (f64::MAX, 100.),
            (f64::INFINITY, 100.),
            (f64::NAN, 100.),
            (100., f64::NAN),
        ] {
            let mut state = WallpaperReveal::default();
            let reveal = state.resolve(Some((origin, true)), true, raw, now).unwrap();
            assert!(reveal.not_before() >= now);
            assert!(reveal.not_before() <= now + Duration::from_millis(80));
        }
    }
    #[test]
    fn legacy_dither_reference_is_monotonic_and_preserves_every_final_byte() {
        let source = RenderImage::new(vec![image::Frame::new(RgbaImage::from_pixel(
            8,
            6,
            image::Rgba([40, 120, 200, 180]),
        ))]);
        let original = source.as_bytes(0).unwrap();
        let mut previous = vec![0u8; 48];
        for step in 0..=14 {
            let frame = dither_reveal_frame(&source, step as f32 / 14.);
            for (index, pixel) in frame.as_bytes(0).unwrap().chunks_exact(4).enumerate() {
                assert_eq!(&pixel[..3], &original[index * 4..index * 4 + 3]);
                assert!(pixel[3] >= previous[index]);
                previous[index] = pixel[3];
            }
            if step == 14 {
                assert_eq!(frame.as_bytes(0), Some(original));
            }
        }
    }
}
