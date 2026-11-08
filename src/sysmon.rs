//! 系统监控面板：CPU / 内存 / GPU 实时读数；温度 / 功耗可选且不可读时静默跳过。
//!
//! 遵循 BrimDeck 的 `Reading<T>` 原则：读数要么有值，要么“不可用（原因）”，
//! **永不伪造 0**；不可读项显示 `—`。
//!
//! 普通用户无需管理员即可读取 CPU / 内存；GPU 走系统性能计数器。
//! 温度、功耗依赖更深的系统接口，统一返回 `Unknown`（不影响其它读数，也不阻塞）。

use std::sync::mpsc;
use std::time::Duration;

/// 一项读数：已知值或不可用原因。
#[derive(Clone, Debug, PartialEq)]
pub enum Reading {
    Known(f64),
    Unknown(&'static str),
}
impl Reading {
    pub fn value(&self) -> Option<f64> {
        match self {
            Reading::Known(v) => Some(*v),
            Reading::Unknown(_) => None,
        }
    }
    pub fn pct_text(&self) -> String {
        match self {
            Reading::Known(v) => format!("{:.0}%", v),
            Reading::Unknown(_) => "—".into(),
        }
    }
}

/// 一份系统快照。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub cpu: Reading,
    pub mem_ratio: Reading,
    pub mem_used: u64,
    pub mem_total: u64,
    pub gpu: Reading,
    pub cpu_temp: Reading,
    pub gpu_temp: Reading,
    pub power: Reading,
    pub version: u64,
}
impl Default for Reading {
    fn default() -> Self {
        Reading::Unknown("尚未读取")
    }
}
/// 收起态可显示的读数（顺序固定：CPU、内存、GPU，与 `Settings::sysmon_metrics` 一致）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Metric {
    Cpu,
    Mem,
    Gpu,
}
impl Metric {
    pub const ALL: [Self; 3] = [Self::Cpu, Self::Mem, Self::Gpu];
    /// 设置文件里用的键。
    pub fn key(self) -> &'static str {
        match self { Self::Cpu => "c", Self::Mem => "m", Self::Gpu => "g" }
    }
    /// 圆环里的单字母。
    pub fn short(self) -> &'static str {
        match self { Self::Cpu => "C", Self::Mem => "M", Self::Gpu => "G" }
    }
    /// 设置里与提示里的名字。
    pub fn label(self) -> &'static str {
        match self { Self::Cpu => "CPU", Self::Mem => "内存", Self::Gpu => "GPU" }
    }
    pub fn ring_color(self) -> eframe::egui::Color32 {
        use eframe::egui::Color32;
        match self { Self::Cpu => Color32::from_rgb(40, 130, 255), Self::Mem => Color32::from_rgb(0, 200, 190), Self::Gpu => Color32::from_rgb(200, 110, 220) }
    }
    /// 该读数在当前快照里的值。
    pub fn reading(self, snap: &Snapshot) -> &Reading {
        match self { Self::Cpu => &snap.cpu, Self::Mem => &snap.mem_ratio, Self::Gpu => &snap.gpu }
    }
    /// 在设置里的顺序下标。
    pub fn index(self) -> usize {
        match self { Self::Cpu => 0, Self::Mem => 1, Self::Gpu => 2 }
    }
}
/// 设置文件里 `sysmon_metrics` 的键（顺序同 `Metric::ALL`）。
pub const METRICS: [&str; 3] = ["c", "m", "g"];

impl Snapshot {
    /// 收起态 HUD 文本：`C 12% M 48%`（仅显示 `shown` 里勾选且已知的项）。
    pub fn hud_text(&self) -> Option<String> {
        self.hud_text_for([true; 3])
    }
    /// `shown` 按 `Metric::ALL` 的顺序表示要显示哪几项。
    pub fn hud_text_for(&self, shown: [bool; 3]) -> Option<String> {
        let parts: Vec<String> = Metric::ALL
            .into_iter()
            .filter(|metric| shown[metric.index()])
            .filter_map(|metric| metric.reading(self).value().map(|v| format!("{} {:.0}%", metric.short(), v)))
            .collect();
        if parts.is_empty() {
            None
        } else {
            Some(parts.join("  "))
        }
    }
    /// `shown` 里至少有一项能读到值。
    pub fn any_known(&self, shown: [bool; 3]) -> bool {
        Metric::ALL.into_iter().filter(|metric| shown[metric.index()]).any(|metric| metric.reading(self).value().is_some())
    }
}

