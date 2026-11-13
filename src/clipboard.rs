//! 剪贴板记录：最近复制的文字，在置物架页的「剪贴板」里查看、再次复制、固定。
//!
//! - 只记文字；图片和文件不记录。
//! - 最近记录与固定条目都保存在本机 `clipboard.json`。
//! - 密码管理器等标记为「不进剪贴板历史」的内容一律跳过
//!   （`ExcludeClipboardContentFromMonitorProcessing` / `CanIncludeInClipboardHistory = 0`）。
//! - Windows 上用 `AddClipboardFormatListener` 等系统通知，没有轮询；
//!   监听线程只在剪贴板真正变化时醒来。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// 未固定条目的上限；固定的不占这个名额。
pub const MAX_RECENT: usize = 30;
/// 单条最长字符数，超过的不记录（大段日志、整份文件内容）。
pub const MAX_CHARS: usize = 20_000;
pub const MAX_PINNED: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ClipKind {
    #[default]
    Text,
    Image,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Clip {
    /// 文本内容；图片条目为空字符串（缩略由 `image` 表示）。
    pub text: String,
    /// 本地时间 `HH:MM`。
    pub at: String,
    pub pinned: bool,
    pub kind: ClipKind,
    /// 用户备注（可为空）。
    pub note: String,
    /// 图片条目：`%APPDATA%\Daozai\clipboard-blobs\<hash>.png` 的文件名（不含目录）。
    pub image: Option<String>,
}

impl Clip {
    /// 搜索匹配：文字 / 备注 / 图片标记。
    pub fn matches(&self, query: &str) -> bool {
        if query.trim().is_empty() {
            return true;
        }
        let q = query.to_lowercase();
        match self.kind {
            ClipKind::Text => self.text.to_lowercase().contains(&q) || self.note.to_lowercase().contains(&q),
            ClipKind::Image => self.note.to_lowercase().contains(&q) || q == "图片" || "image".contains(&q),
        }
    }
}

/// 图片 blob 目录。
pub fn blobs_dir() -> std::path::PathBuf {
    crate::model::config_dir().join("clipboard-blobs")
}

#[derive(Debug)]
pub struct History {
    pub items: Vec<Clip>,
    /// 每次变化 +1，界面据此判断是否需要重绘。
    pub version: u64,
    /// 未固定条目上限（可在设置中配置）。
    pub max_recent: usize,
    path: Option<std::path::PathBuf>,
}

impl Default for History {
    fn default() -> Self {
        Self { items: Vec::new(), version: 0, max_recent: MAX_RECENT, path: None }
    }
}

impl History {
    /// 新文字放最前；同样的文字只留一条（移到最前、保留固定状态）。
    pub fn push(&mut self, text: &str, at: &str) -> bool {
        if text.trim().is_empty() || text.chars().count() > MAX_CHARS { return false; }
        let pinned = match self.items.iter().position(|c| c.kind == ClipKind::Text && c.text == text) {
            Some(0) => return false,
            Some(i) => self.items.remove(i).pinned,
            None => false,
        };
        self.items.insert(0, Clip { text: text.to_string(), at: at.to_string(), pinned, kind: ClipKind::Text, note: String::new(), image: None });
        self.trim();
        self.changed();
        true
    }

    /// 新图片放最前（按 blob 文件名去重，同图移到最前）。
    pub fn push_image(&mut self, blob: &str, at: &str) -> bool {
        if blob.is_empty() { return false; }
        let pinned = match self.items.iter().position(|c| c.kind == ClipKind::Image && c.image.as_deref() == Some(blob)) {
            Some(0) => return false,
            Some(i) => self.items.remove(i).pinned,
            None => false,
        };
        self.items.insert(0, Clip { text: String::new(), at: at.to_string(), pinned, kind: ClipKind::Image, note: String::new(), image: Some(blob.to_string()) });
        self.trim();
        self.changed();
        true
    }

    fn trim(&mut self) {
        let cap = self.max_recent.max(1);
        let mut recent = 0;
        self.items.retain(|c| c.pinned || { recent += 1; recent <= cap });
    }

    pub fn remove(&mut self, index: usize) {
        if index < self.items.len() {
            let item = self.items.remove(index);
            // 删除图片 blob。
            if let Some(blob) = item.image {
                let _ = std::fs::remove_file(blobs_dir().join(blob));
            }
            self.changed();
        }
    }

    /// 清空未固定的条目，固定的保留。
    pub fn clear_recent(&mut self) {
        let mut removed = Vec::new();
        self.items.retain(|c| {
            if c.pinned { true } else { if let Some(b) = &c.image { removed.push(b.clone()); } false }
        });
        for blob in removed { let _ = std::fs::remove_file(blobs_dir().join(blob)); }
        self.changed();
    }

    pub fn toggle_pin(&mut self, index: usize) -> bool {
        let pinned_count = self.items.iter().filter(|c| c.pinned).count();
        let Some(item) = self.items.get_mut(index) else { return false; };
        if !item.pinned && pinned_count >= MAX_PINNED { return false; }
        item.pinned = !item.pinned;
        self.changed();
        true
    }

    /// 设置条目备注。
    pub fn set_note(&mut self, index: usize, note: &str) {
        if let Some(item) = self.items.get_mut(index) {
            item.note = note.trim().to_string();
            self.changed();
        }
    }

    /// 按 query 过滤出条目下标（保持顺序）。
    pub fn search_indices(&self, query: &str) -> Vec<usize> {
        self.items.iter().enumerate().filter(|(_, c)| c.matches(query)).map(|(i, _)| i).collect()
    }

    fn to_json(&self) -> Vec<u8> {
        let rows: Vec<_> = self.items.iter().map(|c| serde_json::json!({
            "pinned": c.pinned,
            "text": c.text,
            "at": c.at,
            "kind": if c.kind == ClipKind::Image { "image" } else { "text" },
            "note": c.note,
            "image": c.image,
        })).collect();
        serde_json::to_vec(&rows).unwrap_or_default()
    }

    pub fn from_json(raw: &[u8]) -> Self {
        let rows: Vec<serde_json::Value> = serde_json::from_slice(raw).unwrap_or_default();
        let items = rows.iter().filter_map(|row| {
            let kind = match row.get("kind").and_then(|v| v.as_str()) {
                Some("image") => ClipKind::Image,
                _ => ClipKind::Text,
            };
            let text = row.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let image = row.get("image").and_then(|v| v.as_str()).map(str::to_string);
            // 文本条目必须有文字；图片条目必须有 blob。
            if kind == ClipKind::Text && text.is_empty() { return None; }
            if kind == ClipKind::Image && image.is_none() { return None; }
            Some(Clip {
                text,
                at: row.get("at").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                pinned: row.get("pinned").and_then(|v| v.as_bool()).unwrap_or(true),
                kind,
                note: row.get("note").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                image,
            })
        }).take(MAX_PINNED + MAX_RECENT).collect();
        Self { items, ..Self::default() }
    }

    fn path() -> std::path::PathBuf { crate::model::config_dir().join("clipboard.json") }

    pub fn load() -> Self {
        Self::load_from(Self::path())
    }

    fn load_from(path: std::path::PathBuf) -> Self {
        let mut history = std::fs::read(&path).map(|raw| Self::from_json(&raw)).unwrap_or_default();
        history.path = Some(path);
        history
    }

    fn changed(&mut self) {
        self.version += 1;
        if let Some(path) = &self.path {
            if let Some(parent) = path.parent() { let _ = std::fs::create_dir_all(parent); }
            let _ = crate::model::atomic_write(path, &self.to_json());
        }
    }
}

/// 图片条目的缩略图（界面把 blob 解码成小尺寸贴图缓存起来）。
///
/// blob 是纯内容哈希，同一张图只会解码一次；解码走后台线程，界面先画占位、就绪后再贴图。
#[derive(Default)]
pub struct Thumbnails {
    entries: std::collections::HashMap<String, Entry>,
    tick: u64,
    worker: Option<Worker>,
}
struct Entry {
    /// 行内小图（最长边 `THUMB_SIZE`）。
    thumb: Slot,
    /// 悬停时的大图（最长边 `PREVIEW_SIZE`），解码好之后一直保留，悬停不必再等。
    preview: Slot,
    /// 最近一次被绘制的帧号，按容量淘汰时先丢最久没用的。
    last_used: u64,
}
#[derive(Default)]
struct Slot {
    image: Option<eframe::egui::TextureHandle>,
    /// 已请求解码（避免在结果回来前重复排队）。
    requested: bool,
    /// 已确认无法解码（blob 缺失或损坏），不再重试。
    failed: bool,
}
struct Worker {
    requests: std::sync::mpsc::SyncSender<(String, u32)>,
    results: std::sync::mpsc::Receiver<(String, Option<eframe::egui::ColorImage>)>,
}
/// 行内缩略图的最长边（逻辑像素）。行高 22 多一点，留余量给高 DPI。
pub const THUMB_SIZE: u32 = 96;
/// 悬停大图的最长边：足够看清截图里的文字，又不至于占满屏幕。
pub const PREVIEW_SIZE: u32 = 640;
/// 同时保留的缩略图数量。
const THUMB_CAPACITY: usize = 48;

/// 取图结果：已就绪、正在解码、或确认无法解码。
pub enum Thumb {
    Ready(eframe::egui::TextureHandle),
    Loading,
    Failed,
}

impl Thumbnails {
    /// 取行内小图。
    pub fn get(&mut self, ctx: &eframe::egui::Context, blob: &str, large: bool) -> Thumb {
        let size = if large { PREVIEW_SIZE } else { THUMB_SIZE };
        self.request(ctx, blob, size)
    }
    /// 直接请求某一档尺寸（两档可以同时排队，共用一次文件读取）。
    fn request(&mut self, ctx: &eframe::egui::Context, blob: &str, size: u32) -> Thumb {
        if self.worker.is_none() { self.start(ctx.clone()); }
        {
            let worker = self.worker.as_ref().expect("worker 刚启动");
            while let Ok((blob, image)) = worker.results.try_recv() {
                let Some(entry) = self.entries.get_mut(&blob) else { continue };
                let (image_size, slot) = if image.as_ref().is_some_and(|image| image.width().max(image.height()) as u32 > THUMB_SIZE) {
                    (PREVIEW_SIZE, &mut entry.preview)
                } else {
                    (THUMB_SIZE, &mut entry.thumb)
                };
                match image {
                    Some(image) => slot.image = Some(ctx.load_texture(format!("clip:{blob}:{image_size}"), image, eframe::egui::TextureOptions::LINEAR)),
                    None => slot.failed = true,
                }
                slot.requested = false;
            }
        }
        self.tick = self.tick.wrapping_add(1);
        let tick = self.tick;
        let entry = self.entries.entry(blob.to_string()).or_insert_with(|| Entry { thumb: Slot::default(), preview: Slot::default(), last_used: 0 });
        entry.last_used = tick;
        let slot = if size == PREVIEW_SIZE { &mut entry.preview } else { &mut entry.thumb };
        if let Some(texture) = &slot.image { return Thumb::Ready(texture.clone()); }
        if slot.failed { return Thumb::Failed; }
        if !slot.requested {
            let worker = self.worker.as_ref().expect("worker 刚启动");
            if worker.requests.try_send((blob.to_string(), size)).is_ok() { slot.requested = true; }
        }
        if self.entries.len() > THUMB_CAPACITY {
            let in_use = |entry: &&Entry| entry.thumb.requested || entry.preview.requested;
            if let Some(oldest) = self.entries.iter()
                .filter(|(key, entry)| key.as_str() != blob && !in_use(entry))
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| key.clone())
            {
                self.entries.remove(&oldest);
            }
        }
        Thumb::Loading
    }
    /// 测试用：直接把已解码好的贴图放进缓存，跳过后台线程。
    #[cfg(test)]
    pub fn insert_decoded(&mut self, blob: &str, image: eframe::egui::TextureHandle) {
        self.tick = self.tick.wrapping_add(1);
        let slot = Slot { image: Some(image), requested: false, failed: false };
        self.entries.insert(blob.to_string(), Entry { thumb: slot, preview: Slot::default(), last_used: self.tick });
    }
    /// 缓存的缩略图数量（测试用）。
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    /// 已排队的解码请求数（测试用）。
    #[cfg(test)]
    pub fn pending(&self) -> usize {
        self.entries.values().map(|entry| entry.thumb.requested as usize + entry.preview.requested as usize).sum()
    }
    /// 只保留这些 blob 的缩略图（历史被清空或条目被删除后释放贴图）。
    pub fn retain(&mut self, blobs: &[String]) {
        self.entries.retain(|blob, _| blobs.contains(blob));
    }
    pub fn clear(&mut self) {
        self.entries.clear();
    }
    fn start(&mut self, ctx: eframe::egui::Context) {
        let (request_tx, requests) = std::sync::mpsc::sync_channel::<(String, u32)>(16);
        let (results, result_rx) = std::sync::mpsc::sync_channel(16);
        let dir = blobs_dir();
        std::thread::Builder::new().name("daozai-clip-thumbs".into()).spawn(move || {
            // 同一张图的两次请求（小图 + 大图）共用一次读取和解码。
            let mut cache: Option<(String, eframe::egui::ColorImage)> = None;
            while let Ok((blob, size)) = requests.recv() {
                let full = match &cache {
                    Some((cached, image)) if *cached == blob => Some(image.clone()),
                    _ => std::fs::read(dir.join(&blob)).ok().and_then(|bytes| decode_image(&bytes)),
                };
                let Some(full) = full else {
                    cache = None;
                    if results.send((blob, None)).is_err() { break; }
                    ctx.request_repaint();
                    continue;
                };
                if cache.as_ref().is_none_or(|(cached, _)| *cached != blob) { cache = Some((blob.clone(), full.clone())); }
                let image = shrink(&full, size);
                if results.send((blob, Some(image))).is_err() { break; }
                ctx.request_repaint();
            }
        }).ok();
        self.worker = Some(Worker { requests: request_tx, results: result_rx });
    }
}

