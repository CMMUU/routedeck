use chrono::{DateTime, Utc};
use serde::Serialize;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};
use sysinfo::Networks;
use tauri::{AppHandle, Emitter};

#[cfg(target_os = "macos")]
mod macos_title;

#[cfg(target_os = "macos")]
use std::sync::OnceLock;

pub const TRAFFIC_EVENT: &str = "global-traffic";
pub const TRAY_ID: &str = "main";

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GlobalTrafficSnapshot {
    pub enabled: bool,
    pub upload_bytes_per_second: u64,
    pub download_bytes_per_second: u64,
    pub sampled_at: Option<DateTime<Utc>>,
    pub interfaces: Vec<String>,
}

pub struct GlobalTrafficMonitor {
    enabled: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
    started: AtomicBool,
    snapshot: Arc<Mutex<GlobalTrafficSnapshot>>,
}

impl Default for GlobalTrafficMonitor {
    fn default() -> Self {
        Self {
            enabled: Arc::new(AtomicBool::new(true)),
            shutdown: Arc::new(AtomicBool::new(false)),
            started: AtomicBool::new(false),
            snapshot: Arc::new(Mutex::new(GlobalTrafficSnapshot {
                enabled: true,
                ..Default::default()
            })),
        }
    }
}

impl GlobalTrafficMonitor {
    pub fn start(&self, app: AppHandle, enabled: bool) {
        self.enabled.store(enabled, Ordering::Release);
        self.shutdown.store(false, Ordering::Release);
        if self.started.swap(true, Ordering::AcqRel) {
            return;
        }
        let enabled_flag = Arc::clone(&self.enabled);
        let shutdown = Arc::clone(&self.shutdown);
        let snapshot = Arc::clone(&self.snapshot);
        tauri::async_runtime::spawn(async move {
            run_monitor(app, enabled_flag, shutdown, snapshot).await;
        });
    }

    pub fn set_enabled(&self, app: &AppHandle, enabled: bool) {
        self.enabled.store(enabled, Ordering::Release);
        let next = GlobalTrafficSnapshot {
            enabled,
            sampled_at: Some(Utc::now()),
            ..Default::default()
        };
        store_and_publish(app, &self.snapshot, next.clone());
        update_tray(app, &next);
    }

    pub fn snapshot(&self) -> GlobalTrafficSnapshot {
        self.snapshot
            .lock()
            .map(|snapshot| snapshot.clone())
            .unwrap_or_default()
    }

    pub fn stop(&self) {
        self.shutdown.store(true, Ordering::Release);
    }
}

async fn run_monitor(
    app: AppHandle,
    enabled: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
    snapshot: Arc<Mutex<GlobalTrafficSnapshot>>,
) {
    let mut networks = Networks::new_with_refreshed_list();
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut collecting = false;
    let mut last_sample_at = Instant::now();
    let mut last_tray_key = String::new();

    loop {
        interval.tick().await;
        if shutdown.load(Ordering::Acquire) {
            break;
        }
        if !enabled.load(Ordering::Acquire) {
            collecting = false;
            continue;
        }
        if !collecting {
            networks.refresh(true);
            collecting = true;
            last_sample_at = Instant::now();
            let initial = GlobalTrafficSnapshot {
                enabled: true,
                sampled_at: Some(Utc::now()),
                ..Default::default()
            };
            store_and_publish(&app, &snapshot, initial.clone());
            update_tray_if_changed(&app, &initial, &mut last_tray_key);
            continue;
        }

        networks.refresh(true);
        let elapsed = last_sample_at.elapsed().as_secs_f64().max(0.001);
        last_sample_at = Instant::now();
        let (uploaded, downloaded, interfaces) = aggregate_network_deltas(&networks);
        if !enabled.load(Ordering::Acquire) {
            collecting = false;
            continue;
        }
        let next = GlobalTrafficSnapshot {
            enabled: true,
            upload_bytes_per_second: rate_per_second(uploaded, elapsed),
            download_bytes_per_second: rate_per_second(downloaded, elapsed),
            sampled_at: Some(Utc::now()),
            interfaces,
        };
        store_and_publish(&app, &snapshot, next.clone());
        update_tray_if_changed(&app, &next, &mut last_tray_key);
    }
}