/// 内存读数：已用比例、已用、总量（字节）。纯逻辑，便于测试。
pub fn mem_reading(total: u64, avail: u64) -> (Reading, u64, u64) {
    if total == 0 {
        return (Reading::Unknown("无内存信息"), 0, 0);
    }
    let used = total.saturating_sub(avail);
    let ratio = used as f64 / total as f64 * 100.0;
    (Reading::Known(ratio.clamp(0.0, 100.0)), used, total)
}

/// 由两次 GetSystemTimes 的 (idle, kernel, user) 累计值计算 CPU 使用率百分比。
/// kernel 已包含 idle。纯逻辑。
pub fn cpu_usage(prev: (u64, u64, u64), cur: (u64, u64, u64)) -> Reading {
    let d_idle = cur.0.saturating_sub(prev.0);
    let d_kernel = cur.1.saturating_sub(prev.1);
    let d_user = cur.2.saturating_sub(prev.2);
    let d_total = d_kernel + d_user; // kernel 含 idle
    if d_total == 0 {
        return Reading::Unknown("无 CPU 采样");
    }
    let busy = d_total.saturating_sub(d_idle);
    Reading::Known((busy as f64 / d_total as f64 * 100.0).clamp(0.0, 100.0))
}

/// 格式化字节：如 "12.3 GB"。
pub fn format_bytes(b: u64) -> String {
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    let f = b as f64;
    if f >= GB {
        format!("{:.1} GB", f / GB)
    } else if f >= MB {
        format!("{:.0} MB", f / MB)
    } else {
        format!("{b} B")
    }
}

/// 后台采样服务：1.5 秒一次。
pub struct Service {
    rx: mpsc::Receiver<Snapshot>,
    _stop: mpsc::Sender<()>,
}
impl Service {
    pub fn start(repaint: impl Fn() + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel();
        let (stop, stopped) = mpsc::channel::<()>();
        std::thread::Builder::new()
            .name("daozai-sysmon".into())
            .spawn(move || {
                let mut sampler = Sampler::new();
                let mut version = 0u64;
                loop {
                    if stopped.try_recv().is_ok() {
                        break;
                    }
                    let mut snap = sampler.sample();
                    version += 1;
                    snap.version = version;
                    let _ = tx.send(snap);
                    repaint();
                    for _ in 0..15 {
                        if stopped.try_recv().is_ok() {
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
            })
            .ok();
        Self { rx, _stop: stop }
    }
    pub fn try_recv(&self) -> Option<Snapshot> {
        self.rx.try_recv().ok()
    }
}

/// 采样器：保存上一次 CPU 时间以计算增量，以及 GPU 的 PDH 查询。
pub struct Sampler {
    last_cpu: Option<(u64, u64, u64)>,
    #[cfg(windows)]
    gpu: Option<GpuQuery>,
    #[cfg(not(windows))]
    gpu: Option<()>,
}
impl Sampler {
    pub fn new() -> Self {
        Self { last_cpu: None, gpu: None }
    }
    pub fn sample(&mut self) -> Snapshot {
        let mut snap = Snapshot::default();
        // CPU
        if let Some(cur) = read_system_times() {
            if let Some(prev) = self.last_cpu {
                snap.cpu = cpu_usage(prev, cur);
            } else {
                snap.cpu = Reading::Unknown("采样中");
            }
            self.last_cpu = Some(cur);
        } else {
            snap.cpu = Reading::Unknown("无法读取 CPU");
        }
        // 内存
        if let Some((total, avail)) = read_memory() {
            let (r, used, tot) = mem_reading(total, avail);
            snap.mem_ratio = r;
            snap.mem_used = used;
            snap.mem_total = tot;
        } else {
            snap.mem_ratio = Reading::Unknown("无法读取内存");
        }
        // GPU：PDH \GPU Engine(*)\Utilization Percentage（不可用即 Unknown）。
        #[cfg(windows)]
        {
            if self.gpu.is_none() {
                self.gpu = GpuQuery::open();
            }
            snap.gpu = self.gpu.as_mut().map(|q| q.sample()).unwrap_or(Reading::Unknown("无 GPU 计数器"));
        }
        #[cfg(not(windows))]
        {
            snap.gpu = Reading::Unknown("非 Windows");
        }
        // 温度 / 功耗：需要更深层的驱动接口，暂不采集。
        snap.cpu_temp = Reading::Unknown("未采集");
        snap.gpu_temp = Reading::Unknown("未采集");
        snap.power = Reading::Unknown("未采集");
        snap
    }
}
impl Default for Sampler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(windows)]
fn read_system_times() -> Option<(u64, u64, u64)> {
    use windows::Win32::System::Threading::GetSystemTimes;
    unsafe {
        let mut idle = windows::Win32::Foundation::FILETIME::default();
        let mut kernel = windows::Win32::Foundation::FILETIME::default();
        let mut user = windows::Win32::Foundation::FILETIME::default();
        GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)).ok()?;
        let to_u64 = |ft: &windows::Win32::Foundation::FILETIME| ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64;
        Some((to_u64(&idle), to_u64(&kernel), to_u64(&user)))
    }
}
#[cfg(not(windows))]
fn read_system_times() -> Option<(u64, u64, u64)> {
    None
}