/// 读取 PNG blob 并缩放到最长边 `max`；解码失败返回 None。纯逻辑，可单测。
pub fn load_thumbnail(path: &std::path::Path, max: u32) -> Option<eframe::egui::ColorImage> {
    let bytes = std::fs::read(path).ok()?;
    thumbnail_from_bytes(&bytes, max)
}

/// 解码 PNG / JPEG / GIF 等为 RGBA 图像。纯逻辑，可单测。
fn decode_image(bytes: &[u8]) -> Option<eframe::egui::ColorImage> {
    let image = image::load_from_memory(bytes).ok()?.to_rgba8();
    if image.width() == 0 || image.height() == 0 { return None; }
    Some(eframe::egui::ColorImage::from_rgba_unmultiplied([image.width() as usize, image.height() as usize], image.as_raw()))
}

/// PNG 字节 → 缩略图。纯逻辑，可单测。
pub fn thumbnail_from_bytes(bytes: &[u8], max: u32) -> Option<eframe::egui::ColorImage> {
    Some(shrink(&decode_image(bytes)?, max))
}

/// 按最长边缩放（不放大）。纯逻辑，可单测。
fn shrink(image: &eframe::egui::ColorImage, max: u32) -> eframe::egui::ColorImage {
    if image.width() as u32 <= max && image.height() as u32 <= max { return image.clone(); }
    let w = (image.width() as u32).max(1);
    let h = (image.height() as u32).max(1);
    let scale = max as f32 / w.max(h) as f32;
    let (tw, th) = (((w as f32) * scale).round().max(1.0) as usize, ((h as f32) * scale).round().max(1.0) as usize);
    let mut out = eframe::egui::ColorImage::new([tw, th], image.pixels[0]);
    for y in 0..th {
        for x in 0..tw {
            // 盒式平均：缩小时比最近邻清楚得多，尤其是截图里的文字。
            let x0 = x * w as usize / tw;
            let x1 = ((x + 1) * w as usize / tw).max(x0 + 1).min(w as usize);
            let y0 = y * h as usize / th;
            let y1 = ((y + 1) * h as usize / th).max(y0 + 1).min(h as usize);
            let (mut r, mut g, mut b, mut a, mut n) = (0u64, 0u64, 0u64, 0u64, 0u64);
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let p = image.pixels[sy * image.width() + sx];
                    r += p.r() as u64; g += p.g() as u64; b += p.b() as u64; a += p.a() as u64; n += 1;
                }
            }
            let n = n.max(1);
            out.pixels[y * tw + x] = eframe::egui::Color32::from_rgba_unmultiplied((r / n) as u8, (g / n) as u8, (b / n) as u8, (a / n) as u8);
        }
    }
    out
}