fn aggregate_network_deltas(networks: &Networks) -> (u64, u64, Vec<String>) {
    let mut uploaded = 0_u64;
    let mut downloaded = 0_u64;
    let mut interfaces = Vec::new();
    for (name, network) in networks.iter() {
        if !should_include_interface(name) {
            continue;
        }
        uploaded = uploaded.saturating_add(network.transmitted());
        downloaded = downloaded.saturating_add(network.received());
        if network.total_transmitted() > 0 || network.total_received() > 0 {
            interfaces.push(name.clone());
        }
    }
    interfaces.sort();
    (uploaded, downloaded, interfaces)
}

fn rate_per_second(bytes: u64, elapsed_seconds: f64) -> u64 {
    ((bytes as f64) / elapsed_seconds)
        .round()
        .clamp(0.0, u64::MAX as f64) as u64
}

fn should_include_interface(name: &str) -> bool {
    let name = name.trim().to_ascii_lowercase();
    if name.is_empty() || matches!(name.as_str(), "lo" | "lo0" | "loopback") {
        return false;
    }
    let excluded_prefixes = [
        "utun",
        "tun",
        "tap",
        "tailscale",
        "wg",
        "wireguard",
        "docker",
        "veth",
        "virbr",
        "bridge",
        "br-",
        "awdl",
        "llw",
        "anpi",
        "gif",
        "stf",
        "vmnet",
        "vboxnet",
        "zt",
        "zerotier",
        "ham",
        "nordlynx",
    ];
    !excluded_prefixes
        .iter()
        .any(|prefix| name.starts_with(prefix))
        && !name.contains("loopback")
        && !name.contains("virtual")
        && !name.contains(" vpn")
}

fn store_and_publish(
    app: &AppHandle,
    state: &Arc<Mutex<GlobalTrafficSnapshot>>,
    next: GlobalTrafficSnapshot,
) {
    if let Ok(mut snapshot) = state.lock() {
        *snapshot = next.clone();
    }
    let _ = app.emit(TRAFFIC_EVENT, next);
}

fn update_tray_if_changed(
    app: &AppHandle,
    snapshot: &GlobalTrafficSnapshot,
    last_key: &mut String,
) {
    #[cfg(target_os = "macos")]
    let key = format!(
        "{}:{}",
        snapshot.enabled,
        macos_title::display_key(
            snapshot.upload_bytes_per_second,
            snapshot.download_bytes_per_second
        )
    );
    #[cfg(not(target_os = "macos"))]
    let key = format!(
        "{}:{}:{}",
        snapshot.enabled,
        format_tray_rate(snapshot.upload_bytes_per_second),
        format_tray_rate(snapshot.download_bytes_per_second)
    );
    if key == *last_key {
        return;
    }
    *last_key = key;
    update_tray(app, snapshot);
}

fn update_tray(app: &AppHandle, snapshot: &GlobalTrafficSnapshot) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        eprintln!("global traffic tray icon not found");
        return;
    };
    if !snapshot.enabled {
        // tray-icon 0.24's macOS None setter leaves the previous native title
        // intact. An empty string explicitly removes both rows.
        let _ = tray.set_title(Some(""));
        #[cfg(target_os = "macos")]
        if let Err(error) = macos_title::clear(&tray) {
            eprintln!("global traffic native tray reset failed: {error}");
        }
        if let Some(icon) = app.default_window_icon() {
            let _ = tray.set_icon_with_as_template(Some(icon.clone()), false);
        }
        let _ = tray.set_tooltip(Some("Serylane"));
        return;
    }

    let upload = format_tray_rate(snapshot.upload_bytes_per_second);
    let download = format_tray_rate(snapshot.download_bytes_per_second);
    let tooltip = format!("Serylane\n↑ {upload}\n↓ {download}");

    #[cfg(target_os = "macos")]
    {
        // Only the shared S mark remains an image. Numbers use AppKit text at
        // its real point size, independently of tray-icon's 18pt image scaling.
        let native = tray
            .set_icon_with_as_template(Some(native_macos_brand_icon()), true)
            .map_err(|error| error.to_string())
            .and_then(|()| {
                macos_title::update(
                    &tray,
                    snapshot.upload_bytes_per_second,
                    snapshot.download_bytes_per_second,
                )
            });
        match native {
            Ok(()) => return,
            Err(error) => eprintln!("global traffic native tray fallback: {error}"),
        }
        let _ = tray.set_title(Some(""));
        let _ = macos_title::clear(&tray);
        let (icon, is_template) = render_macos_tray_icon(&upload, &download);
        if let Err(error) = tray.set_icon_with_as_template(Some(icon), is_template) {
            eprintln!("global traffic tray icon update failed: {error}");
        }
    }

    #[cfg(target_os = "linux")]
    {
        let _ = tray.set_title(Some(format!("↑ {upload}\n↓ {download}")));
    }
    let _ = tray.set_tooltip(Some(tooltip));
}

