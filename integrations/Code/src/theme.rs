//! Persisted appearance preferences and cached native render tokens.
//! Resolve once when preferences or the system scheme change, never per frame.
use crate::appearance::Appearance;
use gpui::{Pixels, Rgba, SharedString, px};
use serde::{Deserialize, Serialize};
use std::{
    cell::{Cell, RefCell},
    hash::{Hash, Hasher},
    io::Read,
    path::Path,
};

pub const MAX_THEME_BYTES: usize = 64 * 1024;
pub const UI_FONT_DEFAULT: &str = ".AppleSystemUIFont";
pub const MONO_FONT_DEFAULT: &str = "Menlo";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scheme {
    #[default]
    System,
    Dark,
    Light,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Preset {
    #[default]
    Default,
    Harbor,
    Moss,
    Ember,
    Orchid,
    Tidepool,
    Dune,
    Rosewood,
    Glacier,
    Arcade,
    Ink,
}
impl Preset {
    pub const ALL: [Self; 11] = [
        Self::Default,
        Self::Harbor,
        Self::Moss,
        Self::Ember,
        Self::Orchid,
        Self::Tidepool,
        Self::Dune,
        Self::Rosewood,
        Self::Glacier,
        Self::Arcade,
        Self::Ink,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "Default",
            Self::Harbor => "Harbor",
            Self::Moss => "Moss",
            Self::Ember => "Ember",
            Self::Orchid => "Orchid",
            Self::Tidepool => "Tidepool",
            Self::Dune => "Dune",
            Self::Rosewood => "Rosewood",
            Self::Glacier => "Glacier",
            Self::Arcade => "Arcade",
            Self::Ink => "Ink",
        }
    }
    fn index(self) -> usize {
        Self::ALL.iter().position(|p| *p == self).unwrap_or(0)
    }
}
/// Native material preference. Automatic follows public API availability.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all="snake_case")]
pub enum GlassMaterial { #[default] Automatic, Classic, Liquid }
impl GlassMaterial { pub const ALL:[Self;3]=[Self::Automatic,Self::Classic,Self::Liquid];pub fn label(self)->&'static str{match self{Self::Automatic=>"Automatic",Self::Classic=>"Classic blur",Self::Liquid=>"Liquid Glass"}} }
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Contrast {
    #[default]
    Standard,
    High,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffPalette {
    #[default]
    Standard,
    Colorblind,
    HighContrast,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorSlot {
    Accent,
    Background,
    Foreground,
    Sidebar,
    Surface,
}
impl ColorSlot {
    pub const ALL: [Self; 5] = [
        Self::Accent,
        Self::Background,
        Self::Foreground,
        Self::Sidebar,
        Self::Surface,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Accent => "Accent",
            Self::Background => "Background",
            Self::Foreground => "Text",
            Self::Sidebar => "Sidebar",
            Self::Surface => "Cards and code",
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct ColorOverrides {
    pub accent: Option<u32>,
    pub background: Option<u32>,
    pub foreground: Option<u32>,
    pub sidebar: Option<u32>,
    pub surface: Option<u32>,
}
impl ColorOverrides {
    pub fn get(&self, slot: ColorSlot) -> Option<u32> {
        match slot {
            ColorSlot::Accent => self.accent,
            ColorSlot::Background => self.background,
            ColorSlot::Foreground => self.foreground,
            ColorSlot::Sidebar => self.sidebar,
            ColorSlot::Surface => self.surface,
        }
    }
    pub fn set(&mut self, slot: ColorSlot, color: Option<u32>) {
        let color = color.map(|v| v & 0xffffff);
        match slot {
            ColorSlot::Accent => self.accent = color,
            ColorSlot::Background => self.background = color,
            ColorSlot::Foreground => self.foreground = color,
            ColorSlot::Sidebar => self.sidebar = color,
            ColorSlot::Surface => self.surface = color,
        }
    }
}

/// Kept separate from legacy `Appearance` so old two-field literals and saved
/// workspaces stay compatible. All import/export and persistence use serde.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Theme {
    pub scheme: Scheme,
    pub preset: Preset,
    pub colors: ColorOverrides,
    pub ui_font: String,
    pub mono_font: String,
    pub ui_font_size: f32,
    pub mono_font_size: f32,
    pub sidebar_glass: bool,
    pub main_glass: bool,
    pub borderless_composer: bool,
    pub horizontal_agent_picker: bool,
    pub github_widget: bool,
    pub compact_diff: bool,
    pub adaptive_text:crate::adaptive_text::Settings,
    pub transparency: bool,
    pub glass_material: GlassMaterial,
    pub contrast: Contrast,
    pub diff_palette: DiffPalette,
}
impl Hash for Theme {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.scheme.hash(state);
        self.preset.hash(state);
        self.colors.hash(state);
        self.ui_font.hash(state);
        self.mono_font.hash(state);
        self.ui_font_size.to_bits().hash(state);
        self.mono_font_size.to_bits().hash(state);
        self.sidebar_glass.hash(state);
        self.main_glass.hash(state);
        self.borderless_composer.hash(state);
        self.horizontal_agent_picker.hash(state);
        self.github_widget.hash(state);
        self.compact_diff.hash(state);
        self.adaptive_text.enabled.hash(state);self.adaptive_text.settle_ms.hash(state);
        self.transparency.hash(state);
        self.glass_material.hash(state);
        self.contrast.hash(state);
        self.diff_palette.hash(state);
    }
}
impl Default for Theme {
    fn default() -> Self {
        Self {
            scheme: Scheme::System,
            preset: Preset::Default,
            colors: ColorOverrides::default(),
            ui_font: UI_FONT_DEFAULT.into(),
            mono_font: MONO_FONT_DEFAULT.into(),
            ui_font_size: 14.,
            mono_font_size: 12.,
            sidebar_glass: true,
            main_glass: true,
            borderless_composer: false,
            horizontal_agent_picker: true,
            github_widget: true,
            compact_diff: false,
            adaptive_text:Default::default(),
            transparency: true,
            glass_material:GlassMaterial::Automatic,
            contrast: Contrast::Standard,
            diff_palette: DiffPalette::Standard,
        }
    }
}
impl Theme {
    pub fn normalized(&self) -> Self {
        let mut next = self.clone();
        next.adaptive_text=next.adaptive_text.normalized();
        next.ui_font = normalize_font(&next.ui_font, UI_FONT_DEFAULT);
        next.mono_font = normalize_font(&next.mono_font, MONO_FONT_DEFAULT);
        next.ui_font_size = finite_size(next.ui_font_size, 14., 10., 24.);
        next.mono_font_size = finite_size(next.mono_font_size, 12., 9., 24.);
        for slot in ColorSlot::ALL {
            next.colors.set(slot, next.colors.get(slot));
        }
        next
    }
    pub fn is_dark(&self, system_dark: bool) -> bool {
        match self.scheme {
            Scheme::System => system_dark,
            Scheme::Dark => true,
            Scheme::Light => false,
        }
    }
    pub fn resolve(&self, system_dark: bool, appearance: Appearance) -> ResolvedTheme {
        let next = self.normalized();
        let dark = next.is_dark(system_dark);
        let palette = PALETTES[next.preset.index()][if dark { 0 } else { 1 }];
        let background = next.colors.background.unwrap_or(palette.panel_solid);
        let sidebar = next.colors.sidebar.unwrap_or(palette.surface);
        let surface = next.colors.surface.unwrap_or(palette.panel_solid);
        let mut foreground = next.colors.foreground.unwrap_or(palette.content);
        if next.contrast == Contrast::High {
            foreground = contrasting(foreground, background, 7.);
        }
        let mut accent = next.colors.accent.unwrap_or(palette.accent);
        // Keep arbitrary custom accents but ensure buttons and links can be read.
        accent = contrasting(
            accent,
            background,
            if next.contrast == Contrast::High {
                4.5
            } else {
                3.
            },
        );
        let on_accent = if contrast_ratio(accent, 0xffffff) > contrast_ratio(accent, 0) {
            0xffffff
        } else {
            0
        };
        let muted = contrasting(
            blend(foreground, background, 0.58),
            background,
            if next.contrast == Contrast::High {
                7.
            } else {
                4.5
            },
        );
        let border = blend(
            foreground,
            background,
            if next.contrast == Contrast::High {
                0.42
            } else {
                0.17
            },
        );
        let (add, remove) = match (next.diff_palette, dark) {
            (DiffPalette::Standard, true) => (0xaddbb6, 0xf0acac),
            (DiffPalette::Standard, false) => (0x176631, 0xa32430),
            (DiffPalette::Colorblind, true) => (0x7fc8ff, 0xffc183),
            (DiffPalette::Colorblind, false) => (0x075e9e, 0x985000),
            (DiffPalette::HighContrast, true) => (0x8dffa4, 0xffa6bc),
            (DiffPalette::HighContrast, false) => (0x005817, 0x990023),
        };
        let opacity = appearance.normalized().opacity;
        let native_transparency = next.transparency && (next.sidebar_glass || next.main_glass);
        let panel_alpha = if next.contrast == Contrast::High {
            opacity.max(0.85)
        } else {
            opacity
        };
        // MonoCode's transparent inset panes preserve one backdrop. Tint it
        // once natively; reserve local fill for chrome/cards rather than stacking
        // multiple 70% overlays across the reading surface.
        let high=next.contrast==Contrast::High;
        let native_alpha=if native_transparency {if high{0.80}else{0.16+0.30*panel_alpha}}else{0.};
        let residual=|target:f32|((target-native_alpha)/(1.-native_alpha)).clamp(0.,1.);
        let sidebar_rgba=with_alpha(sidebar,if native_transparency&&next.sidebar_glass{residual(if high{0.96}else{0.26+0.38*panel_alpha})}else{1.});
        let main_rgba=with_alpha(background,if native_transparency&&next.main_glass{residual(if high{0.96}else{0.20+0.34*panel_alpha})}else{1.});
        let surface_rgba=with_alpha(surface,if native_transparency&&next.main_glass{if high{0.94}else{0.16+0.30*panel_alpha}}else{1.});
        let menu_rgba = with_alpha(
            surface,
            if native_transparency {
                panel_alpha.max(0.88)
            } else {
                1.
            },
        );
        let hover_rgba = with_alpha(
            foreground,
            if next.contrast == Contrast::High {
                0.18
            } else {
                0.08
            },
        );
        ResolvedTheme {
            dark,
            background,
            sidebar,
            surface,
            foreground,
            muted,
            accent,
            on_accent,
            border,
            success: contrasting(if dark { 0x8fc89a } else { 0x23743a }, background, 3.),
            warning: contrasting(if dark { 0xddc9ac } else { 0x825500 }, background, 3.),
            error: contrasting(if dark { 0xe58c8c } else { 0xb12432 }, background, 3.),
            link: contrasting(palette.link, background, 4.5),
            heading: contrasting(palette.heading, background, 4.5),
            syntax_keyword: contrasting(palette.syntax[0], background, 4.5),
            syntax_callable: contrasting(palette.syntax[1], background, 4.5),
            syntax_string: contrasting(palette.syntax[2], background, 4.5),
            syntax_type: contrasting(palette.syntax[3], background, 4.5),
            syntax_tag: contrasting(palette.syntax[4], background, 4.5),
            syntax_number: contrasting(palette.syntax[5], background, 4.5),
            syntax_comment: contrasting(palette.syntax[6], background, 4.5),
            syntax_property: contrasting(palette.syntax[7], background, 4.5),
            syntax_meta: contrasting(palette.syntax[8], background, 4.5),
            syntax_invalid: contrasting(palette.syntax[9], background, 4.5),
            add,
            remove,
            add_bg: with_alpha(add, 0.12),
            remove_bg: with_alpha(remove, 0.12),
            ui_font: SharedString::from(next.ui_font),
            mono_font: SharedString::from(next.mono_font),
            ui_font_size: next.ui_font_size,
            mono_font_size: next.mono_font_size,
            native_transparency,
            glass_material:next.glass_material,
            sidebar_glass: native_transparency && next.sidebar_glass,
            main_glass: native_transparency && next.main_glass,
            backdrop_rgba:with_alpha(background,native_alpha),
            sidebar_rgba,
            main_rgba,
            surface_rgba,
            menu_rgba,
            hover_rgba,
        }
    }
    /// Bounded, versioned, transparent JSON. Unknown future fields are harmless;
    /// unknown enum values and unsupported versions produce an actionable error.
    pub fn from_json(input: &str) -> anyhow::Result<Self> {
        anyhow::ensure!(
            input.len() <= MAX_THEME_BYTES,
            "Theme exceeds the 64 KB limit"
        );
        let value: serde_json::Value = serde_json::from_str(input)?;
        anyhow::ensure!(value.is_object(), "A theme must be a JSON object");
        let data = if let Some(theme) = value.get("theme") {
            anyhow::ensure!(
                value.get("version").and_then(|v| v.as_u64()) == Some(1),
                "Unsupported theme version; expected version 1"
            );
            theme.clone()
        } else {
            value
        };
        anyhow::ensure!(data.is_object(), "Theme settings must be a JSON object");
        Ok(serde_json::from_value::<Self>(data)?.normalized())
    }
    pub fn from_json_with_appearance(input: &str) -> anyhow::Result<(Self, Option<Appearance>)> {
        let theme = Self::from_json(input)?;
        let value: serde_json::Value = serde_json::from_str(input)?;
        let glass = value
            .get("glass")
            .map(|value| serde_json::from_value::<Appearance>(value.clone()))
            .transpose()?
            .map(Appearance::normalized);
        Ok((theme, glass))
    }
    pub fn to_json_with_appearance(&self, appearance: Appearance) -> anyhow::Result<String> {
        Ok(serde_json::to_string_pretty(
            &serde_json::json!({"application":"Code","version":1,"theme":self.normalized(),"glass":appearance.normalized()}),
        )?)
    }
    pub fn import_file_with_appearance(path: &Path) -> anyhow::Result<(Self, Option<Appearance>)> {
        let file = std::fs::File::open(path)?;
        let mut input = String::new();
        file.take((MAX_THEME_BYTES + 1) as u64)
            .read_to_string(&mut input)?;
        Self::from_json_with_appearance(&input)
    }
    pub fn export_file_with_appearance(
        &self,
        path: &Path,
        appearance: Appearance,
    ) -> anyhow::Result<()> {
        std::fs::write(path, self.to_json_with_appearance(appearance)?)?;
        Ok(())
    }
}
fn finite_size(value: f32, default: f32, min: f32, max: f32) -> f32 {
    if value.is_finite() {
        (value.clamp(min, max) * 2.).round() / 2.
    } else {
        default
    }
}
fn normalize_font(value: &str, default: &str) -> String {
    let value = value.trim();
    if value.is_empty() || value.len() > 128 || value.chars().any(char::is_control) {
        default.into()
    } else {
        value.into()
    }
}
pub fn parse_hex(input: &str) -> anyhow::Result<u32> {
    let value = input.trim().strip_prefix('#').unwrap_or(input.trim());
    anyhow::ensure!(
        matches!(value.len(), 3 | 6)
            && value.is_ascii()
            && value.bytes().all(|v| v.is_ascii_hexdigit()),
        "Use a color such as #459BF7 or #49F"
    );
    let rgb = if value.len() == 3 {
        value.chars().flat_map(|c| [c, c]).collect::<String>()
    } else {
        value.into()
    };
    Ok(u32::from_str_radix(&rgb, 16)?)
}
pub fn format_hex(color: u32) -> String {
    format!("#{:06X}", color & 0xffffff)
}
/// Static swatches: no theme resolution, font allocations or contrast work in
/// preset-grid rendering.
pub fn preset_preview(preset: Preset, dark: bool) -> (u32, u32, u32) {
    let p = PALETTES[preset.index()][if dark { 0 } else { 1 }];
    (p.panel_solid, p.content, p.accent)
}
fn with_alpha(color: u32, opacity: f32) -> u32 {
    ((color & 0xffffff) << 8) | (opacity * 255.).round().clamp(0., 255.) as u32
}
fn blend(front: u32, back: u32, amount: f32) -> u32 {
    [16, 8, 0].into_iter().fold(0, |v, shift| {
        let a = ((front >> shift) & 255) as f32;
        let b = ((back >> shift) & 255) as f32;
        v | ((b + (a - b) * amount).round() as u32) << shift
    })
}
pub fn contrast_ratio(a: u32, b: u32) -> f32 {
    fn luminance(rgb: u32) -> f32 {
        [16, 8, 0]
            .into_iter()
            .zip([0.2126, 0.7152, 0.0722])
            .map(|(shift, weight)| {
                let c = ((rgb >> shift) & 255) as f32 / 255.;
                weight
                    * if c <= 0.04045 {
                        c / 12.92
                    } else {
                        ((c + 0.055) / 1.055).powf(2.4)
                    }
            })
            .sum()
    }
    let a = luminance(a);
    let b = luminance(b);
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}
fn contrasting(color: u32, background: u32, minimum: f32) -> u32 {
    if contrast_ratio(color, background) >= minimum {
        return color;
    }
    let target = if contrast_ratio(0, background) > contrast_ratio(0xffffff, background) {
        0
    } else {
        0xffffff
    };
    for step in 1..=100 {
        let candidate = blend(target, color, step as f32 / 100.);
        if contrast_ratio(candidate, background) >= minimum {
            return candidate;
        }
    }
    target
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedTheme {
    pub dark: bool,
    pub background: u32,
    pub sidebar: u32,
    pub surface: u32,
    pub foreground: u32,
    pub muted: u32,
    pub accent: u32,
    pub on_accent: u32,
    pub border: u32,
    pub success: u32,
    pub warning: u32,
    pub error: u32,
    pub link: u32,
    pub heading: u32,
    pub syntax_keyword: u32,
    pub syntax_callable: u32,
    pub syntax_string: u32,
    pub syntax_type: u32,
    pub syntax_tag: u32,
    pub syntax_number: u32,
    pub syntax_comment: u32,
    pub syntax_property: u32,
    pub syntax_meta: u32,
    pub syntax_invalid: u32,
    pub add: u32,
    pub remove: u32,
    pub add_bg: u32,
    pub remove_bg: u32,
    pub ui_font: SharedString,
    pub mono_font: SharedString,
    pub ui_font_size: f32,
    pub mono_font_size: f32,
    pub native_transparency: bool,
    pub glass_material:GlassMaterial,
    pub sidebar_glass: bool,
    pub main_glass: bool,
    backdrop_rgba:u32,
    sidebar_rgba: u32,
    main_rgba: u32,
    surface_rgba: u32,
    menu_rgba: u32,
    hover_rgba: u32,
}
impl ResolvedTheme {
    pub fn backdrop_rgba(&self)->u32{self.backdrop_rgba}
    pub fn sidebar_rgba(&self) -> u32 {
        self.sidebar_rgba
    }
    pub fn main_rgba(&self) -> u32 {
        self.main_rgba
    }
    pub fn surface_rgba(&self) -> u32 {
        self.surface_rgba
    }
    pub fn hover_rgba(&self) -> u32 {
        self.hover_rgba
    }
    pub fn menu_rgba(&self) -> u32 {
        self.menu_rgba
    }
    pub fn color(&self, slot: ColorSlot) -> u32 {
        match slot {
            ColorSlot::Accent => self.accent,
            ColorSlot::Background => self.background,
            ColorSlot::Foreground => self.foreground,
            ColorSlot::Sidebar => self.sidebar,
            ColorSlot::Surface => self.surface,
        }
    }
    /// Exact legacy semantic colors only. Provider identity and arbitrary media
    /// colors intentionally pass through; do not quantize/remap image pixels.
    pub fn map_rgb(&self, color: u32) -> u32 {
        match color {
            0xebebeb | 0xffffff | 0xd4d4d4 | 0xe5e7eb | 0xe8e8e8 | 0xeeeeee | 0xd6d9df
            | 0xd5d6d8 | 0xd6d6d6 => self.foreground,
            0x929292 | 0x999999 | 0x9a9a9a | 0xa9a9a9 | 0x888888 | 0x777777 | 0x707070
            | 0x666666 | 0x737b82 | 0xa7adb4 | 0xb5b5b5 => self.muted,
            0x459bf7 | 0x88bdf5 => self.accent,
            0x171717 | 0x161616 | 0x151515 | 0x141414 | 0x181818 | 0x1a1a1a | 0x202020
            | 0x202326 => self.background,
            0x242424 | 0x252525 | 0x262626 | 0x222222 | 0x282828 | 0x303030 => self.surface,
            0x8fc89a | 0x9fc6a8 => self.success,
            0xe58c8c | 0xee9494 | 0xf2a6a6 => self.error,
            0xddc9ac | 0xe5ac8c => self.warning,
            0xaddbb6 => self.add,
            0xf0acac => self.remove,
            0xbdabf3 => self.syntax_keyword,
            0xb5cba5 => self.syntax_string,
            0x747d8b => self.syntax_comment,
            0xc5c9e8 => self.foreground,
            0xe6b77e => self.syntax_number,
            0x7dd3fc => self.syntax_type,
            _ => color,
        }
    }
    pub fn map_rgba(&self, color: u32) -> u32 {
        match color {
            0x8fc89a16 => return self.add_bg,
            0xe58c8c16 => return self.remove_bg,
            0x242424bb => return self.menu_rgba(),
            0xebebeb0a => return self.surface_rgba(),
            _ => {}
        }
        (self.map_rgb(color >> 8) << 8) | (color & 255)
    }
}
thread_local! { static CURRENT:RefCell<ResolvedTheme>=RefCell::new(Theme::default().resolve(true,Appearance::default())); }
thread_local! { static MONO_METRICS:Cell<Option<(f32,f32)>>=const {Cell::new(None)}; }
pub fn install_resolved(theme: ResolvedTheme) {
    let font_changed = CURRENT.with(|current| {
        let mut current = current.borrow_mut();
        let changed =
            current.mono_font != theme.mono_font || current.mono_font_size != theme.mono_font_size;
        *current = theme;
        changed
    });
    if font_changed {
        MONO_METRICS.with(|metrics| metrics.set(None));
    }
}
/// Terminal cell metrics are measured once per font/theme change, with no
/// repeated system font enumeration or shaping in the terminal's paint loop.
pub fn ensure_font_metrics(window: &gpui::Window) {
    MONO_METRICS.with(|cache| {
        if cache.get().is_none() {
            let face = gpui::font(mono_font());
            let font_id = window.text_system().resolve_font(&face);
            let width = window
                .text_system()
                .advance(font_id, mono_px(12.), 'M')
                .ok()
                .map(|advance| f32::from(advance.width))
                .filter(|width| width.is_finite() && *width > 0.)
                .unwrap_or_else(|| f32::from(mono_px(7.23)));
            let height = f32::from((window.text_system().ascent(font_id, mono_px(12.))
                + window.text_system().descent(font_id, mono_px(12.)).abs()).ceil()).max(f32::from(mono_px(14.)));
            cache.set(Some((width,height)));
        }
    });
}
pub fn mono_cell_width() -> f32 {
    MONO_METRICS.with(|metrics| metrics.get().map(|metrics|metrics.0))
        .unwrap_or_else(|| f32::from(mono_px(7.23)))
}
pub fn mono_cell_height() -> f32 {
    MONO_METRICS.with(|metrics|metrics.get().map(|metrics|metrics.1))
        .unwrap_or_else(|| f32::from(mono_px(14.)))
}
pub fn rgb(color: u32) -> Rgba {
    CURRENT.with(|t| gpui::rgb(t.borrow().map_rgb(color)))
}
pub fn rgba(color: u32) -> Rgba {
    CURRENT.with(|t| gpui::rgba(t.borrow().map_rgba(color)))
}
/// Text/icons on accent-filled controls bypass legacy semantic color mapping.
pub fn on_accent() -> Rgba {
    CURRENT.with(|theme| gpui::rgb(theme.borrow().on_accent))
}
/// Explicit identity-color bypass for provider logos, file/media marks and art.
pub fn brand_rgb(color: u32) -> Rgba {
    gpui::rgb(color)
}

pub fn ui_font() -> SharedString {
    CURRENT.with(|t| t.borrow().ui_font.clone())
}
pub fn mono_font() -> SharedString {
    CURRENT.with(|t| t.borrow().mono_font.clone())
}
pub fn ui_px(base: f32) -> Pixels {
    CURRENT.with(|t| px(base * t.borrow().ui_font_size / 14.))
}
pub fn mono_px(base: f32) -> Pixels {
    CURRENT.with(|t| px(base * t.borrow().mono_font_size / 12.))
}
pub fn ui_line_height(base: f32) -> Pixels {
    ui_px(base)
}
pub fn mono_line_height(base: f32) -> Pixels {
    mono_px(base)
}

#[derive(Clone, Copy)]
struct Palette {
    surface: u32,
    panel_solid: u32,
    content: u32,
    accent: u32,
    link: u32,
    heading: u32,
    syntax: [u32; 10],
}
include!("theme_palettes.rs");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn old_settings_and_partial_theme_get_defaults() {
        let theme: Theme = serde_json::from_str("{\"scheme\":\"light\"}").unwrap();
        assert_eq!(theme.scheme, Scheme::Light);
        assert_eq!(theme.ui_font, UI_FONT_DEFAULT);
        assert!(theme.main_glass);
        assert_eq!(
            serde_json::from_str::<Appearance>("{}").unwrap(),
            Appearance::default()
        );
    }
    #[test]
    fn invalid_values_are_bounded_without_corrupting_unicode_fonts() {
        let mut theme = Theme {
            ui_font: "\n".into(),
            mono_font: "日本語 Mono".into(),
            ui_font_size: f32::NAN,
            mono_font_size: 999.,
            ..Theme::default()
        };
        theme.colors.accent = Some(u32::MAX);
        let theme = theme.normalized();
        assert_eq!(theme.ui_font, UI_FONT_DEFAULT);
        assert_eq!(theme.mono_font, "日本語 Mono");
        assert_eq!(theme.ui_font_size, 14.);
        assert_eq!(theme.mono_font_size, 24.);
        assert_eq!(theme.colors.accent, Some(0xffffff));
    }
    #[test]
    fn paired_palettes_and_system_scheme_resolve() {
        for preset in Preset::ALL {
            let theme = Theme {
                preset,
                ..Theme::default()
            };
            let dark = theme.resolve(true, Appearance::default());
            let light = theme.resolve(false, Appearance::default());
            assert!(dark.dark);
            assert!(!light.dark);
            assert_ne!(dark.background, light.background);
            assert!(contrast_ratio(dark.foreground, dark.background) > 4.5);
            assert!(contrast_ratio(light.foreground, light.background) > 4.5);
            assert!(contrast_ratio(dark.on_accent, dark.accent) >= 4.5);
            assert!(contrast_ratio(light.on_accent, light.accent) >= 4.5);
            assert!(contrast_ratio(light.map_rgb(0xc5c9e8), light.background) >= 4.5);
            assert_ne!(dark.map_rgb(0xebebeb), light.map_rgb(0xebebeb));
        }
    }
    #[test]
    fn opaque_and_independent_glass_have_real_native_flags() {
        let theme = Theme {
            main_glass: false,
            ..Theme::default()
        };
        let t = theme.resolve(true, Appearance::default());
        assert!(t.native_transparency && t.sidebar_glass);
        assert!(!t.main_glass);
        assert_eq!(t.main_rgba() & 255, 255);
        assert!(t.sidebar_rgba() & 255 < 255);
        let theme = Theme {
            transparency: false,
            ..theme
        };
        let t = theme.resolve(true, Appearance::default());
        assert!(!t.native_transparency);
        for color in [
            t.main_rgba(),
            t.sidebar_rgba(),
            t.surface_rgba(),
            t.menu_rgba(),
        ] {
            assert_eq!(color & 255, 255);
        }
    }
    #[test]
    fn native_tint_and_reading_surfaces_compose_independently(){
        for opacity in [0.,0.3,0.6,1.]{let theme=Theme::default().resolve(true,Appearance{opacity,blur_radius:24});let base=(theme.backdrop_rgba()&255)as f32/255.;let composed=|rgba:u32|base+(1.-base)*(rgba&255)as f32/255.;assert!((composed(theme.main_rgba())-(0.20+0.34*opacity)).abs()<0.008);assert!((composed(theme.sidebar_rgba())-(0.26+0.38*opacity)).abs()<0.008);assert!((theme.surface_rgba()&255)>=(theme.sidebar_rgba()&255));}
        let theme=Theme{transparency:false,..Theme::default()}.resolve(true,Appearance::default());assert_eq!(theme.backdrop_rgba()&255,0);assert_eq!(theme.main_rgba()&255,255);
    }
    #[test]
    fn identity_colors_do_not_change_and_alpha_is_preserved() {
        let t = Theme {
            scheme: Scheme::Light,
            preset: Preset::Moss,
            ..Theme::default()
        }
        .resolve(true, Appearance::default());
        for color in [0xd7ac91, 0xa39cfa, 0xa7b8de, 0x123456] {
            assert_eq!(t.map_rgb(color), color);
            assert_eq!(t.map_rgba((color << 8) | 0x77), (color << 8) | 0x77);
        }
        assert_eq!(t.map_rgba(0xebebeb14) & 255, 0x14);
        assert_eq!(t.map_rgba(0xebebeb14) >> 8, t.foreground);
    }
    #[test]
    fn high_contrast_handles_conflicting_custom_colors_and_diff_palettes() {
        let theme = Theme {
            contrast: Contrast::High,
            colors: ColorOverrides {
                foreground: Some(0x333333),
                background: Some(0x333333),
                ..Default::default()
            },
            ..Default::default()
        };
        let resolved = theme.resolve(true, Appearance::default());
        assert!(contrast_ratio(resolved.foreground, resolved.background) >= 7.);
        assert!(contrast_ratio(resolved.muted, resolved.background) >= 7.);
        let mut theme = theme;
        theme.diff_palette = DiffPalette::Colorblind;
        let alternate = theme.resolve(true, Appearance::default());
        assert_ne!(alternate.add, resolved.add);
        assert_ne!(alternate.remove, resolved.remove);
    }
    #[test]
    fn import_export_roundtrip_is_versioned_bounded_and_validated() {
        let original = Theme {
            scheme: Scheme::Light,
            preset: Preset::Orchid,
            ui_font: "Helvetica Neue".into(),
            ..Theme::default()
        };
        let json = original
            .to_json_with_appearance(Appearance::default())
            .unwrap();
        assert_eq!(Theme::from_json(&json).unwrap(), original);
        assert!(Theme::from_json("{\"version\":2,\"theme\":{}}").is_err());
        assert!(Theme::from_json("[]").is_err());
        assert!(Theme::from_json("{\"scheme\":\"nope\"}").is_err());
        assert!(Theme::from_json(&" ".repeat(MAX_THEME_BYTES + 1)).is_err());
        assert_eq!(Theme::from_json("{}").unwrap(), Theme::default());
        let glass = Appearance {
            blur_radius: 42,
            opacity: 0.38,
        };
        let complete = original.to_json_with_appearance(glass).unwrap();
        assert_eq!(
            Theme::from_json_with_appearance(&complete).unwrap(),
            (original, Some(glass))
        );
        assert_eq!(Theme::from_json_with_appearance("{}").unwrap().1, None);
    }
    #[test]
    fn colors_accept_only_rgb_hex() {
        assert_eq!(parse_hex(" #49f ").unwrap(), 0x4499ff);
        assert_eq!(parse_hex("123ABC").unwrap(), 0x123abc);
        for invalid in ["rgba(0,0,0,1)", "#12345678", "#gggggg", "１２３", "", "red"] {
            assert!(parse_hex(invalid).is_err());
        }
        assert_eq!(format_hex(0xabcdef), "#ABCDEF");
    }
}
