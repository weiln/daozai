//! 便签窗口：Alt+C 唤起的独立便签板。
//!
//! 版式与配色都照着岛内的「新建日程」表单来：标题和右侧说明同排、标签与字段
//! 左右对齐、底部右侧放主按钮；颜色跟随岛体皮肤，毛玻璃皮肤再压一层顶部柔光。
//! 上半部分是输入框，下半部分是便签列表；窗口无边框、圆角，和岛体是同一套语言。

use crate::app::App;
use crate::model::Skin;
use crate::notes::{self, Added};
use crate::skin_paint;
use crate::theme::{self, W};
use eframe::egui::{self, Color32, RichText};

pub const WIDTH: f32 = 420.0;
pub const HEIGHT: f32 = 520.0;

/// 面板内边距、标签列宽：比例沿用岛内日程表单（标签 43、字段 86）。
const PAD_X: f32 = 32.0;
const PAD_Y: f32 = 16.0;
const LABEL_W: f32 = 40.0;
const LABEL_GAP: f32 = 10.0;
/// 输入框高度：窗口上半部分。
const INPUT_H: f32 = 170.0;
/// 字段与便签卡的圆角。
const FIELD_RADIUS: f32 = 10.0;
/// 底部按钮高度（日程表单是 22×1.4 ≈ 31，这里取 28）。
const BUTTON_H: f32 = 28.0;
/// 列表里的小胶囊（编辑 / 复制 / 删除、清空全部）。
const CHIP_H: f32 = 22.0;
const PANEL_RADIUS: f32 = 18.0;
/// 列表里每条便签最多显示的行数。
const PREVIEW_ROWS: usize = 2;
/// 删除 / 清空的二次确认有效时间（秒）。
const CONFIRM_SECS: f64 = 3.0;

// 字号：跟岛内日程表单（13 / 10 / 9）同一档，窗口整体不再放大。
const TITLE_SIZE: f32 = 13.5;
const BODY_SIZE: f32 = 12.0;
const LABEL_SIZE: f32 = 10.5;
const SMALL_SIZE: f32 = 9.5;
const BUTTON_SIZE: f32 = 10.5;

fn vp_id() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("daozai-notes")
}

/// 各皮肤对应的调色板（与 App::skin_palette 保持一致）。
fn palette(skin: Skin) -> Option<skin_paint::Palette> {
    match skin {
        Skin::Cream => Some(skin_paint::CREAM),
        Skin::GlassLight => Some(skin_paint::GLASS_LIGHT),
        Skin::GlassDark => Some(skin_paint::GLASS_DARK),
        Skin::Classic | Skin::Fluid => None,
    }
}

/// 便签窗口使用的一整套颜色与材质。
#[derive(Clone, Copy)]
struct Look {
    light: bool,
    /// 窗口底色：经典纯黑、奶油纸色、毛玻璃半透明玻璃底。
    bg: Color32,
    /// 输入框与便签卡底（岛体的 theme::CARD）。
    field: Color32,
    /// 普通按钮底（theme::BTN）。
    button: Color32,
    ink: Color32,
    sub: Color32,
    dim: Color32,
    accent: Color32,
    selected: Color32,
    danger: Color32,
    danger_fill: Color32,
    /// 二次确认底上的文字色。
    on_danger: Color32,
    line: Color32,
    veil: Color32,
    /// 毛玻璃皮肤的顶部柔光强度（0 表示没有）。
    sheen: u8,
}