#[cfg(windows)]
fn read_memory() -> Option<(u64, u64)> {
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    unsafe {
        let mut m = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
        GlobalMemoryStatusEx(&mut m).ok()?;
        Some((m.ullTotalPhys, m.ullAvailPhys))
    }
}
#[cfg(not(windows))]
fn read_memory() -> Option<(u64, u64)> {
    None
}

/// GPU 使用率：PDH `\GPU Engine(*)\Utilization Percentage`。
/// 每个引擎计数器之和即整体占用（任务管理器口径为跨引擎求和后再归一）。
#[cfg(windows)]
struct GpuQuery {
    query: isize,
    counters: Vec<isize>,
    /// 已经采集过一次（PDH 需要两次采样才能取值）。
    primed: bool,
}
#[cfg(windows)]
impl GpuQuery {
    fn open() -> Option<Self> {
        use windows::Win32::System::Performance::{PdhAddEnglishCounterW, PdhOpenQueryW};
        unsafe {
            let mut query: isize = 0;
            if PdhOpenQueryW(windows::core::PCWSTR::null(), 0, &mut query) != 0 {
                return None;
            }
            // 通配计数器：读取时用 PdhGetFormattedCounterArrayW 展开为多实例数组。
            let mut counters = Vec::new();
            let mut counter: isize = 0;
            let name = windows::core::HSTRING::from("\\GPU Engine(*)\\Utilization Percentage");
            if PdhAddEnglishCounterW(query, windows::core::PCWSTR(name.as_ptr()), 0, &mut counter) == 0 {
                counters.push(counter);
            }
            if counters.is_empty() {
                let _ = windows::Win32::System::Performance::PdhCloseQuery(query);
                return None;
            }
            Some(Self { query, counters, primed: false })
        }
    }

