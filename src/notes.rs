//! 便签：Alt+C 唤起的便签窗口数据与全局快捷键。
//!
//! - 便签保存在本机 "notes.json"（文本 + Unix 秒），不联网。
//! - 快捷键在 Windows 上走 "RegisterHotKey" + 专用消息窗口线程；其它平台只保留窗口入口。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// 最多保留的便签条数；超出时移除最旧的一条。
pub const MAX_NOTES: usize = 100;
/// 单条便签的字数上限。
pub const MAX_CHARS: usize = 2000;
const FILE: &str = "notes.json";

/// 一条便签。`at` 是保存时的 Unix 秒，显示时再按今天/更早格式化。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Note {
    pub text: String,
    pub at: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Notes {
    /// 最新的在前。
    pub items: Vec<Note>,
}

/// 添加便签的结果：空文本被拒绝；写满时移除最旧的一条。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Added {
    Saved,
    Trimmed,
    Empty,
}

fn path() -> std::path::PathBuf {
    crate::model::config_dir().join(FILE)
}

impl Notes {
    pub fn load() -> Self {
        let Ok(raw) = std::fs::read(path()) else { return Self::default() };
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&raw) else { return Self::default() };
        let items = value
            .get("notes")
            .and_then(|notes| notes.as_array())
            .map(|list| {
                list.iter()
                    .filter_map(|item| {
                        let text = item.get("text")?.as_str()?.trim();
                        if text.is_empty() {
                            return None;
                        }
                        let text: String = text.chars().take(MAX_CHARS).collect();
                        let at = item.get("at").and_then(|at| at.as_i64()).unwrap_or(0);
                        Some(Note { text, at })
                    })
                    .take(MAX_NOTES)
                    .collect()
            })
            .unwrap_or_default();
        Self { items }
    }

    pub fn save(&self) -> Result<(), String> {
        let dir = crate::model::config_dir();
        std::fs::create_dir_all(&dir).map_err(|e| format!("无法创建配置目录：{e}"))?;
        let notes: Vec<_> = self.items.iter().map(|note| serde_json::json!({"text": note.text, "at": note.at})).collect();
        let body = serde_json::json!({ "notes": notes });
        crate::model::atomic_write(&dir.join(FILE), body.to_string().as_bytes()).map_err(|e| format!("无法保存便签：{e}"))
    }

    /// 添加一条便签；空白文本不保存。
    pub fn add(&mut self, text: &str, at: i64) -> Added {
        let text = text.trim();
        if text.is_empty() {
            return Added::Empty;
        }
        let text: String = text.chars().take(MAX_CHARS).collect();
        self.items.insert(0, Note { text, at });
        let trimmed = self.items.len() > MAX_NOTES;
        if trimmed {
            self.items.truncate(MAX_NOTES);
        }
        if trimmed { Added::Trimmed } else { Added::Saved }
    }

    pub fn remove(&mut self, index: usize) -> Option<Note> {
        (index < self.items.len()).then(|| self.items.remove(index))
    }

    pub fn clear(&mut self) -> bool {
        let had = !self.items.is_empty();
        self.items.clear();
        had
    }
}

/// 便签时间：当天只写时刻，更早的带上日期（跨天后再看不会分不清是哪天）。
pub fn time_label(at: i64, now: chrono::DateTime<chrono::Local>) -> String {
    let Some(saved) = chrono::DateTime::from_timestamp(at, 0).map(|at| at.with_timezone(&chrono::Local)) else {
        return String::new();
    };
    if saved.date_naive() == now.date_naive() {
        saved.format("%H:%M").to_string()
    } else {
        saved.format("%m-%d %H:%M").to_string()
    }
}

/// 当前时间戳，供 UI 保存便签时使用。
pub fn now_unix() -> i64 {
    chrono::Local::now().timestamp()
}

// ---------------------------------------------------------------- 全局快捷键
/// 快捷键是否已触发（UI 每帧检查，true 表示要切换窗口显隐）。
pub struct Hotkey {
    pub toggle: Arc<AtomicBool>,
}

impl Hotkey {
    /// 启动全局快捷键线程；注册失败时返回 None（便签窗口仍可从设置打开）。
    pub fn start() -> Option<Self> {
        let toggle = Arc::new(AtomicBool::new(false));
        #[cfg(windows)]
        {
            let t = toggle.clone();
            std::thread::Builder::new()
                .name("daozai-notes-hotkey".into())
                .spawn(move || hotkey_loop(t))
                .ok()?;
        }
        #[cfg(not(windows))]
        let _ = toggle;
        Some(Self { toggle })
    }

    /// 取出并清除触发标志。
    pub fn take(&self) -> bool {
        self.toggle.swap(false, Ordering::Relaxed)
    }
}