impl Look {
    fn for_skin(skin: Skin) -> Self {
        // 窗口底色跟岛体面板同一套皮肤材质：毛玻璃用半透明玻璃底，
        // 叠在桌面上像一块压暗的磨砂玻璃，而不是死黑一块。
        let bg = match skin {
            Skin::Classic => Color32::BLACK,
            Skin::Cream => {
                let [r, g, b] = skin_paint::CREAM.paper;
                Color32::from_rgb(r, g, b)
            }
            // 独立窗口没有系统模糊，玻璃底做成不透明，否则桌面的字会透进面板。
            Skin::GlassLight => Color32::from_rgb(246, 247, 250),
            Skin::GlassDark => Color32::from_rgb(96, 96, 99),
            Skin::Fluid => Color32::from_rgb(13, 13, 16),
        };
        // 组件颜色按「叠在这块玻璃上」来算，避免把半透明色画成黑块。
        let [pr, pg, pb, _] = bg.to_srgba_unmultiplied();
        let solid = Color32::from_rgb(pr, pg, pb);
        let pal = palette(skin);
        let map = |color: Color32| pal.map_or(color, |pal| flatten(skin_paint::map_color(color, &pal), solid));
        let light = (0.299 * pr as f32 + 0.587 * pg as f32 + 0.114 * pb as f32) / 255.0 > 0.5;
        let ink = map(theme::WHITE);
        let seen = |color: Color32| {
            let [r, g, b, _] = color.to_srgba_unmultiplied();
            [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0]
        };
        // 次要文字也要看得清：皮肤映射后的灰字对比度不够时，往墨色/白色方向提一点。
        let readable = |color: Color32, min: f32| {
            let toward = if light { Color32::BLACK } else { Color32::WHITE };
            let mut out = color;
            for _ in 0..10 {
                if crate::glass::contrast(seen(out), seen(bg)) >= min {
                    break;
                }
                out = mix(out, toward, 0.15);
            }
            out
        };
        Self {
            light,
            bg,
            field: map(theme::CARD),
            button: map(theme::BTN),
            ink,
            sub: readable(map(theme::GRAY), 4.5),
            dim: readable(map(theme::DIM), 3.2),
            accent: map(Color32::from_rgb(82, 162, 255)),
            selected: map(Color32::from_rgb(28, 65, 100)),
            danger: map(theme::RED),
            danger_fill: map(Color32::from_rgb(120, 28, 36)),
            on_danger: if light { ink } else { Color32::WHITE },
            line: map(Color32::from_white_alpha(18)),
            veil: map(Color32::from_white_alpha(22)),
            sheen: if skin == Skin::GlassDark { 34 } else { 0 },
        }
    }
}

/// 独立窗口没有系统模糊，毛玻璃皮肤的半透明底会把桌面细节透出来；
/// 这里按纸色压成不透明，文字对比度才稳定。
fn flatten(color: Color32, base: Color32) -> Color32 {
    let a = color.a();
    if a == 255 {
        return Color32::from_rgb(color.r(), color.g(), color.b());
    }
    if a == 0 {
        return base;
    }
    // Color32 是预乘的，这里直接用 sRGB 数值还原，不能走 to_srgba_unmultiplied：
    // 那条路径会经过线性空间，半透明的玻璃色会整体变暗。
    let straight = |v: u8| (v as u32 * 255 / a as u32).min(255) as u8;
    let t = a as f32 / 255.0;
    let mix = |base: u8, top: u8| (base as f32 + (top as f32 - base as f32) * t).round() as u8;
    Color32::from_rgb(mix(base.r(), straight(color.r())), mix(base.g(), straight(color.g())), mix(base.b(), straight(color.b())))
}

/// 便签窗口的运行状态。
pub struct State {
    pub open: bool,
    pub focus_requested: bool,
    pub board: notes::Notes,
    pub input: String,
    pub hotkey: Option<notes::Hotkey>,
    /// 首次打开把焦点放到输入框。
    pub focus_input: bool,
    /// 新建便签后让列表回到顶部。
    pub scroll_top: bool,
    /// 正在编辑的便签下标；保存时改写这条而不是新建。
    pub editing: Option<usize>,
    /// 待确认删除的下标与开始时间。
    pub confirm_delete: Option<usize>,
    pub confirm_at: f64,
    /// 「清空全部」的二次确认与开始时间。
    pub confirm_clear: bool,
    pub clear_at: f64,
    /// 窗口内的一次性提示。
    pub status: String,
    pub status_at: f64,
}

impl State {
    pub fn load() -> Self {
        Self {
            open: false,
            focus_requested: false,
            board: notes::Notes::load(),
            input: String::new(),
            hotkey: None,
            focus_input: false,
            scroll_top: false,
            editing: None,
            confirm_delete: None,
            confirm_at: 0.0,
            confirm_clear: false,
            clear_at: 0.0,
            status: String::new(),
            status_at: 0.0,
        }
    }

    pub fn open_window(&mut self) {
        self.open = true;
        self.focus_requested = true;
        self.focus_input = true;
        self.confirm_delete = None;
        self.confirm_clear = false;
    }