#[cfg(target_os = "macos")]
fn native_macos_brand_icon() -> tauri::image::Image<'static> {
    static ICON: OnceLock<tauri::image::Image<'static>> = OnceLock::new();
    ICON.get_or_init(|| {
        // 18pt image at 2x, with the original S occupying 16pt.
        let mut rgba = vec![0_u8; 36 * 36 * 4];
        draw_brand_mark(&mut rgba, 36, 36, 2, 2, 32);
        tauri::image::Image::new_owned(rgba, 36, 36)
    })
    .clone()
}

fn format_tray_rate(bytes_per_second: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    let bytes = bytes_per_second as f64;
    let (value, unit) = if bytes >= GIB {
        (bytes / GIB, "G/s")
    } else if bytes >= MIB {
        (bytes / MIB, "M/s")
    } else if bytes >= KIB {
        (bytes / KIB, "K/s")
    } else {
        (bytes, "B/s")
    };
    if value < 10.0 && unit != "B/s" {
        format!("{value:.1}{unit}")
    } else {
        format!("{:.0}{unit}", value.min(999.0))
    }
}

#[cfg(target_os = "macos")]
fn render_macos_tray_icon(upload: &str, download: &str) -> (tauri::image::Image<'static>, bool) {
    if let Some(font) = macos_status_font() {
        return (
            render_monochrome_macos_tray_icon(font, upload, download),
            true,
        );
    }
    (render_pixel_macos_tray_icon(upload, download), true)
}

#[cfg(target_os = "macos")]
static MACOS_STATUS_FONT: OnceLock<Option<fontdue::Font>> = OnceLock::new();

#[cfg(target_os = "macos")]
fn macos_status_font() -> Option<&'static fontdue::Font> {
    MACOS_STATUS_FONT
        .get_or_init(|| {
            [
                "/System/Library/Fonts/SFNS.ttf",
                "/System/Library/Fonts/SFNSMono.ttf",
                "/System/Library/Fonts/Supplemental/Arial Bold.ttf",
            ]
            .iter()
            .find_map(|path| {
                std::fs::read(path).ok().and_then(|bytes| {
                    fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default()).ok()
                })
            })
        })
        .as_ref()
}

#[cfg(target_os = "macos")]
fn render_monochrome_macos_tray_icon(
    font: &fontdue::Font,
    upload: &str,
    download: &str,
) -> tauri::image::Image<'static> {
    const HEIGHT: u32 = 64;
    const FONT_SIZE: f32 = 27.0;
    const TEXT_X: u32 = 94;
    let text_width = smooth_text_width(font, upload, FONT_SIZE)
        .max(smooth_text_width(font, download, FONT_SIZE));
    let width = (TEXT_X as f32 + text_width + 8.0)
        .ceil()
        .clamp(184.0, 244.0) as u32;
    let mut rgba = vec![0_u8; (width * HEIGHT * 4) as usize];
    let monochrome = [255, 255, 255, 255];
    draw_brand_mark(&mut rgba, width, HEIGHT, 0, 4, 56);
    draw_smooth_arrow(&mut rgba, width, HEIGHT, 74.0, 8.0, true, monochrome);
    draw_smooth_arrow(&mut rgba, width, HEIGHT, 74.0, 36.0, false, monochrome);
    draw_smooth_text(
        &mut rgba, width, HEIGHT, font, upload, TEXT_X, 27, FONT_SIZE, monochrome,
    );
    draw_smooth_text(
        &mut rgba, width, HEIGHT, font, download, TEXT_X, 57, FONT_SIZE, monochrome,
    );
    tauri::image::Image::new_owned(rgba, width, HEIGHT)
}

