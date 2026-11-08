//! 快捷翻译窗口（独立 egui viewport）与翻译设置。
//!
//! 全局快捷键（默认 Alt+C）切换窗口显隐；窗口内可输入/粘贴文本、选择译向、
//! Tab 一键填入最新剪贴板、译后一键复制。密钥缺失时在窗口内直接录入。

use crate::app::App;
use crate::translate::{self, Direction};
use eframe::egui::{self, Color32, RichText};

/// 翻译窗口的运行状态。
pub struct State {
    pub open: bool,
    pub focus_requested: bool,
    pub direction: Direction,
    pub input: String,
    pub result: String,
    pub error: String,
    pub busy: bool,
    task: Option<translate::Translate>,
    /// 密钥输入框（尚未保存时）。
    pub key_input: String,
    pub has_key: bool,
    /// 后端（OpenAI 兼容地址 + 模型），可自定义，默认 DeepSeek。
    pub endpoint: translate::Endpoint,
    /// 是否展开窗口内的「API 设置」。
    pub backend_open: bool,
    /// 正在编辑的地址 / 模型（点「保存」后生效）。
    pub url_input: String,
    pub model_input: String,
    pub endpoint_error: String,
    pub hotkey: Option<translate::Hotkey>,
    /// Tab 提示：上次填入的剪贴板预览，避免重复提示。
    pub clipboard_hint: String,
    /// 首次打开时把焦点放到输入框。
    focus_input: bool,
}

impl State {
    pub fn load() -> Self {
        let direction = crate::model::Settings::load().translate_direction;
        let has_key = cfg!(windows) && translate::has_secret();
        let endpoint = translate::Endpoint::load();
        Self {
            open: false,
            focus_requested: false,
            direction,
            input: String::new(),
            result: String::new(),
            error: String::new(),
            busy: false,
            task: None,
            key_input: String::new(),
            has_key,
            backend_open: false,
            url_input: endpoint.base_url.clone(),
            model_input: endpoint.model.clone(),
            endpoint,
            endpoint_error: String::new(),
            hotkey: None,
            clipboard_hint: String::new(),
            focus_input: false,
        }
    }
}