    pub fn close_window(&mut self) {
        self.open = false;
        self.confirm_delete = None;
        self.confirm_clear = false;
    }

    pub fn toggle(&mut self) {
        if self.open {
            self.close_window();
        } else {
            self.open_window();
        }
    }

    fn say(&mut self, message: impl Into<String>, now: f64) {
        self.status = message.into();
        self.status_at = now;
    }

    fn confirming_clear(&self, now: f64) -> bool {
        self.confirm_clear && now - self.clear_at < CONFIRM_SECS
    }

    /// 退出编辑状态；clear_input 为 true 时顺便清掉草稿。
    fn stop_editing(&mut self, clear_input: bool) {
        self.editing = None;
        if clear_input {
            self.input.clear();
        }
        self.focus_input = true;
    }
}

impl App {
    /// 每帧调用：处理 Alt+C、轮询便签窗口并绘制。
    pub fn notes_ui(&mut self, ctx: &egui::Context) {
        if self.settings.notes_enabled && self.notes.hotkey.is_none() {
            self.notes.hotkey = notes::Hotkey::start();
        }
        if self.settings.notes_enabled && self.notes.hotkey.as_ref().is_some_and(|hotkey| hotkey.take()) {
            let was_open = self.notes.open;
            self.notes.toggle();
            if was_open {
                ctx.send_viewport_cmd_to(vp_id(), egui::ViewportCommand::Visible(false));
            }
        }
        if !self.settings.notes_enabled && self.notes.open {
            self.notes.close_window();
            ctx.send_viewport_cmd_to(vp_id(), egui::ViewportCommand::Visible(false));
        }
        if !self.notes.open {
            return;
        }

        let focus_requested = std::mem::take(&mut self.notes.focus_requested);
        let mut close = false;
        ctx.show_viewport_immediate(
            vp_id(),
            egui::ViewportBuilder::default()
                .with_title("岛仔 · 便签")
                .with_icon(theme::app_icon())
                .with_inner_size([WIDTH, HEIGHT])
                .with_decorations(false)
                .with_transparent(true)
                .with_resizable(false)
                .with_visible(false)
                .with_active(false)
                .with_always_on_top()
                .with_window_level(egui::WindowLevel::AlwaysOnTop),
            |ctx, _| {
                ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(egui::SystemTheme::Dark));
                if focus_requested {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                if ctx.input(|i| i.viewport().close_requested()) {
                    close = true;
                }
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(ctx, |ui| {
                        if self.notes_panel(ui) {
                            close = true;
                        }
                    });
            },
        );

        if close {
            self.notes.close_window();
            ctx.send_viewport_cmd_to(vp_id(), egui::ViewportCommand::Visible(false));
        } else {
            ctx.request_repaint();
        }
    }

    /// 画便签窗口内容；返回 true 表示请求关闭窗口。测试可直接调用。
    pub(crate) fn notes_panel(&mut self, ui: &mut egui::Ui) -> bool {
        let rect = ui.max_rect();
        let look = Look::for_skin(self.settings.skin);
        let now = ui.input(|i| i.time);
        let mut close = false;
        let mut save = false;
        let mut clear_all = false;
        let mut delete: Option<usize> = None;
        let mut copy: Option<String> = None;
        let mut edit: Option<usize> = None;
        if !self.notes.status.is_empty() && now - self.notes.status_at > 2.4 {
            self.notes.status.clear();
        }
        if self.notes.confirm_delete.is_some() && now - self.notes.confirm_at >= CONFIRM_SECS {
            self.notes.confirm_delete = None;
        }
        if self.notes.confirm_clear && now - self.notes.clear_at >= CONFIRM_SECS {
            self.notes.confirm_clear = false;
        }
        // Esc：编辑中取消编辑，否则收起窗口。
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            if self.notes.editing.is_some() {
                self.notes.stop_editing(true);
            } else {
                close = true;
            }
        }
        let editing = self.notes.editing.is_some();

        // 面板：一整张圆角卡片撑出岛体轮廓，毛玻璃皮肤加顶部柔光。
        let frame = rect.shrink(0.5);
        ui.painter().rect_filled(frame, PANEL_RADIUS, look.bg);
        if look.sheen > 0 {
            top_sheen(ui.painter(), frame, PANEL_RADIUS, look.bg, look.sheen);
        }
        ui.painter().rect_stroke(frame, PANEL_RADIUS, egui::Stroke::new(1.0_f32, look.line), egui::StrokeKind::Inside);