/// 原始图片尺寸（悬停提示框里显示「1920 × 1080」这类信息）。纯逻辑。
pub fn image_size(path: &std::path::Path) -> Option<(u32, u32)> {
    let bytes = std::fs::read(path).ok()?;
    image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok()?.into_dimensions().ok()
}

/// 悬停提示框里的大图：最长边 300 逻辑点（截图里的字也能看清）。
pub const HOVER_MAX_SIDE: f32 = 300.0;
/// 悬停多久才弹出大图（与 egui 默认提示框节奏一致，避免鼠标划过时闪图）。
pub const HOVER_DELAY: f64 = 0.35;

/// 大图在卡片里占的位置：等比缩放到最长边不超过 `HOVER_MAX_SIDE`，且不放大。
pub fn hover_image_size(texture: eframe::egui::Vec2) -> eframe::egui::Vec2 {
    let longest = texture.x.max(texture.y).max(1.0);
    let scale = 1.0_f32.min(HOVER_MAX_SIDE / longest);
    eframe::egui::vec2((texture.x * scale).round().max(1.0), (texture.y * scale).round().max(1.0))
}

/// 界面与监听线程共用。
#[derive(Clone)]
pub struct Shared {
    pub history: Arc<Mutex<History>>,
    pub enabled: Arc<AtomicBool>,
    /// 图片缩略图缓存（仅界面线程使用）。
    pub thumbnails: Arc<Mutex<Thumbnails>>,
}