#[cfg(target_os = "macos")]
fn smooth_text_width(font: &fontdue::Font, text: &str, size: f32) -> f32 {
    let mut width = 0.0_f32;
    let mut previous = None;
    for character in text.chars() {
        if let Some(previous) = previous {
            width += font
                .horizontal_kern(previous, character, size)
                .unwrap_or(0.0);
        }
        width += font.metrics(character, size).advance_width;
        previous = Some(character);
    }
    width
}

#[cfg(target_os = "macos")]
#[allow(clippy::too_many_arguments)]
fn draw_smooth_text(
    rgba: &mut [u8],
    width: u32,
    height: u32,
    font: &fontdue::Font,
    text: &str,
    x: u32,
    baseline: i32,
    size: f32,
    color: [u8; 4],
) {
    let mut cursor = x as f32;
    let mut previous = None;
    for character in text.chars() {
        if let Some(previous) = previous {
            cursor += font
                .horizontal_kern(previous, character, size)
                .unwrap_or(0.0);
        }
        let (metrics, bitmap) = font.rasterize(character, size);
        let glyph_x = cursor.round() as i32 + metrics.xmin;
        let glyph_y = baseline - metrics.ymin - metrics.height as i32;
        for row in 0..metrics.height {
            for column in 0..metrics.width {
                let coverage = bitmap[row * metrics.width + column];
                if coverage == 0 {
                    continue;
                }
                blend_pixel(
                    rgba,
                    width,
                    height,
                    glyph_x + column as i32,
                    glyph_y + row as i32,
                    color,
                    coverage,
                );
                blend_pixel(
                    rgba,
                    width,
                    height,
                    glyph_x + column as i32 + 1,
                    glyph_y + row as i32,
                    color,
                    coverage,
                );
            }
        }
        cursor += metrics.advance_width;
        previous = Some(character);
    }
}

#[cfg(target_os = "macos")]
static TRAY_BRAND_ICON: OnceLock<tauri::image::Image<'static>> = OnceLock::new();

#[cfg(target_os = "macos")]
fn tray_brand_icon() -> &'static tauri::image::Image<'static> {
    // Compile the same PNG used by the app/sidebar; no runtime file lookup or
    // independent handwritten brand glyph can drift from the application icon.
    TRAY_BRAND_ICON.get_or_init(|| tauri::include_image!("icons/128x128.png"))
}