        // 顶部空白处可以拖动窗口。
        let drag_rect = egui::Rect::from_min_max(rect.min, egui::pos2(rect.right() - 80.0, rect.top() + 46.0));
        let drag = ui.interact(drag_rect, egui::Id::new("notes-drag"), egui::Sense::drag());
        if drag.drag_started() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }

        let inner = egui::Rect::from_min_max(rect.min + egui::vec2(PAD_X, PAD_Y), rect.max - egui::vec2(PAD_X, PAD_Y));
        let mut content = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::Min)));
        {
            let style = content.style_mut();
            style.visuals = if look.light { egui::Visuals::light() } else { egui::Visuals::dark() };
            let visuals = &mut style.visuals;
            visuals.text_cursor.stroke = egui::Stroke::new(1.5_f32, look.accent);
            visuals.selection.bg_fill = alpha(look.accent, 96);
            visuals.selection.stroke = egui::Stroke::new(1.0_f32, look.ink);
            visuals.widgets.noninteractive.bg_stroke = egui::Stroke::NONE;
            visuals.widgets.inactive.bg_fill = alpha(look.ink, 36);
            visuals.widgets.hovered.bg_fill = alpha(look.ink, 64);
            visuals.widgets.active.bg_fill = alpha(look.ink, 84);
            visuals.extreme_bg_color = look.field;
            style.spacing.item_spacing = egui::vec2(6.0, 6.0);
            style.spacing.scroll = egui::style::ScrollStyle::thin();
        }

        // 标题行：跟日程表单一样，左边标题、右边「仅保存在本机」。
        content.horizontal(|ui| {
            ui.label(RichText::new(if editing { "编辑便签" } else { "便签" }).font(theme::font(TITLE_SIZE, W::Semibold)).color(look.ink));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icon_button(ui, &look, "×", "关闭（Esc）") {
                    close = true;
                }
                ui.add_space(2.0);
                if editing {
                    pill(ui, &look, "编辑中", look.selected, look.ink, 18.0, SMALL_SIZE, W::Regular, false);
                    ui.add_space(4.0);
                }
                ui.label(RichText::new("仅保存在本机").font(theme::font(SMALL_SIZE, W::Regular)).color(look.sub));
            });
        });
        content.add_space(10.0);

        // 上半部分：标签 + 输入框，跟「名称」一行同样的结构。
        // 高度固定，草稿再长也不会把窗口顶开。
        let row = content.available_rect_before_wrap();
        let field_width = (row.width() - LABEL_W - LABEL_GAP).max(0.0);
        content.painter().text(
            row.min + egui::vec2(0.0, 1.0),
            egui::Align2::LEFT_TOP,
            "内容",
            theme::font(LABEL_SIZE, W::Regular),
            look.sub,
        );
        let field = egui::Rect::from_min_size(
            egui::pos2(row.left() + LABEL_W + LABEL_GAP, row.top()),
            egui::vec2(field_width, INPUT_H),
        );
        content.painter().rect_filled(field, FIELD_RADIUS, look.field);
        if look.light {
            content.painter().rect_stroke(field, FIELD_RADIUS, egui::Stroke::new(1.0_f32, look.line), egui::StrokeKind::Inside);
        }
        {
            let mut field_ui = content.new_child(
                egui::UiBuilder::new()
                    .max_rect(field.shrink2(egui::vec2(10.0, 8.0)))
                    .layout(egui::Layout::top_down(egui::Align::Min)),
            );
            egui::ScrollArea::vertical()
                .id_salt("notes-input-scroll")
                .max_height(INPUT_H - 16.0)
                .auto_shrink([false, false])
                .show(&mut field_ui, |ui| {
                    ui.set_width(ui.available_width());
                    let edit = egui::TextEdit::multiline(&mut self.notes.input)
                        .id(egui::Id::new("notes-input"))
                        .frame(false)
                        .hint_text(RichText::new(if editing { "修改这条便签…" } else { "写点什么…" }).font(theme::font(BODY_SIZE, W::Regular)).color(look.dim))
                        .text_color(look.ink)
                        .font(theme::font(BODY_SIZE, W::Regular))
                        .desired_width(f32::INFINITY)
                        .min_size(egui::vec2(0.0, INPUT_H - 16.0));
                    let response = ui.add(edit);
                    if std::mem::take(&mut self.notes.focus_input) {
                        response.request_focus();
                    }
                    if ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter)) {
                        save = true;
                    }
                });
        }
        content.advance_cursor_after_rect(egui::Rect::from_min_size(row.min, egui::vec2(row.width(), INPUT_H)));

        // 字段下方的说明行，对应日程表单的「日期 年-月-日 · 时间 时:分」。
        content.add_space(4.0);
        content.horizontal(|ui| {
            ui.add_space(LABEL_W + LABEL_GAP);
            let hint = if editing {
                "Esc 取消编辑 · Ctrl+Enter 保存修改"
            } else if self.settings.notes_enabled {
                "Alt+C 唤出 · Ctrl+Enter 保存 · Esc 收起"
            } else {
                "Ctrl+Enter 保存 · Esc 收起 · 快捷键已在设置中关闭"
            };
            ui.label(RichText::new(hint).font(theme::font(SMALL_SIZE, W::Regular)).color(look.dim));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let used = self.notes.input.chars().count();
                if !self.notes.status.is_empty() {
                    let color = if self.notes.status.contains("失败") || self.notes.status.contains("空") { look.danger } else { look.sub };
                    ui.label(RichText::new(self.notes.status.clone()).font(theme::font(SMALL_SIZE, W::Regular)).color(color));
                } else if used > notes::MAX_CHARS {
                    ui.label(RichText::new(format!("超过 {} 字，保存时截断", notes::MAX_CHARS)).font(theme::font(SMALL_SIZE, W::Regular)).color(look.danger));
                } else if used > 0 {
                    ui.label(RichText::new(format!("{used}/{}", notes::MAX_CHARS)).font(theme::font(SMALL_SIZE, W::Regular)).color(look.dim));
                }
            });
        });

        // 底部操作行：左边清空，右边取消 / 保存，跟日程表单同一排法。
        content.add_space(8.0);
        content.horizontal(|ui| {
            let can_clear = editing || !self.notes.input.is_empty();
            let label = if editing { "取消编辑" } else { "清空输入" };
            if pill(ui, &look, label, look.button, if can_clear { look.ink } else { look.dim }, BUTTON_H, BUTTON_SIZE, W::Regular, can_clear).clicked() {
                self.notes.stop_editing(true);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let can_save = !self.notes.input.trim().is_empty();
                let label = if editing { "保存修改" } else { "保存到本机" };
                let fill = if can_save { look.accent } else { look.button };
                let text = if can_save { Color32::WHITE } else { look.dim };
                if pill(ui, &look, label, fill, text, BUTTON_H, BUTTON_SIZE, W::Semibold, can_save).clicked() {
                    save = true;
                }
                ui.add_space(4.0);
                if pill(ui, &look, "取消", look.button, look.ink, BUTTON_H, BUTTON_SIZE, W::Regular, true).clicked() {
                    close = true;
                }
            });
        });

        // 分隔线 + 列表标题。
        content.add_space(10.0);
        let (line, _) = content.allocate_exact_size(egui::vec2(content.available_width(), 1.0), egui::Sense::hover());
        content.painter().rect_filled(line, 0.0, look.line);
        content.add_space(8.0);
        content.horizontal(|ui| {
            ui.label(RichText::new("最近便签").font(theme::font(11.0, W::Semibold)).color(look.ink));
            ui.label(RichText::new(format!("{} 条", self.notes.board.items.len())).font(theme::font(SMALL_SIZE, W::Regular)).color(look.sub));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let confirming = self.notes.confirming_clear(now);
                let enabled = !self.notes.board.items.is_empty();
                let fill = if confirming { look.danger_fill } else { look.button };
                let text = if confirming { look.on_danger } else if enabled { look.ink } else { look.dim };
                if pill(ui, &look, if confirming { "确认清空" } else { "清空全部" }, fill, text, CHIP_H, SMALL_SIZE, W::Regular, enabled).clicked() {
                    clear_all = true;
                }
            });
        });
        content.add_space(6.0);

        // 下半部分：便签列表。
        let list_h = content.available_height().max(72.0);
        let confirming_delete = self.notes.confirm_delete;
        let confirm_at = self.notes.confirm_at;
        let board = &self.notes.board;
        let mut next_confirm = confirming_delete;
        let mut area = egui::ScrollArea::vertical()
            .id_salt("notes-list")
            .auto_shrink([false, false])
            .max_height(list_h);
        if std::mem::take(&mut self.notes.scroll_top) {
            // 新建后回到顶部，立即看到刚保存的便签。
            area = area.vertical_scroll_offset(0.0);
        }
        area.show(&mut content, |ui| {
            ui.set_width(ui.available_width());
            if board.items.is_empty() {
                empty_state(ui, &look);
                return;
            }
            let today = chrono::Local::now();
            for (index, note) in board.items.iter().enumerate() {
                let confirming = confirming_delete == Some(index) && now - confirm_at < CONFIRM_SECS;
                let being_edited = self.notes.editing == Some(index);
                let mut frame = egui::Frame::new()
                    .fill(look.field)
                    .corner_radius(FIELD_RADIUS)
                    .inner_margin(egui::Margin { left: 10, right: 10, top: 8, bottom: 8 });
                if being_edited {
                    frame = frame.stroke(egui::Stroke::new(1.0_f32, look.accent));
                }
                frame.show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    note_preview(ui, &note.text, &look);
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(notes::time_label(note.at, today)).font(theme::font(SMALL_SIZE, W::Regular)).color(look.sub));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let label = if confirming { "确认删除" } else { "删除" };
                            let fill = if confirming { look.danger_fill } else { look.button };
                            let text = if confirming { look.on_danger } else { look.danger };
                            if pill(ui, &look, label, fill, text, CHIP_H, SMALL_SIZE, W::Regular, true).clicked() {
                                if confirming {
                                    delete = Some(index);
                                } else {
                                    next_confirm = Some(index);
                                }
                            }
                            if pill(ui, &look, "复制", look.button, look.ink, CHIP_H, SMALL_SIZE, W::Regular, true).clicked() {
                                copy = Some(note.text.clone());
                            }
                            let label = if being_edited { "编辑中" } else { "编辑" };
                            let fill = if being_edited { look.selected } else { look.button };
                            if pill(ui, &look, label, fill, look.ink, CHIP_H, SMALL_SIZE, W::Regular, !being_edited).clicked() {
                                edit = Some(index);
                            }
                        });
                    });
                });
                ui.add_space(6.0);
            }
        });
        if next_confirm != confirming_delete {
            self.notes.confirm_delete = next_confirm;
            self.notes.confirm_at = now;
        }

        if let Some(index) = edit {
            if index < self.notes.board.items.len() {
                self.notes.editing = Some(index);
                self.notes.input = self.notes.board.items[index].text.clone();
                self.notes.focus_input = true;
                self.notes.status.clear();
                self.notes.confirm_delete = None;
            }
        }
        if save {
            let text = self.notes.input.trim().to_string();
            if text.is_empty() {
                self.notes.say("便签是空的", now);
            } else if let Some(index) = self.notes.editing.filter(|index| *index < self.notes.board.items.len()) {
                let clipped: String = text.chars().take(notes::MAX_CHARS).collect();
                self.notes.board.items[index].text = clipped;
                let message = match self.notes.board.save() {
                    Err(_) => "保存失败，请检查配置目录".to_string(),
                    Ok(()) => "已保存修改".to_string(),
                };
                self.notes.stop_editing(true);
                self.notes.confirm_delete = None;
                self.notes.confirm_clear = false;
                self.notes.say(message, now);
            } else {
                self.notes.editing = None;
                let added = self.notes.board.add(&text, notes::now_unix());
                match added {
                    Added::Empty => self.notes.say("便签是空的", now),
                    Added::Saved | Added::Trimmed => {
                        self.notes.input.clear();
                        self.notes.focus_input = true;
                        self.notes.scroll_top = true;
                        self.notes.confirm_delete = None;
                        self.notes.confirm_clear = false;
                        let message = match self.notes.board.save() {
                            Err(_) => "保存失败，请检查配置目录".to_string(),
                            Ok(()) if added == Added::Trimmed => "已保存（已移除最旧一条）".to_string(),
                            Ok(()) => "已保存".to_string(),
                        };
                        self.notes.say(message, now);
                    }
                }
            }
        }
        if let Some(index) = delete {
            if self.notes.board.remove(index).is_some() {
                self.notes.editing = match self.notes.editing {
                    Some(edit) if edit == index => {
                        self.notes.input.clear();
                        self.notes.focus_input = true;
                        None
                    }
                    Some(edit) if edit > index => Some(edit - 1),
                    other => other,
                };
                let _ = self.notes.board.save();
                self.notes.confirm_delete = None;
                self.notes.confirm_clear = false;
                self.notes.say("便签已删除", now);
            }
        }
        if clear_all {
            if self.notes.confirming_clear(now) {
                if self.notes.board.clear() {
                    let _ = self.notes.board.save();
                    self.notes.say("便签已清空", now);
                }
                self.notes.editing = None;
                self.notes.confirm_clear = false;
            } else {
                self.notes.confirm_clear = true;
                self.notes.clear_at = now;
            }
        }
        if let Some(text) = copy {
            ui.ctx().copy_text(text);
            self.notes.say("已复制", now);
        }
        close
    }
}