impl Shared {
    pub fn new(enabled: bool) -> Self {
        let mut history = History::load();
        history.max_recent = crate::model::Settings::load().clipboard_max_recent as usize;
        Self { history: Arc::new(Mutex::new(history)), enabled: Arc::new(AtomicBool::new(enabled)), thumbnails: Arc::new(Mutex::new(Thumbnails::default())) }
    }
    pub fn set_enabled(&self, on: bool) { self.enabled.store(on, Ordering::Relaxed); }
    pub fn is_enabled(&self) -> bool { self.enabled.load(Ordering::Relaxed) }
    pub fn lock(&self) -> std::sync::MutexGuard<'_, History> {
        self.history.lock().unwrap_or_else(|e| e.into_inner())
    }
    fn record_update(&self, text: &str, from_history: bool) -> bool {
        !from_history && self.record(text)
    }
    pub fn record(&self, text: &str) -> bool {
        if !self.is_enabled() { return false; }
        let at = chrono::Local::now().format("%H:%M").to_string();
        self.lock().push(text, &at)
    }
    /// 记录一张图片（PNG 字节）。返回是否新增。
    pub fn record_image(&self, png: &[u8]) -> bool {
        if !self.is_enabled() || png.is_empty() { return false; }
        use ring::digest;
        let digest = digest::digest(&digest::SHA256, png);
        let hash: String = digest.as_ref().iter().take(16).map(|b| format!("{b:02x}")).collect();
        let blob = format!("{hash}.png");
        let dir = blobs_dir();
        if std::fs::create_dir_all(&dir).is_err() { return false; }
        let path = dir.join(&blob);
        if !path.exists() && std::fs::write(&path, png).is_err() { return false; }
        let at = chrono::Local::now().format("%H:%M").to_string();
        self.lock().push_image(&blob, &at)
    }
}

impl Default for Shared {
    fn default() -> Self { Self::new(true) }
}