impl App {
    /// 每帧调用：处理热键、轮询翻译结果、渲染独立翻译窗口。
    pub fn translate_ui(&mut self, ctx: &egui::Context) {
        // 启动全局快捷键（只一次，且仅在启用时）。
        if self.settings.translate_enabled && self.translate.hotkey.is_none() {
            self.translate.hotkey = translate::Hotkey::start();
        }
        // 热键触发 → 切换显隐（仅在启用时响应）。
        if self.settings.translate_enabled && self.translate.hotkey.as_ref().is_some_and(|h| h.take()) {
            self.translate.open = !self.translate.open;
            self.translate.focus_requested = self.translate.open;
            if self.translate.open {
                self.translate.focus_input = true;
            }
        }
        if !self.settings.translate_enabled {
            self.translate.open = false;
        }
        // 轮询后台翻译任务。
        if let Some(task) = &self.translate.task {
            if let Some(result) = task.poll() {
                self.translate.task = None;
                self.translate.busy = false;
                match result {
                    Ok(text) => {
                        self.translate.result = text;
                        self.translate.error.clear();
                    }
                    Err(e) => self.translate.error = e,
                }
            }
        }

        if !self.translate.open {
            return;
        }

        let id = egui::ViewportId::from_hash_of("daozai-translate");
        let focus_requested = std::mem::take(&mut self.translate.focus_requested);
        let mut close = false;
        let mut start_task: Option<(Direction, String, translate::Endpoint)> = None;
        let mut copy_text: Option<String> = None;
        let mut fill_clipboard = false;
        let mut save_key = false;
        let mut save_endpoint = false;
        let mut direction = self.translate.direction;
        let mut input = std::mem::take(&mut self.translate.input);
        let mut key_input = std::mem::take(&mut self.translate.key_input);
        let mut url_input = self.translate.url_input.clone();
        let mut model_input = self.translate.model_input.clone();
        let mut backend_open = self.translate.backend_open;
        let has_key = self.translate.has_key;
        let endpoint = self.translate.endpoint.clone();
        let endpoint_error = self.translate.endpoint_error.clone();
        let backend_label = endpoint.label();
        let busy = self.translate.busy;
        let error = self.translate.error.clone();
        let result = self.translate.result.clone();
        let clipboard_hint = self.translate.clipboard_hint.clone();
        let clip_latest = self.clipboard.lock().items.first().map(|c| crate::clipboard::preview(&c.text)).unwrap_or_default();

        // 岛体固定深色；但子视口有自己的 Options（默认 ThemePreference::System），
        // 会把浮动窗口按系统亮暗重绘。这里显式跟随岛体，避免深色下窗口仍是浅色。
        let dark = ctx.style().visuals.dark_mode;
        ctx.show_viewport_immediate(
            id,
            egui::ViewportBuilder::default()
                .with_title("岛仔 · 快捷翻译")
                .with_icon(crate::theme::app_icon())
                .with_inner_size([420.0, 500.0])
                .with_min_inner_size([340.0, 380.0])
                .with_visible(false)
                .with_active(false)
                .with_always_on_top()
                .with_window_level(egui::WindowLevel::AlwaysOnTop),
            |ctx, _| {
                // 子视口的主题偏好与岛体保持一致（标题栏、滚动条、内建控件随之变深）。
                ctx.set_theme(if dark { egui::ThemePreference::Dark } else { egui::ThemePreference::Light });
                ctx.set_visuals(if dark { egui::Visuals::dark() } else { egui::Visuals::light() });
                ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(if dark {
                    egui::SystemTheme::Dark
                } else {
                    egui::SystemTheme::Light
                }));
                let bg = if dark { Color32::from_rgb(28, 28, 31) } else { Color32::from_rgb(246, 247, 249) };
                egui::CentralPanel::default()
                    .frame(egui::Frame::new().fill(bg).inner_margin(egui::Margin::symmetric(16, 14)))
                    .show(ctx, |ui| {
                        if focus_requested {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                        }
                        if ctx.input(|i| i.viewport().close_requested()) {
                            close = true;
                        }
                        let ink = if dark { Color32::from_rgb(230, 230, 235) } else { Color32::from_rgb(30, 34, 41) };
                        let sub = if dark { Color32::from_gray(150) } else { Color32::from_gray(105) };

                        // 标题 + 译向切换
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("快捷翻译").size(16.0).strong().color(ink));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                for dir in Direction::ALL.into_iter().rev() {
                                    let sel = direction == dir;
                                    let btn = egui::Button::new(RichText::new(dir.label()).size(12.5).color(if sel { Color32::WHITE } else { ink }))
                                        .fill(if sel { Color32::from_rgb(70, 120, 240) } else { Color32::TRANSPARENT });
                                    if ui.add(btn).clicked() {
                                        direction = dir;
                                    }
                                }
                            });
                        });
                        ui.add_space(4.0);
                        ui.label(RichText::new("Alt+C 唤起 · Tab 填入剪贴板 · Ctrl+Enter 翻译").size(11.0).color(sub));
                        ui.add_space(8.0);

                        // 后端：显示当前地址与模型，点「设置」展开自定义。
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(format!("翻译后端 · {} / {}", backend_label, endpoint.model)).size(11.0).color(sub));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.add(egui::Button::new(RichText::new(if backend_open { "收起" } else { "API 设置" }).size(11.5)).small()).clicked() {
                                    backend_open = !backend_open;
                                }
                            });
                        });

                        // 未配置密钥，或展开 API 设置时显示这张卡片。
                        if !has_key || backend_open {
                            egui::Frame::new()
                                .fill(if dark { Color32::from_rgb(48, 42, 30) } else { Color32::from_rgb(255, 247, 230) })
                                .corner_radius(8.0)
                                .inner_margin(egui::Margin::same(10))
                                .show(ui, |ui| {
                                    if !has_key {
                                        ui.label(RichText::new("填入 API Key（安全保存在 Windows 凭据管理器）").size(11.5).color(sub));
                                        ui.add_space(4.0);
                                        ui.horizontal(|ui| {
                                            let field = egui::TextEdit::singleline(&mut key_input).password(true).hint_text("sk-...").desired_width(ui.available_width() - 70.0);
                                            if ui.add(field).lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                                save_key = true;
                                            }
                                            if ui.button("保存密钥").clicked() {
                                                save_key = true;
                                            }
                                        });
                                    }
                                    if backend_open {
                                        ui.add_space(4.0);
                                        ui.label(RichText::new("API 地址（OpenAI 兼容，例如 https://api.deepseek.com/v1、https://api.openai.com/v1）").size(10.5).color(sub));
                                        ui.add(egui::TextEdit::singleline(&mut url_input).hint_text(translate::DEFAULT_BASE_URL).desired_width(f32::INFINITY));
                                        ui.add_space(2.0);
                                        ui.label(RichText::new("模型名称").size(10.5).color(sub));
                                        ui.horizontal(|ui| {
                                            ui.add(egui::TextEdit::singleline(&mut model_input).hint_text(translate::DEFAULT_MODEL).desired_width(ui.available_width() - 70.0));
                                            if ui.button("保存后端").clicked() {
                                                save_endpoint = true;
                                            }
                                        });
                                        if !endpoint_error.is_empty() {
                                            ui.label(RichText::new(&endpoint_error).size(11.0).color(Color32::from_rgb(230, 90, 90)));
                                        }
                                    }
                                });
                            ui.add_space(8.0);
                        }

                        // 输入框
                        let input_h = ((ui.available_height() - 150.0) / 2.0).max(72.0);
                        let edit = egui::TextEdit::multiline(&mut input)
                            .hint_text("输入或粘贴要翻译的文字…")
                            .desired_width(f32::INFINITY)
                            .desired_rows(4)
                            .background_color(if dark { Color32::from_rgb(38, 38, 42) } else { Color32::WHITE });
                        let resp = ui.add_sized([ui.available_width(), input_h], edit);
                        if std::mem::take(&mut self.translate.focus_input) {
                            resp.request_focus();
                        }
                        // Tab 填入剪贴板
                        if resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Tab)) {
                            fill_clipboard = true;
                        }
                        // Ctrl+Enter 翻译
                        if ui.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.command) {
                            start_task = Some((direction, input.clone(), endpoint.clone()));
                        }

                        if !clipboard_hint.is_empty() {
                            ui.label(RichText::new(format!("Tab 将填入：{}", clipboard_hint)).size(10.5).color(sub));
                        }
                        ui.add_space(4.0);

                        ui.horizontal(|ui| {
                            let can_translate = !busy && !input.trim().is_empty();
                            if ui.add_enabled(can_translate, egui::Button::new(RichText::new(if busy { "翻译中…" } else { "翻译" }).size(13.0))).clicked() {
                                start_task = Some((direction, input.clone(), endpoint.clone()));
                            }
                            if ui.button(RichText::new("填入剪贴板").size(12.5)).clicked() {
                                fill_clipboard = true;
                            }
                            if !input.is_empty() && ui.button(RichText::new("清空").size(12.5)).clicked() {
                                input.clear();
                            }
                            if !result.is_empty() {
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if ui.button(RichText::new("复制译文").size(12.5)).clicked() {
                                        copy_text = Some(result.clone());
                                    }
                                });
                            }
                        });

                        if !error.is_empty() {
                            ui.add_space(4.0);
                            ui.label(RichText::new(&error).size(12.0).color(Color32::from_rgb(230, 90, 90)));
                        }

                        // 结果区
                        ui.add_space(6.0);
                        egui::Frame::new()
                            .fill(if dark { Color32::from_rgb(38, 38, 42) } else { Color32::WHITE })
                            .corner_radius(8.0)
                            .inner_margin(egui::Margin::same(10))
                            .show(ui, |ui| {
                                ui.set_min_height((ui.available_height() - 4.0).max(40.0));
                                ui.set_width(ui.available_width());
                                if result.is_empty() {
                                    ui.label(RichText::new("译文将显示在这里").size(12.5).color(sub));
                                } else {
                                    ui.add(egui::Label::new(RichText::new(&result).size(13.5).color(ink)).wrap());
                                }
                            });
                    });
            },
        );

        // 回写状态
        self.translate.direction = direction;
        self.translate.input = input;
        self.translate.key_input = key_input;
        self.translate.backend_open = backend_open;
        if url_input != self.translate.url_input { self.translate.endpoint_error.clear(); }
        if model_input != self.translate.model_input { self.translate.endpoint_error.clear(); }
        self.translate.url_input = url_input;
        self.translate.model_input = model_input;

        if fill_clipboard && !clip_latest.is_empty() {
            self.translate.input = clip_latest.clone();
            self.translate.clipboard_hint = crate::clipboard::preview(&clip_latest);
            self.translate.focus_input = true;
        }
        if save_key {
            match translate::save_secret(&self.translate.key_input.clone()) {
                Ok(()) => {
                    self.translate.has_key = true;
                    self.translate.key_input.clear();
                    self.translate.error.clear();
                }
                Err(e) => self.translate.error = e,
            }
        }
        if save_endpoint {
            match translate::validate_endpoint(&self.translate.url_input.clone(), &self.translate.model_input.clone()) {
                Ok(parsed) => {
                    let previous = self.translate.endpoint.clone();
                    self.translate.endpoint = parsed.clone();
                    self.translate.url_input = parsed.base_url.clone();
                    self.translate.model_input = parsed.model.clone();
                    self.translate.endpoint_error.clear();
                    if parsed != previous {
                        // 换了后端，上一次的译文不再对应当前设置。
                        self.translate.result.clear();
                        self.translate.error.clear();
                        parsed.save();
                    }
                    self.translate.backend_open = false;
                }
                Err(e) => self.translate.endpoint_error = e,
            }
        }
        if let Some((dir, text, endpoint)) = start_task {
            self.translate.error.clear();
            self.translate.busy = true;
            self.translate.task = Some(translate::Translate::start_with(dir, text, endpoint));
            // 记住方向
            self.settings.translate_direction = dir;
            self.settings.save();
        }
        if let Some(text) = copy_text {
            ctx.copy_text(text);
        }
        if close {
            self.translate.open = false;
        }
        if self.translate.open || self.translate.busy {
            ctx.request_repaint();
        }
    }

    /// 翻译设置的入口按钮（放在「常规」设置页里调用）。
    pub fn translate_settings_button(&mut self, ui: &mut egui::Ui) {
        const KEY_ID: &str = "translate-key-draft";
        let mut key_draft = ui.data(|data| data.get_temp::<String>(egui::Id::new(KEY_ID))).unwrap_or_default();
        let endpoint = self.translate.endpoint.clone();
        let has = self.translate.has_key;
        let mut saved = false;
        let mut open_window = false;
        ui.vertical(|ui| {
            ui.label(RichText::new(format!("后端 · {} / {}", endpoint.label(), endpoint.model)).size(12.0));
            egui::Grid::new("translate-backend").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                ui.label(RichText::new("API 地址").size(12.0).color(Color32::from_gray(140)));
                ui.add(egui::TextEdit::singleline(&mut self.translate.url_input).hint_text(translate::DEFAULT_BASE_URL).desired_width(f32::INFINITY));
                ui.end_row();
                ui.label(RichText::new("模型").size(12.0).color(Color32::from_gray(140)));
                ui.add(egui::TextEdit::singleline(&mut self.translate.model_input).hint_text(translate::DEFAULT_MODEL).desired_width(f32::INFINITY));
                ui.end_row();
                ui.label(RichText::new("API Key").size(12.0).color(Color32::from_gray(140)));
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut key_draft).password(true).hint_text(if has { "已保存，输入可替换" } else { "sk-..." }).desired_width(ui.available_width() - 70.0));
                    if ui.button("保存").clicked() {
                        saved = true;
                    }
                });
                ui.end_row();
            });
            if !self.translate.endpoint_error.is_empty() {
                ui.label(RichText::new(&self.translate.endpoint_error).size(11.5).color(Color32::from_rgb(230, 90, 90)));
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let changed = self.translate.url_input.trim() != endpoint.base_url || self.translate.model_input.trim() != endpoint.model;
                if ui.add_enabled(changed, egui::Button::new("保存后端")).clicked() {
                    match translate::validate_endpoint(&self.translate.url_input.clone(), &self.translate.model_input.clone()) {
                        Ok(parsed) => {
                            self.translate.endpoint = parsed.clone();
                            self.translate.url_input = parsed.base_url.clone();
                            self.translate.model_input = parsed.model.clone();
                            self.translate.endpoint_error.clear();
                            parsed.save();
                        }
                        Err(error) => self.translate.endpoint_error = error,
                    }
                }
                if has && ui.button("清除密钥").clicked() {
                    translate::forget_secret();
                    self.translate.has_key = false;
                }
                if ui.button("打开翻译窗口").clicked() {
                    open_window = true;
                }
            });
            if !has {
                ui.label(RichText::new("未配置 API Key 时会尝试沿用已连接的 DeepSeek 额度账号密钥。").size(11.5).color(Color32::from_gray(120)));
            }
        });
        if saved {
            match translate::save_secret(&key_draft) {
                Ok(()) => {
                    self.translate.has_key = true;
                    key_draft.clear();
                    self.translate.endpoint_error.clear();
                }
                Err(error) => self.translate.endpoint_error = error,
            }
        }
        ui.data_mut(|data| data.insert_temp(egui::Id::new(KEY_ID), key_draft));
        if open_window {
            self.translate.open = true;
            self.translate.focus_requested = true;
            self.translate.focus_input = true;
        }
    }
}