// ---------------------------------------------------------------- 小控件

/// 玻璃皮肤的顶部柔光：顶上一条圆角光带，向下逐条带淡出。
///
/// 各条带直接和面板底色预混成不透明色再画：半透明白在 egui 里是按线性空间
/// 预乘的，直接叠加会比看起来亮一大截，把整块玻璃照成灰白。
fn top_sheen(painter: &egui::Painter, rect: egui::Rect, radius: f32, bg: Color32, strength: u8) {
    let top = rect.top();
    let radius = radius.min(rect.height() * 0.5);
    let band_color = |fade: f32| mix(bg, Color32::WHITE, strength as f32 / 255.0 * (1.0 - fade));
    // 第一段正好覆盖两个圆角的高度，圆角才不会压扁。
    painter.rect_filled(
        egui::Rect::from_min_max(egui::pos2(rect.left() + 0.5, top), egui::pos2(rect.right() - 0.5, top + radius)),
        egui::CornerRadius { nw: radius as u8, ne: radius as u8, sw: 0, se: 0 },
        band_color(0.0),
    );
    let fade = (rect.height() * 0.5).min(220.0) - radius;
    if fade <= 0.0 {
        return;
    }
    let bands = 14;
    for i in 0..bands {
        let t0 = i as f32 / bands as f32;
        let t1 = (i + 1) as f32 / bands as f32;
        let band = egui::Rect::from_min_max(
            egui::pos2(rect.left() + 0.5, top + radius + fade * t0),
            egui::pos2(rect.right() - 0.5, top + radius + fade * t1),
        );
        painter.rect_filled(band, 0.0, band_color((t0 + t1) * 0.5));
    }
}