/// 列表里的一行预览：换行与连续空白压成一个空格。
pub fn preview(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ---------------------------------------------------------------- Windows 监听

/// 剪贴板监听的持有者；销毁时让监听线程退出并注销系统监听，不等待线程结束。
pub struct Watcher {
    #[cfg(windows)]
    stop: Arc<win::Stop>,
}

#[cfg(windows)]
impl Drop for Watcher {
    fn drop(&mut self) { self.stop.request(); }
}

#[cfg(windows)]
pub fn watch(shared: Shared, repaint: impl Fn() + Send + 'static) -> Option<Watcher> {
    let stop = Arc::new(win::Stop::default());
    let thread_stop = stop.clone();
    std::thread::Builder::new().name("daozai-clipboard".into()).spawn(move || {
        // 启动失败（极少见）时剪贴板页只是没有新记录，不影响其他功能。
        let _ = win::run(shared, Box::new(repaint), &thread_stop);
    }).ok()?;
    Some(Watcher { stop })
}

#[cfg(not(windows))]
pub fn watch(_shared: Shared, _repaint: impl Fn() + Send + 'static) -> Option<Watcher> { Some(Watcher {}) }

#[cfg(windows)]
const HISTORY_COPY_FORMAT: &str = "Daozai.ClipboardHistoryCopy";

#[cfg(windows)]
pub fn copy_history(text: &str) -> bool {
    use clipboard_win::{Clipboard, raw, register_format};
    let Some(format) = register_format(HISTORY_COPY_FORMAT) else { return false };
    let Ok(_clipboard) = Clipboard::new_attempts(10) else { return false };
    raw::set_string(text).is_ok() && raw::set_without_clear(format.get(), &[1]).is_ok()
}

/// 解析 CF_DIB（BITMAPINFOHEADER）为 RGBA 图像并编码 PNG。仅支持未压缩的 24/32 位 BI_RGB。
/// 失败返回 None。纯逻辑，可单测。
pub fn decode_dib(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < 40 {
        return None;
    }
    let u32_at = |o: usize| -> u32 { u32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]) };
    let i32_at = |o: usize| -> i32 { i32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]) };
    let header_size = u32_at(0) as usize;
    if header_size < 40 {
        return None;
    }
    let width = i32_at(4);
    let height = i32_at(8);
    let bpp = u16::from_le_bytes([data[14], data[15]]) as u32;
    let compression = u32_at(16);
    if width <= 0 || height == 0 || compression != 0 {
        return None;
    }
    let top_down = height < 0;
    let h = height.unsigned_abs() as usize;
    let w = width as usize;
    if bpp != 24 && bpp != 32 {
        return None;
    }
    let bytes_pp = (bpp / 8) as usize;
    let row_bytes = (w * bytes_pp + 3) & !3; // 4 字节对齐
    let pixel_offset = header_size; // BI_RGB 无调色板时紧随头
    let needed = pixel_offset + row_bytes * h;
    if data.len() < needed {
        return None;
    }
    let mut img = image::RgbaImage::new(w as u32, h as u32);
    for row in 0..h {
        let src_row = if top_down { row } else { h - 1 - row };
        let base = pixel_offset + src_row * row_bytes;
        for col in 0..w {
            let p = base + col * bytes_pp;
            let b = data[p];
            let g = data[p + 1];
            let r = data[p + 2];
            let a = if bytes_pp == 4 { data[p + 3] } else { 255 };
            img.put_pixel(col as u32, row as u32, image::Rgba([r, g, b, a]));
        }
    }
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

#[cfg(windows)]
mod win {
    use super::Shared;
    use std::cell::RefCell;
    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::{HGLOBAL, HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::DataExchange::*;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
    use windows::Win32::System::Ole::CF_UNICODETEXT;
    use windows::Win32::UI::WindowsAndMessaging::*;

    const WM_CLIPBOARDUPDATE: u32 = 0x031D;

    struct State { shared: Shared, repaint: Box<dyn Fn() + Send> }

    /// 监听线程建好消息窗口后登记线程号；持有者销毁时先置位再读线程号，两边总有一方看到对方。
    #[derive(Default)]
    pub struct Stop { requested: std::sync::atomic::AtomicBool, thread: std::sync::atomic::AtomicU32 }
    impl Stop {
        pub fn request(&self) {
            use std::sync::atomic::Ordering::SeqCst;
            self.requested.store(true, SeqCst);
            let thread = self.thread.load(SeqCst);
            if thread != 0 { unsafe { let _ = PostThreadMessageW(thread, WM_QUIT, WPARAM(0), LPARAM(0)); } }
        }
        fn register(&self) -> bool {
            use std::sync::atomic::Ordering::SeqCst;
            self.thread.store(unsafe { windows::Win32::System::Threading::GetCurrentThreadId() }, SeqCst);
            !self.requested.load(SeqCst)
        }
    }
    thread_local! { static STATE: RefCell<Option<State>> = const { RefCell::new(None) }; }

    unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        if msg == WM_CLIPBOARDUPDATE {
            STATE.with(|state| {
                if let Some(state) = state.borrow().as_ref() {
                    if state.shared.is_enabled() {
                        if let Some((text, from_history)) = read_text(hwnd) {
                            if state.shared.record_update(&text, from_history) { (state.repaint)(); }
                        } else if let Some(png) = read_image(hwnd) {
                            // 仅在有图片且无文字（或图片优先）时记录图片。
                            if state.shared.record_image(&png) { (state.repaint)(); }
                        }
                    }
                }
            });
            return LRESULT(0);
        }
        DefWindowProcW(hwnd, msg, wp, lp)
    }