    fn sample(&mut self) -> Reading {
        use windows::Win32::System::Performance::*;
        unsafe {
            // 第一次只收集数据，第二次才能算出百分比。
            if PdhCollectQueryData(self.query) != 0 {
                return Reading::Unknown("GPU 采样失败");
            }
            if !self.primed {
                self.primed = true;
                return Reading::Unknown("采样中");
            }
            let mut total = 0.0f64;
            let mut any = false;
            for counter in &self.counters {
                let mut size = 0u32;
                let mut count = 0u32;
                // 先问大小。
                let _ = PdhGetFormattedCounterArrayW(*counter, PDH_FMT_DOUBLE, &mut size, &mut count, None);
                if size == 0 || count == 0 {
                    continue;
                }
                // 用 u64 对齐缓冲，避免 PDH 写入未对齐指针。
                let slots = (size as usize + 7) / 8;
                let mut buf = vec![0u64; slots];
                let items = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
                if PdhGetFormattedCounterArrayW(*counter, PDH_FMT_DOUBLE, &mut size, &mut count, Some(items)) != 0 {
                    continue;
                }
                for i in 0..count as usize {
                    let item = &*items.add(i);
                    let v = item.FmtValue.Anonymous.doubleValue;
                    if v.is_finite() && v >= 0.0 {
                        total += v;
                        any = true;
                    }
                }
            }
            if !any {
                return Reading::Unknown("无 GPU 数据");
            }
            Reading::Known(total.clamp(0.0, 100.0))
        }
    }
}
#[cfg(windows)]
impl Drop for GpuQuery {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::System::Performance::PdhCloseQuery(self.query);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mem_reading_computes_ratio() {
        let (r, used, total) = mem_reading(1000, 250);
        assert_eq!(used, 750);
        assert_eq!(total, 1000);
        assert_eq!(r.pct_text(), "75%");
        // 无内存信息 → Unknown，绝不为 0。
        let (r2, _, _) = mem_reading(0, 0);
        assert!(matches!(r2, Reading::Unknown(_)));
        assert_eq!(r2.pct_text(), "—");
    }

    #[test]
    fn cpu_usage_math() {
        // prev idle=100 kernel=200 user=100; cur idle=150 kernel=300 user=150
        // d_idle=50, d_kernel=100, d_user=50, d_total=150, busy=100 → 66.7%
        let r = cpu_usage((100, 200, 100), (150, 300, 150));
        assert!((r.value().unwrap() - 66.666).abs() < 0.1);
        // 无增量 → Unknown
        assert!(matches!(cpu_usage((1, 1, 1), (1, 1, 1)), Reading::Unknown(_)));
    }

    #[test]
    fn hud_text_only_known() {
        let mut snap = Snapshot::default();
        assert!(snap.hud_text().is_none());
        snap.cpu = Reading::Known(12.0);
        snap.mem_ratio = Reading::Known(48.0);
        assert_eq!(snap.hud_text().unwrap(), "C 12%  M 48%");
    }

    #[test]
    fn hud_text_follows_selected_metrics() {
        let mut snap = Snapshot::default();
        snap.cpu = Reading::Known(12.0);
        snap.mem_ratio = Reading::Known(48.0);
        snap.gpu = Reading::Known(73.0);
        assert_eq!(snap.hud_text_for([true, false, false]).unwrap(), "C 12%");
        assert_eq!(snap.hud_text_for([false, true, true]).unwrap(), "M 48%  G 73%");
        assert_eq!(snap.hud_text_for([true, true, true]).unwrap(), "C 12%  M 48%  G 73%");
        // 一个都没勾 / 勾了但都不可读：不显示。
        assert!(snap.hud_text_for([false; 3]).is_none());
        snap.cpu = Reading::Unknown("x");
        assert!(snap.hud_text_for([true, false, false]).is_none());
        assert!(snap.any_known([false, true, false]));
        assert!(!snap.any_known([true, false, false]));
    }

    #[test]
    fn metric_keys_match_settings_order() {
        assert_eq!(Metric::ALL.map(|m| m.key()), METRICS);
        assert_eq!(Metric::ALL.map(|m| m.index()), [0, 1, 2]);
    }

    #[test]
    fn format_bytes_units() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(2 * 1024 * 1024), "2 MB");
        assert!(format_bytes(3 * 1024 * 1024 * 1024).contains("GB"));
    }

    #[test]
    fn sampler_first_sample_reads_memory() {
        let mut s = Sampler::new();
        let snap = s.sample();
        // 内存应可读到（Windows/非 Windows 都必须不 panic；读到则 total>0）。
        if let Some(_v) = snap.mem_ratio.value() {
            assert!(snap.mem_total > 0);
        }
    }
}