/// 列表里的便签预览：最多两行，超出用省略号收尾。
fn note_preview(ui: &mut egui::Ui, text: &str, look: &Look) {
    let width = ui.available_width();
    let font = theme::font(BODY_SIZE, W::Regular);
    let mut job = egui::text::LayoutJob::default();
    job.break_on_newline = true;
    job.wrap.max_width = width;
    job.wrap.max_rows = PREVIEW_ROWS;
    job.append(text, 0.0, egui::TextFormat { font_id: font, color: look.ink, ..Default::default() });
    let galley = ui.fonts(|fonts| fonts.layout_job(job));
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, galley.size().y), egui::Sense::hover());
    ui.painter().galley(rect.min, galley, look.ink);
}

fn icon_button(ui: &mut egui::Ui, look: &Look, glyph: &str, hover: &str) -> bool {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::click());
    if response.hovered() {
        ui.painter().circle_filled(rect.center(), 11.0, look.veil);
    }
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        glyph,
        theme::font(13.0, W::Regular),
        if response.hovered() { look.ink } else { look.sub },
    );
    response.on_hover_text(hover).clicked()
}

/// 岛体风格的胶囊按钮：固定高度、圆角、跟随悬停提亮（浅色皮肤改为压暗）。
#[allow(clippy::too_many_arguments)]
fn pill(ui: &mut egui::Ui, look: &Look, label: &str, fill: Color32, fg: Color32, height: f32, size: f32, weight: W, enabled: bool) -> egui::Response {
    let font = theme::font(size, weight);
    let galley = ui.painter().layout_no_wrap(label.to_owned(), font, fg);
    let width = galley.size().x + (height * 0.86).max(20.0);
    let sense = if enabled { egui::Sense::click() } else { egui::Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), sense);
    let hovered = enabled && response.hovered();
    let fill = if hovered {
        let toward = if look.light || fill == look.accent { look.ink } else { Color32::WHITE };
        mix(fill, toward, 0.10)
    } else {
        fill
    };
    ui.painter().rect_filled(rect, height / 2.0, fill);
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, fg);
    if enabled {
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        response
    }
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t) as u8;
    Color32::from_rgba_unmultiplied(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()), 255)
}