    /// 密码管理器、远程桌面等会给敏感内容加上这些格式，表示不要进入剪贴板历史。
    unsafe fn excluded() -> bool {
        let exclude = RegisterClipboardFormatW(w!("ExcludeClipboardContentFromMonitorProcessing"));
        let ignore = RegisterClipboardFormatW(w!("Clipboard Viewer Ignore"));
        if (exclude != 0 && IsClipboardFormatAvailable(exclude).is_ok()) || (ignore != 0 && IsClipboardFormatAvailable(ignore).is_ok()) {
            return true;
        }
        // 值为 DWORD 0 表示不允许进入历史。
        let history = RegisterClipboardFormatW(w!("CanIncludeInClipboardHistory"));
        if history == 0 || IsClipboardFormatAvailable(history).is_err() { return false; }
        let Ok(handle) = GetClipboardData(history) else { return true; };
        let memory = HGLOBAL(handle.0);
        let ptr = GlobalLock(memory) as *const u32;
        if ptr.is_null() { return true; }
        let allowed = GlobalSize(memory) >= 4 && *ptr != 0;
        let _ = GlobalUnlock(memory);
        !allowed
    }

    unsafe fn read_text(hwnd: HWND) -> Option<(String, bool)> {
        if IsClipboardFormatAvailable(CF_UNICODETEXT.0 as u32).is_err() { return None; }
        // 其他程序可能正占用剪贴板，稍等重试几次。
        let mut opened = false;
        for _ in 0..5 {
            if OpenClipboard(hwnd).is_ok() { opened = true; break; }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        if !opened { return None; }
        let text = (|| {
            if excluded() { return None; }
            let handle = GetClipboardData(CF_UNICODETEXT.0 as u32).ok()?;
            let memory = HGLOBAL(handle.0);
            let ptr = GlobalLock(memory) as *const u16;
            if ptr.is_null() { return None; }
            let max = (GlobalSize(memory) / 2).min(super::MAX_CHARS * 2 + 1);
            let mut len = 0;
            while len < max && *ptr.add(len) != 0 { len += 1; }
            let text = (len <= super::MAX_CHARS * 2).then(|| String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len)));
            let _ = GlobalUnlock(memory);
            let from_history = clipboard_win::register_format(super::HISTORY_COPY_FORMAT)
                .is_some_and(|format| IsClipboardFormatAvailable(format.get()).is_ok());
            text.map(|text| (text, from_history))
        })();
        let _ = CloseClipboard();
        text
    }

    /// 读取剪贴板中的位图（CF_DIB），转换为 RGBA 并编码为 PNG。
    /// 不支持 CF_DIB 或解码失败时返回 None（静默跳过）。
    unsafe fn read_image(hwnd: HWND) -> Option<Vec<u8>> {
        const CF_DIB: u32 = 8;
        if IsClipboardFormatAvailable(CF_DIB).is_err() { return None; }
        let mut opened = false;
        for _ in 0..5 {
            if OpenClipboard(hwnd).is_ok() { opened = true; break; }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        if !opened { return None; }
        let png = (|| {
            if excluded() { return None; }
            let handle = GetClipboardData(CF_DIB).ok()?;
            let memory = HGLOBAL(handle.0);
            let base = GlobalLock(memory) as *const u8;
            if base.is_null() { return None; }
            let size = GlobalSize(memory);
            let bytes = std::slice::from_raw_parts(base, size);
            let result = super::decode_dib(bytes);
            let _ = GlobalUnlock(memory);
            result
        })();
        let _ = CloseClipboard();
        png
    }

    pub fn run(shared: Shared, repaint: Box<dyn Fn() + Send>, stop: &Stop) -> windows::core::Result<()> {
        unsafe {
            let instance = GetModuleHandleW(None)?;
            let class = w!("DaozaiClipboardWatcher");
            let wc = WNDCLASSW { lpfnWndProc: Some(proc), hInstance: instance.into(), lpszClassName: class, ..Default::default() };
            RegisterClassW(&wc);
            let hwnd = CreateWindowExW(WINDOW_EX_STYLE(0), class, PCWSTR::null(), WINDOW_STYLE(0), 0, 0, 0, 0, HWND_MESSAGE, None, instance, None)?;
            // 消息窗口建好后线程已有消息队列，此后投递的 WM_QUIT 不会丢。
            if !stop.register() { let _ = DestroyWindow(hwnd); return Ok(()); }
            STATE.with(|state| *state.borrow_mut() = Some(State { shared, repaint }));
            AddClipboardFormatListener(hwnd)?;
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            let _ = RemoveClipboardFormatListener(hwnd);
            let _ = DestroyWindow(hwnd);
            STATE.with(|state| *state.borrow_mut() = None);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn dropping_the_watcher_ends_the_listener_thread(){
        // 先停后启和启动后再停两种顺序都必须让线程退出，不能留下常驻的监听窗口。
        for stop_first in [true,false] {
            let stop=Arc::new(win::Stop::default());let thread_stop=stop.clone();
            let shared=Shared{history:Default::default(),enabled:Arc::new(AtomicBool::new(false)),thumbnails:Default::default()};
            if stop_first {stop.request();}
            let (done,finished)=std::sync::mpsc::channel();
            std::thread::spawn(move||{let _=win::run(shared,Box::new(||{}),&thread_stop);let _=done.send(());});
            if !stop_first {std::thread::sleep(std::time::Duration::from_millis(200));stop.request();}
            finished.recv_timeout(std::time::Duration::from_secs(3)).expect("监听线程应在停止后退出");
        }
    }

    #[test]
    fn history_copy_preserves_order_but_external_copy_promotes() {
        let shared = Shared { history: Default::default(), enabled: Arc::new(AtomicBool::new(true)), thumbnails: Default::default() };
        shared.lock().items.clear();
        shared.record("较早的一条");
        shared.record("最近的一条");
        let before = shared.lock().items.clone();
        assert!(!shared.record_update("较早的一条", true));
        assert_eq!(shared.lock().items, before);
        assert!(shared.record_update("较早的一条", false));
        assert_eq!(shared.lock().items[0].text, "较早的一条");
    }
    #[test]
    fn history_dedupes_caps_and_keeps_pins() {
        let mut h = History::default();
        assert!(!h.push("   \n", "09:00"), "空白不记录");
        assert!(h.push("a", "09:00") && h.push("b", "09:01"));
        assert!(!h.push("b", "09:02"), "与最新一条相同不重复记录");
        assert!(h.push("a", "09:03"));
        assert_eq!(h.items.iter().map(|c| c.text.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(h.items[0].at, "09:03");
        h.items[1].pinned = true; // b
        for i in 0..(MAX_RECENT + 5) { h.push(&format!("n{i}"), "10:00"); }
        assert_eq!(h.items.iter().filter(|c| !c.pinned).count(), MAX_RECENT);
        assert!(h.items.iter().any(|c| c.text == "b" && c.pinned), "固定的不会被挤掉");
        h.clear_recent();
        assert_eq!(h.items.len(), 1);
        assert!(!h.push(&"x".repeat(MAX_CHARS + 1), "10:00"), "超长内容不记录");
    }

    #[test]
    fn reopening_restores_history_and_respects_manual_cleanup() {
        let path = std::env::temp_dir().join(format!("daozai-clip-{}.json", std::process::id()));
        std::fs::write(&path, br#"[{"text":"old","at":"08:00"}]"#).unwrap();
        let mut history = History::load_from(path.clone());
        assert!(history.items[0].pinned);
        history.push("保留", "09:00");
        history.push("删除", "09:01");
        history.remove(0);
        let mut reopened = History::load_from(path.clone());
        assert_eq!(reopened.items, history.items);
        reopened.clear_recent();
        let final_history = History::load_from(path.clone());
        assert_eq!(final_history.items.len(), 1);
        assert_eq!(final_history.items[0].text, "old");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn pinned_round_trip_and_preview() {
        let mut h = History::default();
        h.push("第一行\n第二行  末尾", "08:00");
        h.push("临时", "08:01");
        h.items[1].pinned = true;
        let back = History::from_json(&h.to_json());
        assert_eq!(back.items, h.items);
        assert_eq!(preview(&back.items[1].text), "第一行 第二行 末尾");
        assert!(History::from_json(b"not json").items.is_empty());
    }

    #[test]
    fn image_and_note_and_search() {
        let mut h = History::default();
        h.push("hello world", "08:00");
        h.push_image("abc.png", "08:01");
        h.push("密码：123", "08:02");
        h.set_note(0, "重要链接");
        assert_eq!(h.items[0].note, "重要链接");
        // 搜索：文字命中。注意图片条目在 index1。
        let idx = h.search_indices("hello");
        assert_eq!(idx, vec![2]);
        // 备注命中。
        let idx = h.search_indices("重要");
        assert_eq!(idx, vec![0]);
        // 图片标记命中。
        let idx = h.search_indices("图片");
        assert_eq!(idx, vec![1]);
        // 空查询返回全部。
        assert_eq!(h.search_indices("").len(), 3);
        // JSON 往返保留 kind/note/image。
        let back = History::from_json(&h.to_json());
        assert_eq!(back.items, h.items);
        assert_eq!(back.items[1].kind, ClipKind::Image);
        assert_eq!(back.items[1].image.as_deref(), Some("abc.png"));
    }

    #[test]
    fn image_dedupes_and_caps() {
        let mut h = History::default();
        h.max_recent = 2;
        assert!(h.push_image("a.png", "08:00"));
        assert!(!h.push_image("a.png", "08:01")); // 与最新相同不重复
        assert!(h.push_image("b.png", "08:02"));
        assert!(h.push("文字", "08:03"));
        // 上限 2 条未固定。
        assert_eq!(h.items.iter().filter(|c| !c.pinned).count(), 2);
    }

    #[test]
    fn thumbnail_scales_png_to_fit_and_rejects_garbage() {
        // 构造 200×100 的 PNG，缩略图最长边 96 → 96×48。
        let mut img = image::RgbaImage::new(200, 100);
        for (x, y, p) in img.enumerate_pixels_mut() { *p = image::Rgba([x as u8, (y * 2) as u8, 128, 255]); }
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        let png = out.into_inner();
        let thumb = thumbnail_from_bytes(&png, THUMB_SIZE).expect("PNG 应可解码为缩略图");
        assert_eq!(thumb.size, [96, 48]);
        // 同一张图的悬停大图按 PREVIEW_SIZE 缩放，比例不变。
        let large = thumbnail_from_bytes(&png, PREVIEW_SIZE).unwrap();
        assert_eq!(large.size, [200, 100], "原图小于大图上限时不放大");
        // 小图不放大。
        let mut small = image::RgbaImage::new(8, 6);
        small.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        small.write_to(&mut out, image::ImageFormat::Png).unwrap();
        assert_eq!(thumbnail_from_bytes(&out.into_inner(), THUMB_SIZE).unwrap().size, [8, 6]);
        assert!(thumbnail_from_bytes(b"not an image", THUMB_SIZE).is_none());
        assert!(load_thumbnail(std::path::Path::new("不存在的文件.png"), THUMB_SIZE).is_none());
    }

    #[test]
    fn preview_preserves_aspect_and_box_filters_color() {
        // 4×1 的红蓝条纹缩到 2×1：每格取两个像素的平均，颜色不会丢。
        let mut img = image::RgbaImage::new(4, 1);
        img.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
        img.put_pixel(1, 0, image::Rgba([0, 0, 255, 255]));
        img.put_pixel(2, 0, image::Rgba([255, 0, 0, 255]));
        img.put_pixel(3, 0, image::Rgba([0, 0, 255, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        let small = thumbnail_from_bytes(&out.into_inner(), 2).unwrap();
        assert_eq!(small.size, [2, 1]);
        // 盒式平均把红蓝混成紫色；右侧像素来自第 3、4 列，同样是红蓝各一半。
        let left = small.pixels[0];
        assert!((left.r() as i32 - 128).abs() <= 2 && (left.b() as i32 - 128).abs() <= 2, "应取两列平均值，实际 {left:?}");
        assert_eq!(small.pixels[0], small.pixels[1]);
        // 超长竖图缩到 2 时高度成正比（不压成正方形）。
        let mut tall = image::RgbaImage::new(10, 100);
        for p in tall.pixels_mut() { *p = image::Rgba([10, 200, 30, 255]); }
        let mut out = std::io::Cursor::new(Vec::new());
        tall.write_to(&mut out, image::ImageFormat::Png).unwrap();
        assert_eq!(thumbnail_from_bytes(&out.into_inner(), 20).unwrap().size, [2, 20]);
    }

    #[test]
    fn hover_preview_is_requested_separately_and_sizes_capped() {
        let ctx = eframe::egui::Context::default();
        let mut thumbs = Thumbnails::default();
        // 第一次取小图：排队，返回 Loading（不会立刻有图）。
        assert!(matches!(thumbs.get(&ctx, "a.png", false), Thumb::Loading));
        // 同一张图再要小图不会重复排队；要大图是另一档尺寸，会再排一次。
        assert_eq!(thumbs.pending(), 1);
        assert!(matches!(thumbs.get(&ctx, "a.png", false), Thumb::Loading));
        assert_eq!(thumbs.pending(), 1);
        assert!(matches!(thumbs.get(&ctx, "a.png", true), Thumb::Loading));
        assert_eq!(thumbs.pending(), 1 + 1, "大图与小图各排一次");
        // 大图展示尺寸：等比缩放到最长边 300，不放大。
        assert_eq!(hover_image_size(eframe::egui::vec2(1920.0, 1080.0)), eframe::egui::vec2(300.0, 169.0));
        assert_eq!(hover_image_size(eframe::egui::vec2(120.0, 80.0)), eframe::egui::vec2(120.0, 80.0));
        assert_eq!(hover_image_size(eframe::egui::vec2(40.0, 900.0)), eframe::egui::vec2(13.0, 300.0));
    }

    #[test]
    fn original_image_size_is_read_from_headers() {
        let path = std::env::temp_dir().join(format!("daozai-size-{}.png", std::process::id()));
        image::RgbaImage::new(37, 19).save(&path).unwrap();
        assert_eq!(image_size(&path), Some((37, 19)));
        std::fs::remove_file(&path).unwrap();
        assert_eq!(image_size(&path), None);
    }

    #[test]
    fn decode_dib_minimal_24bpp() {
        // 构造一个 2x1 24bpp 的 BITMAPINFOHEADER + 像素（行对齐到 4 字节）。
        // 行字节 = 2*3=6 → 对齐 8。
        let mut data = Vec::new();
        data.extend_from_slice(&40u32.to_le_bytes()); // biSize
        data.extend_from_slice(&2i32.to_le_bytes()); // width
        data.extend_from_slice(&1i32.to_le_bytes()); // height (bottom-up)
        data.extend_from_slice(&1u16.to_le_bytes()); // planes
        data.extend_from_slice(&24u16.to_le_bytes()); // bpp
        data.extend_from_slice(&0u32.to_le_bytes()); // compression BI_RGB
        data.extend_from_slice(&[0u8; 40 - 20]); // 剩余 header 字段
        // 像素 BGR：红、绿 + 2 字节对齐填充
        data.extend_from_slice(&[0, 0, 255, 0, 255, 0, 0, 0]);
        let png = decode_dib(&data).expect("应能解码");
        assert!(png.starts_with(&[0x89, b'P', b'N', b'G']));
        // 非法数据返回 None。
        assert!(decode_dib(&[0u8; 10]).is_none());
    }
}