#[cfg(target_os = "macos")]
fn draw_brand_mark(rgba: &mut [u8], width: u32, height: u32, x: u32, y: u32, size: u32) {
    if size == 0 {
        return;
    }
    let icon = tray_brand_icon();
    let source = icon.rgba();
    for dy in 0..size.min(height.saturating_sub(y)) {
        let sy0 = dy * icon.height() / size;
        let sy1 = ((dy + 1) * icon.height() / size)
            .max(sy0 + 1)
            .min(icon.height());
        for dx in 0..size.min(width.saturating_sub(x)) {
            let sx0 = dx * icon.width() / size;
            let sx1 = ((dx + 1) * icon.width() / size)
                .max(sx0 + 1)
                .min(icon.width());
            let mut alpha = 0_u32;
            for sy in sy0..sy1 {
                for sx in sx0..sx1 {
                    alpha += u32::from(source[((sy * icon.width() + sx) * 4 + 3) as usize]);
                }
            }
            let coverage = (alpha / ((sy1 - sy0) * (sx1 - sx0))) as u8;
            // macOS applies the correct menu-bar color to this template mask.
            if coverage > 0 {
                blend_pixel(
                    rgba,
                    width,
                    height,
                    (x + dx) as i32,
                    (y + dy) as i32,
                    [255, 255, 255, 255],
                    coverage,
                );
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn draw_smooth_arrow(
    rgba: &mut [u8],
    width: u32,
    height: u32,
    center_x: f32,
    y: f32,
    up: bool,
    color: [u8; 4],
) {
    if up {
        draw_line_segment(
            rgba,
            width,
            height,
            center_x,
            y + 2.0,
            center_x,
            y + 21.0,
            4.0,
            color,
        );
        draw_line_segment(
            rgba,
            width,
            height,
            center_x,
            y + 2.0,
            center_x - 8.0,
            y + 10.0,
            4.0,
            color,
        );
        draw_line_segment(
            rgba,
            width,
            height,
            center_x,
            y + 2.0,
            center_x + 8.0,
            y + 10.0,
            4.0,
            color,
        );
    } else {
        draw_line_segment(
            rgba,
            width,
            height,
            center_x,
            y + 1.0,
            center_x,
            y + 20.0,
            4.0,
            color,
        );
        draw_line_segment(
            rgba,
            width,
            height,
            center_x,
            y + 20.0,
            center_x - 8.0,
            y + 12.0,
            4.0,
            color,
        );
        draw_line_segment(
            rgba,
            width,
            height,
            center_x,
            y + 20.0,
            center_x + 8.0,
            y + 12.0,
            4.0,
            color,
        );
    }
}

#[cfg(target_os = "macos")]
#[allow(clippy::too_many_arguments)]
fn draw_line_segment(
    rgba: &mut [u8],
    width: u32,
    height: u32,
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
    thickness: f32,
    color: [u8; 4],
) {
    let min_x = (x1.min(x2) - thickness).floor().max(0.0) as i32;
    let max_x = (x1.max(x2) + thickness).ceil().min(width as f32 - 1.0) as i32;
    let min_y = (y1.min(y2) - thickness).floor().max(0.0) as i32;
    let max_y = (y1.max(y2) + thickness).ceil().min(height as f32 - 1.0) as i32;
    let dx = x2 - x1;
    let dy = y2 - y1;
    let length_squared = (dx * dx + dy * dy).max(f32::EPSILON);
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let t = (((px - x1) * dx + (py - y1) * dy) / length_squared).clamp(0.0, 1.0);
            let nearest_x = x1 + t * dx;
            let nearest_y = y1 + t * dy;
            let distance = ((px - nearest_x).powi(2) + (py - nearest_y).powi(2)).sqrt();
            let coverage = ((thickness * 0.5 + 0.75 - distance) * 255.0).clamp(0.0, 255.0) as u8;
            if coverage > 0 {
                blend_pixel(rgba, width, height, x, y, color, coverage);
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn blend_pixel(
    rgba: &mut [u8],
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    color: [u8; 4],
    coverage: u8,
) {
    if x < 0 || y < 0 || x >= width as i32 || y >= height as i32 {
        return;
    }
    let index = ((y as u32 * width + x as u32) * 4) as usize;
    let alpha = ((coverage as u16 * color[3] as u16) / 255) as u8;
    if alpha >= rgba[index + 3] {
        rgba[index] = color[0];
        rgba[index + 1] = color[1];
        rgba[index + 2] = color[2];
        rgba[index + 3] = alpha;
    }
}

#[cfg(target_os = "macos")]
fn render_pixel_macos_tray_icon(upload: &str, download: &str) -> tauri::image::Image<'static> {
    const HEIGHT: u32 = 32;
    const ARROW_X: u32 = 24;
    const TEXT_X: u32 = 36;
    let text_width = pixel_text_width(upload, 2).max(pixel_text_width(download, 2));
    let width = (TEXT_X + text_width + 3).clamp(90, 119);
    let mut rgba = vec![0_u8; (width * HEIGHT * 4) as usize];
    draw_brand_mark(&mut rgba, width, HEIGHT, 0, 4, 24);
    draw_arrow(&mut rgba, width, HEIGHT, ARROW_X, 1, true);
    draw_arrow(&mut rgba, width, HEIGHT, ARROW_X, 17, false);
    draw_text(&mut rgba, width, HEIGHT, TEXT_X, 0, upload, 2, 235);
    draw_text(&mut rgba, width, HEIGHT, TEXT_X, 16, download, 2, 235);
    tauri::image::Image::new_owned(rgba, width, HEIGHT)
}

#[cfg(target_os = "macos")]
fn pixel_text_width(text: &str, scale: u32) -> u32 {
    let glyphs = text.chars().count() as u32;
    glyphs.saturating_mul(6 * scale).saturating_sub(scale)
}

#[cfg(target_os = "macos")]
#[allow(clippy::too_many_arguments)]
fn draw_text(
    rgba: &mut [u8],
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    text: &str,
    scale: u32,
    alpha: u8,
) {
    let mut cursor = x;
    for character in text.chars() {
        if let Some(rows) = glyph(character) {
            for (row, bits) in rows.into_iter().enumerate() {
                for column in 0..5_u32 {
                    if bits & (1 << (4 - column)) == 0 {
                        continue;
                    }
                    draw_rect(
                        rgba,
                        width,
                        height,
                        cursor + column * scale,
                        y + row as u32 * scale,
                        scale,
                        scale,
                        alpha,
                    );
                }
            }
        }
        cursor = cursor.saturating_add(6 * scale);
    }
}

#[cfg(target_os = "macos")]
fn draw_arrow(rgba: &mut [u8], width: u32, height: u32, x: u32, y: u32, up: bool) {
    let center = x + 4;
    if up {
        draw_rect(rgba, width, height, center, y + 3, 2, 11, 255);
        for offset in 0..4_u32 {
            draw_rect(
                rgba,
                width,
                height,
                center - offset,
                y + 3 + offset,
                2,
                2,
                255,
            );
            draw_rect(
                rgba,
                width,
                height,
                center + offset,
                y + 3 + offset,
                2,
                2,
                255,
            );
        }
    } else {
        draw_rect(rgba, width, height, center, y, 2, 11, 255);
        for offset in 0..4_u32 {
            draw_rect(
                rgba,
                width,
                height,
                center - offset,
                y + 9 - offset,
                2,
                2,
                255,
            );
            draw_rect(
                rgba,
                width,
                height,
                center + offset,
                y + 9 - offset,
                2,
                2,
                255,
            );
        }
    }
}

#[cfg(target_os = "macos")]
#[allow(clippy::too_many_arguments)]
fn draw_rect(
    rgba: &mut [u8],
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    rect_width: u32,
    rect_height: u32,
    alpha: u8,
) {
    for py in y..y.saturating_add(rect_height).min(height) {
        for px in x..x.saturating_add(rect_width).min(width) {
            let index = ((py * width + px) * 4) as usize;
            rgba[index] = 255;
            rgba[index + 1] = 255;
            rgba[index + 2] = 255;
            rgba[index + 3] = alpha;
        }
    }
}

#[cfg(target_os = "macos")]
fn glyph(character: char) -> Option<[u8; 7]> {
    Some(match character {
        '0' => [14, 17, 19, 21, 25, 17, 14],
        '1' => [4, 12, 4, 4, 4, 4, 14],
        '2' => [14, 17, 1, 2, 4, 8, 31],
        '3' => [30, 1, 1, 14, 1, 1, 30],
        '4' => [2, 6, 10, 18, 31, 2, 2],
        '5' => [31, 16, 16, 30, 1, 1, 30],
        '6' => [14, 16, 16, 30, 17, 17, 14],
        '7' => [31, 1, 2, 4, 8, 8, 8],
        '8' => [14, 17, 17, 14, 17, 17, 14],
        '9' => [14, 17, 17, 15, 1, 1, 14],
        '.' => [0, 0, 0, 0, 0, 12, 12],
        '/' => [1, 2, 2, 4, 8, 8, 16],
        'B' => [30, 17, 17, 30, 17, 17, 30],
        'G' => [14, 17, 16, 23, 17, 17, 15],
        'K' => [17, 18, 20, 24, 20, 18, 17],
        'M' => [17, 27, 21, 21, 17, 17, 17],
        's' => [0, 0, 15, 16, 14, 1, 30],
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::{format_tray_rate, rate_per_second, should_include_interface};

    #[cfg(target_os = "macos")]
    #[test]
    fn tray_mark_is_the_application_icon_alpha_not_an_independent_glyph() {
        let icon = super::tray_brand_icon();
        assert_eq!((icon.width(), icon.height()), (128, 128));
        let mut rgba = vec![0_u8; 128 * 128 * 4];
        super::draw_brand_mark(&mut rgba, 128, 128, 0, 0, 128);
        let mut visible = 0;
        for (rendered, source) in rgba
            .as_chunks::<4>()
            .0
            .iter()
            .zip(icon.rgba().as_chunks::<4>().0)
        {
            assert_eq!(rendered[3], source[3]);
            if source[3] > 0 {
                visible += 1;
                assert_eq!(&rendered[..3], &[255, 255, 255]);
            }
        }
        assert!(visible > 1000 && visible < 128 * 128);
        let before = rgba.clone();
        super::draw_brand_mark(&mut rgba, 128, 128, 0, 0, 0);
        assert_eq!(rgba, before);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_and_fallback_trays_use_the_shared_s_mark() {
        let native = super::native_macos_brand_icon();
        let fallback = super::render_pixel_macos_tray_icon("1.2M/s", "8.3K/s");
        let font = super::macos_status_font().expect("macOS system status font");
        let normal = super::render_monochrome_macos_tray_icon(font, "1.2M/s", "8.3K/s");
        for (image, size, left, top) in [
            (&native, 32, 2, 2),
            (&normal, 56, 0, 4),
            (&fallback, 24, 0, 4),
        ] {
            let mut expected = vec![0_u8; (image.width() * image.height() * 4) as usize];
            super::draw_brand_mark(
                &mut expected,
                image.width(),
                image.height(),
                left,
                top,
                size,
            );
            for y in top..top + size {
                for x in left..left + size {
                    let alpha = ((y * image.width() + x) * 4 + 3) as usize;
                    assert_eq!(image.rgba()[alpha], expected[alpha]);
                }
            }
        }
        // Optional local visual evidence, never used by production or required
        // in CI. The bytes contain only the public icon and synthetic rates.
        if let Some(folder) = std::env::var_os("SERYLANE_TRAY_SNAPSHOT_DIR") {
            let folder = std::path::PathBuf::from(folder);
            std::fs::create_dir_all(&folder).unwrap();
            for (name, image) in [
                ("native-mark", native),
                ("normal", normal),
                ("fallback", fallback),
            ] {
                std::fs::write(folder.join(format!("{name}.rgba")), image.rgba()).unwrap();
                std::fs::write(
                    folder.join(format!("{name}.json")),
                    format!(
                        "{{\"width\":{},\"height\":{}}}",
                        image.width(),
                        image.height()
                    ),
                )
                .unwrap();
            }
        }
    }

    #[test]
    fn filters_virtual_interfaces_without_excluding_physical_names() {
        for name in ["en0", "en7", "eth0", "wlan0", "Ethernet", "Wi-Fi"] {
            assert!(should_include_interface(name), "{name}");
        }
        for name in [
            "lo0",
            "utun8",
            "tailscale0",
            "docker0",
            "veth1234",
            "bridge0",
            "awdl0",
            "VMware Virtual Ethernet Adapter",
        ] {
            assert!(!should_include_interface(name), "{name}");
        }
    }

    #[test]
    fn formats_compact_tray_rates() {
        assert_eq!(format_tray_rate(0), "0B/s");
        assert_eq!(format_tray_rate(1_536), "1.5K/s");
        assert_eq!(format_tray_rate(1_258_291), "1.2M/s");
        assert_eq!(format_tray_rate(12 * 1024 * 1024), "12M/s");
    }

    #[test]
    fn normalizes_delta_by_elapsed_time() {
        assert_eq!(rate_per_second(2_000, 2.0), 1_000);
    }
}