fn alpha(color: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

fn empty_state(ui: &mut egui::Ui, look: &Look) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 100.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, FIELD_RADIUS, look.field);
    ui.painter().text(
        rect.center() - egui::vec2(0.0, 10.0),
        egui::Align2::CENTER_CENTER,
        "还没有便签",
        theme::font(11.5, W::Semibold),
        look.ink,
    );
    ui.painter().text(
        rect.center() + egui::vec2(0.0, 12.0),
        egui::Align2::CENTER_CENTER,
        "在上面的输入框写点什么，Ctrl+Enter 保存",
        theme::font(SMALL_SIZE, W::Regular),
        look.dim,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn luma(color: Color32) -> f32 {
        let [r, g, b, _] = color.to_srgba_unmultiplied();
        (0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32) / 255.0
    }

    fn seen(color: Color32) -> [f32; 3] {
        let [r, g, b, _] = color.to_srgba_unmultiplied();
        [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0]
    }

    fn readable(look: &Look, fg: Color32) {
        let contrast = crate::glass::contrast(seen(fg), seen(look.bg));
        assert!(contrast >= 4.5, "文字在面板上要可读：{:?} on {:?} = {contrast:.2}", fg, look.bg);
    }


    #[test]
    fn every_skin_gets_its_own_notes_palette() {
        let classic = Look::for_skin(Skin::Classic);
        assert_eq!(classic.bg, Color32::BLACK);
        assert!(!classic.light);
        assert!(luma(classic.field) > luma(classic.bg), "经典皮肤字段比面板略亮");
        assert!(luma(classic.button) > luma(classic.field), "按钮比字段再亮一点");
        readable(&classic, classic.ink);

        let cream = Look::for_skin(Skin::Cream);
        assert!(cream.light && luma(cream.bg) > 0.9, "奶油皮肤是暖白底：{:?}", cream.bg);
        assert!(luma(cream.field) < luma(cream.bg), "字段比纸色深");
        assert!(luma(cream.button) < luma(cream.field), "按钮比字段再深");
        assert!(cream.ink.r() < 80 && cream.ink.r() > cream.ink.b(), "深棕字：{:?}", cream.ink);
        assert_eq!(cream.accent, Color32::from_rgb(240, 138, 36), "强调色跟奶油皮肤");
        readable(&cream, cream.ink);
        readable(&cream, cream.sub);

        let glass_light = Look::for_skin(Skin::GlassLight);
        assert!(glass_light.light && luma(glass_light.bg) > 0.9, "毛玻璃浅是浅底：{:?}", glass_light.bg);
        assert!(glass_light.ink.r() < 60, "深色字：{:?}", glass_light.ink);
        assert_eq!(glass_light.accent, Color32::from_rgb(10, 112, 230));
        readable(&glass_light, glass_light.ink);
        readable(&glass_light, glass_light.sub);

        let glass_dark = Look::for_skin(Skin::GlassDark);
        assert!(!glass_dark.light && luma(glass_dark.bg) < 0.5, "毛玻璃深是压暗的玻璃：{:?}", glass_dark.bg);
        assert!(luma(glass_dark.field) < luma(glass_dark.bg), "字段比玻璃底深");
        assert!(glass_dark.sheen > 0, "深色玻璃要有顶部柔光");
        readable(&glass_dark, glass_dark.ink);
        readable(&glass_dark, glass_dark.sub);

        let fluid = Look::for_skin(Skin::Fluid);
        assert!(!fluid.light);
        readable(&fluid, fluid.ink);
    }
}