#[cfg(windows)]
fn hotkey_loop(toggle: Arc<AtomicBool>) {
    use windows::core::w;
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_ALT, MOD_NOREPEAT};
    use windows::Win32::UI::WindowsAndMessaging::*;

    const HOTKEY_ID: i32 = 0x4E54; // 'NT'
    const WM_HOTKEY: u32 = 0x0312;

    unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        if msg == WM_HOTKEY && wp.0 as i32 == HOTKEY_ID {
            if let Some(flag) = TOGGLE.get() {
                flag.store(true, Ordering::Relaxed);
            }
            return LRESULT(0);
        }
        DefWindowProcW(hwnd, msg, wp, lp)
    }
    use std::sync::OnceLock;
    static TOGGLE: OnceLock<Arc<AtomicBool>> = OnceLock::new();
    let _ = TOGGLE.set(toggle);

    unsafe {
        let Ok(instance) = GetModuleHandleW(None) else { return };
        let class = w!("DaozaiNotesHotkey");
        let wc = WNDCLASSW { lpfnWndProc: Some(proc), hInstance: instance.into(), lpszClassName: class, ..Default::default() };
        RegisterClassW(&wc);
        let Ok(hwnd) = CreateWindowExW(WINDOW_EX_STYLE(0), class, windows::core::PCWSTR::null(), WINDOW_STYLE(0), 0, 0, 0, 0, HWND_MESSAGE, None, instance, None) else { return };
        // Alt+C (VK_C = 0x43)
        if RegisterHotKey(hwnd, HOTKEY_ID, HOT_KEY_MODIFIERS(MOD_ALT.0 | MOD_NOREPEAT.0), 0x43).is_err() {
            // 被其它程序占用时静默退出，窗口仍可从设置打开。
            return;
        }
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let _ = UnregisterHotKey(hwnd, HOTKEY_ID);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ENV_LOCK;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("daozai-notes-{name}-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn notes_add_remove_clear_and_reload() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let dir = temp_dir("roundtrip");
        std::env::set_var("DAOZAI_DATA_DIR", &dir);
        let mut notes = Notes::default();
        assert_eq!(notes.add("  买牛奶  ", 100), Added::Saved);
        assert_eq!(notes.items[0].text, "买牛奶");
        assert_eq!(notes.add("第二行\n第二条", 200), Added::Saved);
        assert_eq!(notes.items.len(), 2);
        assert_eq!(notes.items[0].text, "第二行\n第二条", "最新的在前");
        assert_eq!(notes.add("   ", 300), Added::Empty);
        notes.save().unwrap();
        let loaded = Notes::load();
        assert_eq!(loaded, notes);
        assert_eq!(notes.remove(0).unwrap().text, "第二行\n第二条");
        assert!(notes.remove(9).is_none());
        assert!(notes.clear());
        assert!(!notes.clear());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn notes_trim_to_the_cap_and_limit_single_note_length() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let dir = temp_dir("cap");
        std::env::set_var("DAOZAI_DATA_DIR", &dir);
        let mut notes = Notes::default();
        for i in 0..MAX_NOTES {
            assert_eq!(notes.add(&format!("便签 {i}"), i as i64), Added::Saved);
        }
        assert_eq!(notes.add("溢出的最新一条", 999), Added::Trimmed);
        assert_eq!(notes.items.len(), MAX_NOTES);
        assert_eq!(notes.items[0].text, "溢出的最新一条");
        assert!(!notes.items.iter().any(|note| note.text == "便签 0"), "写满后移除最旧的一条");
        notes.add(&"长".repeat(MAX_CHARS + 20), 1000);
        assert_eq!(notes.items[0].text.chars().count(), MAX_CHARS);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn empty_or_broken_file_loads_as_empty_notes() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let dir = temp_dir("broken");
        std::env::set_var("DAOZAI_DATA_DIR", &dir);
        std::fs::write(dir.join(FILE), b"not json").unwrap();
        assert!(Notes::load().items.is_empty());
        std::fs::write(dir.join(FILE), r#"{"notes":[{"text":"  ","at":1},{"text":"有效","at":2}]}"#.as_bytes()).unwrap();
        let loaded = Notes::load();
        assert_eq!(loaded.items.len(), 1);
        assert_eq!(loaded.items[0].text, "有效");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn time_label_switches_between_today_and_older_notes() {
        let now = chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap().with_timezone(&chrono::Local);
        let today = now.timestamp();
        let older = today - 86_400 * 2;
        assert_eq!(time_label(today, now), now.format("%H:%M").to_string());
        let old = chrono::DateTime::from_timestamp(older, 0).unwrap().with_timezone(&chrono::Local);
        assert_eq!(time_label(older, now), old.format("%m-%d %H:%M").to_string());
        assert!(time_label(i64::MAX, now).is_empty());
    }
}
