//! 任务栏闪动监听：微信、钉钉共用一个外壳钩子窗口。
//!
//! 只读 Windows 外壳报告的事件（HSHELL_FLASH / HSHELL_RUDEAPPACTIVATED），
//! 不读取聊天正文、不注入、不修改对方程序；只看得到「哪个进程的窗口在闪」。
//!
//! 去抖沿用 v0.6 起验证过的规则，并按来源各自合并：
//! - 连续闪动只提醒一次，停止闪动五秒后的下一轮才算新提醒；
//! - 用户刚看过某个账号再切走，4 秒内的补闪算这一次提醒的余波；
//! - 用户正看着某个账号时，它自己的提醒直接作废。

use std::cell::RefCell;
use std::time::{Duration, Instant};
use windows::{core::{w, PCWSTR}, Win32::{
    Foundation::{CloseHandle, HWND, LPARAM, LRESULT, WPARAM},
    System::Registry::{RegGetValueW, HKEY, HKEY_CLASSES_ROOT, RRF_RT_REG_SZ},
    UI::{Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass}, WindowsAndMessaging::*},
}};

const SUBCLASS_ID: usize = 0x5745_4348;
const FLASH: usize = 0x8006;

/// 看过某个账号后切走：这段时间内的闪动算作刚才那次提醒的余波。
const LEAVE_GRACE: Duration = Duration::from_secs(4);
/// 刚进入某个账号的窗口（前台激活）之后：紧随其后的补闪只合并，不当成新消息。
const ENTER_GRACE: Duration = Duration::from_secs(2);

#[derive(Default)]
struct Burst { last: Option<Instant>, quiet_until: Option<Instant> }
impl Burst {
    fn attention(&mut self, now: Instant) -> bool {
        // 一轮闪动的第一下就是一条新消息：立刻提醒，不再等「安静几秒」。
        // 只有「刚看完切走」的宽限期（LEAVE_GRACE）内，才把它当作刚才那次提醒的余波。
        // 同一轮里后续的闪动（半秒一下）由 idle_flash_ended 判定为合并。
        let tail=self.quiet_until.is_some_and(|until| now < until);
        self.last=Some(now);
        !tail
    }
    /// 用户刚从这个账号的窗口切走。
    fn left(now: Instant) -> Self { Self{last:Some(now),quiet_until:Some(now+LEAVE_GRACE)} }
}

/// 收到一次外壳事件后的结论，由描述文案与是否重绘使用。
#[derive(Debug,PartialEq,Eq)]
pub enum Heard { Alert, Merged, Viewing, Ended, Ignored }

/// 一个来源（微信 / 钉钉）的合并与待取提醒。纯逻辑，可脱离 Windows 单测。
pub struct Source {
    pub source: crate::app::Messenger,
    /// 每个账号（进程）各自合并连续闪动。
    burst: Vec<(u32,Burst)>,
    /// 尚未取走的提醒：账号进程与闪动的窗口（isize::MAX 表示用户已点开、待清除）。
    pending: Vec<(u32,isize)>,
    /// 当前正在前台查看的账号进程。
    viewing: Vec<u32>,
    /// 上一次记录的前台账号（用于判断「谁切走了」）。
    last_foreground: Option<u32>,
    /// 刚进入前台的账号 → 合并补闪的截止时间。
    enter_quiet: std::collections::HashMap<u32,Instant>,
    /// 最近一次闪动开始、还没等到结束事件的账号进程。
    active_flash: Option<u32>,
    /// 上一帧闪动的时间（判断下一帧是否属于同一轮）。
    last_frame: Option<Instant>,
}
impl Source {
    pub fn new(source:crate::app::Messenger)->Self { Self{source,burst:Vec::new(),pending:Vec::new(),viewing:Vec::new(),last_foreground:None,enter_quiet:Default::default(),active_flash:None,last_frame:None} }
    /// 取走新提醒：每项是（账号进程，闪动的窗口）。
    pub fn take(&mut self) -> Vec<(u32,isize)> { std::mem::take(&mut self.pending) }
    pub fn pending_len(&self)->usize { self.pending.len() }
    /// 用户正在前台看这个账号：之后它自己的闪动不算新提醒；其它账号不受影响。
    ///
    /// 注意只清「闪动合并状态」，不清已经待取的未读——清未读是 main 里「真的切到了窗口」那一步的事，
    /// 否则一次路径不可靠的前台判定就会把刚到的提醒抹掉。
    pub fn enter_view(&mut self,pid:u32){
        self.burst.retain(|(p,_)|*p!=pid);
        // 刚进前台后的几秒可能是「用户追着消息点进来」留下的补闪：只合并、不提醒。
        self.enter_quiet.insert(pid,Instant::now()+ENTER_GRACE);
        if !self.viewing.contains(&pid){self.viewing.push(pid);}
    }
    /// 已经切走的账号：开始宽限期。FOREGROUND 是此刻仍在前台的账号进程。
    pub fn leave_view(&mut self,foreground:Option<u32>,now:Instant){
        let left:Vec<u32>=self.viewing.iter().copied().filter(|p|Some(*p)!=foreground).collect();
        for pid in left {
            self.viewing.retain(|p|*p!=pid);
            self.burst.retain(|(p,_)|*p!=pid);
            self.burst.push((pid,Burst::left(now)));
        }
    }
    /// 距上一帧闪动超过这么久，就认为上一轮闪动已经结束（用户没点、任务栏自己停了）。
    /// 连续闪动是半秒一下，所以 1 秒足够区分「同一轮」和「下一条消息」。
    const BURST_IDLE: Duration = Duration::from_secs(1);
    /// 收到一次闪动：返回是否算新提醒（已写入待取列表）。
    pub fn flash(&mut self,pid:u32,window:isize,now:Instant)->bool{
        let fresh={
            if !self.burst.iter().any(|(p,_)|*p==pid){self.burst.push((pid,Burst::default()));}
            // 刚进前台（用户正追着点开）的补闪：只合并。
            let entering=self.enter_quiet.get(&pid).is_some_and(|until|now<*until);
            self.burst.iter_mut().find(|(p,_)|*p==pid).is_some_and(|(_,burst)|{
                if entering {burst.last=Some(now);return false;}
                // 与上一帧隔得够久（> BURST_IDLE）就当作新的一条消息；否则是同一轮的连续闪动。
                let new_burst=burst.last.is_none_or(|last|now.duration_since(last)>=Source::BURST_IDLE);
                if new_burst {burst.last=Some(now);burst.attention(now)} else {burst.last=Some(now);false}
            })
        };
        if fresh {
            match self.pending.iter_mut().find(|(p,_)|*p==pid){Some(entry)=>entry.1=window,None=>self.pending.push((pid,window))}
        }
        fresh
    }
    /// 外壳报告的激活事件：只用来判断「谁进了前台 / 谁切走了」。
    ///
    /// 只有「上一次看着它、这一次不看了」才算切走。此前没人进前台（None）时记录到「看到它」，
    /// 否则用户从桌面 Alt+Tab 到客户端，会立刻被当成「刚切走」而跳过这次提醒。
    pub fn foreground(&mut self,foreground:Option<u32>,now:Instant){
        let previous=self.last_foreground;
        if let Some(pid)=foreground {self.enter_view(pid);}
        // 看着客户端时它自己发闪动：这不算切走，继续当作「正在看」。
        if previous!=foreground && !(foreground.is_some() && self.viewing.contains(&foreground.unwrap())) {self.leave_view(foreground,now);}
        self.last_foreground=foreground;
    }
    /// 还有没结束的闪动（只用来给「闪动结束」事件找归属）。
    pub fn flashing(&self)->bool {self.active_flash.is_some()}
    /// 处理一次外壳事件：闪动开始计提醒，闪动结束视为用户点开了消息。
    ///
    /// Windows 的 HSHELL_FLASH 与 HSHELL_FLASH|HSHELL_FLASHORIG 成对出现：开始时 lParam 是
    /// 发出闪动的窗口，用户点开/超时结束时 lParam 为 0 或别的窗口。开始时只弹一次横幅；
    /// 结束时把这条未读收掉——否则用户已经在浏览器点开了消息，岛上却一直挂着红点。
    pub fn hear(&mut self,flash:bool,pid:u32,window:isize,foreground:Option<u32>,now:Instant)->Heard{
        self.foreground(foreground,now);
        if !flash {
            if let Some(ended)=self.active_flash.take(){
                self.burst.retain(|(p,_)|*p!=ended);
                // 刚看过 / 切走的账号，其闪动结束是余波，不重复处理。
                if self.viewing.contains(&ended) {return Heard::Viewing;}
                match self.pending.iter_mut().find(|(p,_)|*p==ended){
                    Some(entry)=>{entry.1=isize::MAX;Heard::Ended}
                    None=>Heard::Ignored,
                }
            }else{return Heard::Ignored;}
        }else{
            if pid==0 {return Heard::Ignored;}
            // 上一轮闪动向用户点开为止一直「没结束」：用户没点（任务栏自己停了）也要能接住下一条消息。
            // 超过 BURST_IDLE 没有新的闪动，就当作上一轮已结束，并把它的未读收掉。
            if self.active_flash.is_some_and(|active|active!=pid || self.last_frame.is_some_and(|t|now.duration_since(t)>=Source::BURST_IDLE)) {
                if let Some(ended)=self.active_flash.take(){
                    self.burst.retain(|(p,_)|*p!=ended);
                    if let Some(entry)=self.pending.iter_mut().find(|(p,_)|*p==ended){entry.1=isize::MAX;}
                }
            }
            self.active_flash=Some(pid);
            self.last_frame=Some(now);
            if foreground==Some(pid){self.enter_view(pid);return Heard::Viewing;}
            if self.flash(pid,window,now){Heard::Alert}else{Heard::Merged}
        }
    }
}

fn normalized(path: &str) -> String {
    path.trim_start_matches(r"\\?\").replace('/', "\\").to_lowercase()
}

fn registry_string(root:HKEY,key:PCWSTR,name:PCWSTR)->Option<String>{
    let mut data=[0u16;2048];let mut size=(data.len()*2) as u32;
    unsafe {RegGetValueW(root,key,name,RRF_RT_REG_SZ,None,Some(data.as_mut_ptr().cast()),Some(&mut size))}.ok().ok()?;
    let length=data.iter().position(|c|*c==0).unwrap_or(data.len());
    Some(String::from_utf16_lossy(&data[..length]))
}

// 只提取协议处理程序的路径，绝不执行注册表里的命令或参数。
fn protocol_executable(command:&str)->Option<std::path::PathBuf>{
    let command=command.trim();
    let exe=if let Some(quoted)=command.strip_prefix('"'){quoted.split_once('"')?.0}else{
        command.strip_suffix(" \"%1\"").or_else(||command.strip_suffix(" %1")).unwrap_or(command)
    };
    let path=std::path::PathBuf::from(exe);
    (path.is_absolute() && path.is_file()).then_some(path)
}

pub(crate) fn local_app_data()->Option<std::path::PathBuf>{
    crate::model::config_dir().parent().map(|p|p.join("Local"))
}

/// 钉钉的安装路径：先认协议处理器（更新后仍指向当前版本目录），再认固定位置与运行中的进程。
pub fn dingtalk_paths()->Vec<String>{
    let mut candidates:Vec<std::path::PathBuf>=Vec::new();
    if let Some(exe)=registry_string(HKEY_CLASSES_ROOT,w!("dingtalk\\Shell\\Open\\Command"),PCWSTR::null()).and_then(|cmd|protocol_executable(&cmd)){
        // DingtalkScheme.exe 在安装根目录，主程序在 main\\current\\DingTalk.exe。
        for root in exe.ancestors().skip(1).take(2) {
            candidates.push(root.join("main").join("current").join("DingTalk.exe"));
            candidates.push(root.join("DingTalk.exe"));
        }
    }
    let mut roots:Vec<std::path::PathBuf>=["ProgramFiles(x86)","ProgramFiles"].iter().filter_map(|name|std::env::var(name).ok()).map(|folder|std::path::PathBuf::from(folder).join("DingDing")).collect();
    if let Some(local)=local_app_data(){roots.push(local.join("DingTalk"));}
    for root in roots {
        candidates.push(root.join("main").join("current").join("DingTalk.exe"));
        candidates.push(root.join("DingTalk.exe"));
    }
    // 正在运行的钉钉：直接从进程取路径，绿色版 / 换盘 / 多开都覆盖得到。
    candidates.extend(running_paths("dingtalk.exe"));
    let mut paths:Vec<String>=candidates.into_iter().filter(|p|p.is_absolute() && p.is_file())
        .filter_map(|p|p.canonicalize().ok()).map(|p|normalized(&p.to_string_lossy())).collect();
    paths.sort();paths.dedup();
    // 安装目录可能不在上面的位置：再认文件名相同、且由阿里发布的程序。
    paths.push("*dingtalk.exe".into());
    paths
}

/// 按文件名找正在运行的进程的完整路径。只读取进程镜像名，不碰对方内容。
fn running_paths(exe:&str)->Vec<std::path::PathBuf>{
    use windows::core::PWSTR;
    use windows::Win32::System::{Diagnostics::ToolHelp::{CreateToolhelp32Snapshot,Process32FirstW,Process32NextW,PROCESSENTRY32W,TH32CS_SNAPPROCESS},
        Threading::{OpenProcess,QueryFullProcessImageNameW,PROCESS_NAME_WIN32,PROCESS_QUERY_LIMITED_INFORMATION}};
    let mut found=Vec::new();
    unsafe{
        let Ok(snapshot)=CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS,0) else {return found};
        let mut entry=PROCESSENTRY32W{dwSize:std::mem::size_of::<PROCESSENTRY32W>() as u32,..Default::default()};
        let mut ok=Process32FirstW(snapshot,&mut entry).is_ok();
        while ok {
            let name=String::from_utf16_lossy(&entry.szExeFile);
            if exe.eq_ignore_ascii_case(name.trim_end_matches('\0')){
                if let Ok(process)=OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION,false,entry.th32ProcessID){
                    let mut buffer=[0u16;32768];let mut length=buffer.len() as u32;
                    if QueryFullProcessImageNameW(process,PROCESS_NAME_WIN32,PWSTR(buffer.as_mut_ptr()),&mut length).is_ok(){
                        found.push(std::path::PathBuf::from(String::from_utf16_lossy(&buffer[..length as usize])));
                    }
                    let _=CloseHandle(process);
                }
            }
            ok=Process32NextW(snapshot,&mut entry).is_ok();
        }
        let _=CloseHandle(snapshot);
    }
    found
}

pub(crate) fn client_paths(source:crate::app::Messenger)->Vec<String>{
    match source {
        crate::app::Messenger::Wechat=>crate::wechat::installed_paths(),
        crate::app::Messenger::DingTalk=>dingtalk_paths(),
        crate::app::Messenger::Qq=>crate::wechat::qq_paths(),
    }
}

fn client_main_frame(source:crate::app::Messenger,class:&str,style:u32,extended:u32)->bool{
    let known=match source {
        crate::app::Messenger::Qq=>matches!(class,"Chrome_WidgetWin_1"|"TXGuiFoundation"),
        crate::app::Messenger::Wechat=>class=="WeChatMainWndForPC" || class.starts_with("Qt") && class.ends_with("QWindowIcon"),
        // 钉钉：新版 Electron（Chrome_WidgetWin_1）与旧版 Qt 外壳都认；靠「有系统菜单 + 最小化按钮」过滤弹窗。
        crate::app::Messenger::DingTalk=>class=="Chrome_WidgetWin_1" || class=="StandardFrame_DingTalk" || class.starts_with("Qt") && class.ends_with("QWindowIcon"),
    };
    let frame=(WS_SYSMENU|WS_MINIMIZEBOX).0;
    let resizable_frame=(WS_CAPTION|WS_THICKFRAME|WS_MINIMIZEBOX).0;
    let custom_frame=class.starts_with("Qt") || class=="Chrome_WidgetWin_1";
    known && (style & frame==frame || custom_frame && style & resizable_frame==resizable_frame) && extended & (WS_EX_TOOLWINDOW|WS_EX_NOACTIVATE).0==0
}

pub(crate) fn window_pid(hwnd:HWND)->u32{
    let mut pid=0;
    if !hwnd.0.is_null(){unsafe{GetWindowThreadProcessId(hwnd,Some(&mut pid));}}
    pid
}

pub(crate) fn class_name(hwnd:HWND)->String{
    let mut class=[0u16;128];
    let n=unsafe{GetClassNameW(hwnd,&mut class)}.max(0) as usize;
    String::from_utf16_lossy(&class[..n])
}

pub(crate) unsafe fn visible_main_frame(hwnd:HWND,source:crate::app::Messenger)->bool{
    if !IsWindowVisible(hwnd).as_bool() || GetWindow(hwnd,GW_OWNER).is_ok(){return false;}
    client_main_frame(source,&class_name(hwnd),GetWindowLongW(hwnd,GWL_STYLE) as u32,GetWindowLongW(hwnd,GWL_EXSTYLE) as u32)
}

/// 前台是该客户端的主窗口时返回它所在的进程（多开时每个账号一个进程）。
pub(crate) unsafe fn foreground_main_pid(source:crate::app::Messenger,paths:&[String])->Option<u32>{
    let hwnd=GetForegroundWindow();
    if IsIconic(hwnd).as_bool() || !visible_main_frame(hwnd,source) || !crate::wechat::matches_paths(hwnd,paths){return None;}
    Some(window_pid(hwnd))
}

struct State {
    message: u32,
    taskbar_created: u32,
    registered: bool,
    paths: Vec<(crate::app::Messenger,Vec<String>)>,
    sources: Vec<Source>,
    ctx: eframe::egui::Context,
}
impl State {
    fn slot(&mut self,source:crate::app::Messenger)->&mut Source{
        if !self.sources.iter().any(|s|s.source==source){self.sources.push(Source::new(source));}
        self.sources.iter_mut().find(|s|s.source==source).unwrap()
    }
    fn paths_of(&mut self,source:crate::app::Messenger)->Vec<String>{
        self.paths.iter().find(|(s,_)|*s==source).map(|(_,p)|p.clone()).unwrap_or_default()
    }
    /// 这个进程属于哪个来源。
    fn source_of(&self,pid:u32)->Option<crate::app::Messenger>{
        self.paths.iter().find(|(_,p)|crate::wechat::pid_in(pid,p)).map(|(s,_)|*s)
    }
    /// 闪动结束事件不带真的窗口：归给还在闪的那个来源。
    fn source_with_active_flash(&self)->Option<crate::app::Messenger>{
        self.sources.iter().find(|s|s.flashing()).map(|s|s.source)
    }
}

unsafe extern "system" fn subclass(hwnd: HWND, message: u32, wp: WPARAM, lp: LPARAM, _: usize, data: usize) -> LRESULT {
    {
        let slot=&*(data as *const RefCell<State>);
        if let Ok(mut state)=slot.try_borrow_mut(){
            if message==state.taskbar_created && message!=0 {
                state.registered=RegisterShellHookWindow(hwnd).as_bool();
                state.ctx.request_repaint();
            }
            if message==state.message && matches!(wp.0, FLASH | 4 | 0x8004) {
                let target=HWND(lp.0 as *mut _);
                let pid=window_pid(target);
                // 闪动结束时 lParam 是 0 或任意窗口（不一定是钉钉的）：用「谁的闪动还没结束」来归属。
                let source=state.source_of(pid).or_else(||state.source_with_active_flash());
                if let Some(source)=source {
                    crate::wechat::trace(format!("外壳事件 pid={pid} 码=0x{:X} 窗口={:#x}",wp.0,target.0 as usize));
                    let paths=state.paths_of(source);
                    let foreground=foreground_main_pid(source,&paths);
                    let now=Instant::now();
                    let ours=state.slot(source);
                    ours.foreground(foreground,now);
                    match ours.hear(wp.0==FLASH,pid,target.0 as isize,foreground,now) {
                        Heard::Alert=>{crate::wechat::trace(format!("{} 闪动 pid={pid} 窗口={:#x} → 新提醒",source.name(),target.0 as usize));state.ctx.request_repaint();}
                        Heard::Merged=>crate::wechat::trace(format!("{} 闪动 pid={pid} 窗口={:#x} → 合并 / 宽限期内",source.name(),target.0 as usize)),
                        Heard::Viewing=>crate::wechat::trace(format!("{} 闪动来自正在查看的账号 {pid}，不提醒",source.name())),
                        Heard::Ended=>{crate::wechat::trace(format!("{} 已停止闪动（用户点开了消息）",source.name()));state.ctx.request_repaint();}
                        Heard::Ignored=>{},
                    }
                }
            }
        }
    }
    if message==WM_NCDESTROY {
        let _=DeregisterShellHookWindow(hwnd);
        let _=RemoveWindowSubclass(hwnd,Some(subclass),SUBCLASS_ID);
    }
    DefSubclassProc(hwnd,message,wp,lp)
}

pub struct Listener { hwnd: HWND, state: Box<RefCell<State>> }
impl Listener {
    /// SOURCES 是要监听的来源（关掉提醒的来源不要传进来）。
    pub fn new(ctx: eframe::egui::Context, sources:&[crate::app::Messenger]) -> Result<Self,String> {
        let paths:Vec<(crate::app::Messenger,Vec<String>)>=sources.iter().map(|&s|(s,client_paths(s))).filter(|(_,p)|!p.is_empty()).collect();
        if paths.is_empty(){return Err("未找到微信或钉钉的安装路径".into());}
        unsafe {
            let message=RegisterWindowMessageW(w!("SHELLHOOK"));
            if message==0{return Err("无法注册任务栏提醒事件".into());}
            let hwnd=CreateWindowExW(WS_EX_TOOLWINDOW,w!("STATIC"),w!("DaozaiMessengerListener"),WS_POPUP,0,0,0,0,None,None,None,None).map_err(|_|"无法创建提醒监听窗口")?;
            let state=Box::new(RefCell::new(State{message,taskbar_created:RegisterWindowMessageW(w!("TaskbarCreated")),registered:true,paths,sources:Vec::new(),ctx}));
            let listener=Self{hwnd,state};
            SetWindowSubclass(hwnd,Some(subclass),SUBCLASS_ID,(&*listener.state as *const RefCell<State>) as usize).ok().map_err(|_|"无法接入提醒监听")?;
            RegisterShellHookWindow(hwnd).ok().map_err(|_|"无法监听任务栏提醒")?;
            Ok(listener)
        }
    }
    pub fn registered(&self)->bool{self.state.borrow().registered}
    /// 取走某来源的新提醒：每项是（账号进程，闪动的窗口）。
    pub fn take(&self, source:crate::app::Messenger) -> Vec<(u32,isize)> {
        self.state.borrow_mut().slot(source).take()
    }
    /// 当前监听的来源（找不到安装路径的来源不会出现在这里）。
    pub fn sources(&self)->Vec<crate::app::Messenger>{self.state.borrow().paths.iter().map(|(s,_)|*s).collect()}
}
impl Drop for Listener {
    fn drop(&mut self){unsafe{
        let _=DeregisterShellHookWindow(self.hwnd);
        let _=RemoveWindowSubclass(self.hwnd,Some(subclass),SUBCLASS_ID);
        let _=DestroyWindow(self.hwnd);
    }}
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn continuous_flashes_alert_once_and_a_new_round_after_five_quiet_seconds(){
        let mut s=Source::new(crate::app::Messenger::DingTalk);
        let t0=Instant::now();
        assert!(s.flash(42,0x11,t0),"第一下闪动应提醒");
        assert!(!s.flash(42,0x12,t0+Duration::from_millis(500)),"同一轮连续闪动只提醒一次");
        assert_eq!(s.pending_len(),1,"同一账号合并为一条");
        assert!(s.flash(42,0x13,t0+Duration::from_secs(9)),"停顿后的下一条消息立刻提醒");
        assert_eq!(s.take(),vec![(42,0x13)]);
        assert!(s.take().is_empty(),"取走后不再重复");
    }
    #[test]
    fn leaving_a_viewed_account_suppresses_the_tail_flash(){
        let mut s=Source::new(crate::app::Messenger::DingTalk);
        let t0=Instant::now();
        s.foreground(Some(42),t0);
        assert!(s.viewing.contains(&42),"正在看的账号记入前台");
        s.foreground(None,t0+Duration::from_secs(1));
        assert!(!s.flash(42,0x11,t0+Duration::from_secs(2)),"刚切走 4 秒内的补闪是刚才那次提醒的余波");
        assert!(s.flash(42,0x11,t0+Duration::from_secs(30)),"宽限期过后才提醒");
        assert_eq!(s.take(),vec![(42,0x11)]);
    }
    #[test]
    fn a_flash_from_the_client_in_front_still_counts_as_a_new_message(){
        // 用户从桌面 Alt+Tab 到钉钉，消息同时到达：不得因为「刚进前台」就被当成切走而跳过提醒。
        let mut s=Source::new(crate::app::Messenger::DingTalk);
        let t0=Instant::now();
        assert!(matches!(s.hear(true,42,0x11,Some(42),t0),Heard::Viewing));
        // 用户离开钉钉（切到别的程序）：进入切走宽限期。
        s.foreground(None,t0+Duration::from_millis(500));
        assert!(matches!(s.hear(true,42,0x11,None,t0+Duration::from_secs(30)),Heard::Alert),"几分钟后的新消息要提醒");
    }
    #[test]
    fn accounts_are_merged_independently(){
        let mut s=Source::new(crate::app::Messenger::DingTalk);
        let t0=Instant::now();
        assert!(s.flash(1,0x11,t0));
        assert!(s.flash(2,0x22,t0+Duration::from_millis(50)),"另一个账号的闪动不受影响");
        assert_eq!(s.take(),vec![(1,0x11),(2,0x22)]);
    }
    #[test]
    fn flashing_alerts_once_then_clears_the_unread_when_it_ends(){
        let mut s=Source::new(crate::app::Messenger::DingTalk);
        let t0=Instant::now();
        assert!(matches!(s.hear(true,42,0x11,None,t0),Heard::Alert),"闪动开始时弹一次横幅");
        assert!(matches!(s.hear(true,42,0x12,None,t0+Duration::from_millis(300)),Heard::Merged),"同一轮连续闪动合并");
        assert!(matches!(s.hear(false,0,0,None,t0+Duration::from_secs(2)),Heard::Ended),"闪动结束（用户点开了消息）");
        assert!(!s.flashing(),"结束后不再闪");
        // 结束事件不带窗口，按「还在闪的来源」归属，并通过 isize::MAX 标记待收掉未读。
        assert_eq!(s.take(),vec![(42,isize::MAX)]);
    }
    #[test]
    fn a_new_message_few_seconds_after_the_last_one_still_alerts(){
        // 用户反馈：看完一条后要等很久才会再提示。旧实现要求「安静 5 秒」才提醒，
        // 现在只要确实停了（>1 秒没有闪动）就立刻提醒。
        let mut s=Source::new(crate::app::Messenger::DingTalk);
        let t0=Instant::now();
        assert!(matches!(s.hear(true,42,0x11,None,t0),Heard::Alert));
        assert!(matches!(s.hear(true,42,0x12,None,t0+Duration::from_secs(2)),Heard::Alert),"隔 2 秒的第二条消息要立刻提醒");
    }
    #[test]
    fn a_second_message_after_a_long_stop_flashes_again(){
        // 用户反馈：「连横幅都没有」。钉钉持续闪动 15 秒后停止 44 秒又来消息，必须再提醒。
        let mut s=Source::new(crate::app::Messenger::DingTalk);
        let t0=Instant::now();
        assert!(matches!(s.hear(true,42,0x11,None,t0),Heard::Alert));
        for step in 1..16 {assert!(matches!(s.hear(true,42,0x11,None,t0+Duration::from_millis(300*step)),Heard::Merged),"同一轮每半秒一下的闪动都合并");}
        assert!(matches!(s.hear(true,42,0x12,None,t0+Duration::from_secs(60)),Heard::Alert),"停止闪动 44 秒后的新消息要提醒");
    }
    #[test]
    fn a_pending_unread_survives_entering_the_view(){
        // 前台判定只要来过一次，都不能把已经到手的未读抹掉；只有 main 里真的切到窗口才清。
        let mut s=Source::new(crate::app::Messenger::DingTalk);
        let t0=Instant::now();
        assert!(matches!(s.hear(true,42,0x11,None,t0),Heard::Alert));
        s.foreground(Some(42),t0+Duration::from_millis(50));
        assert_eq!(s.pending_len(),1,"进入前台不销毁未读");
        assert_eq!(s.take(),vec![(42,0x11)]);
    }
    #[test]
    fn flash_end_without_a_start_is_ignored(){
        let mut s=Source::new(crate::app::Messenger::DingTalk);
        assert!(matches!(s.hear(false,0,0,None,Instant::now()),Heard::Ignored));
        assert!(s.take().is_empty());
        assert!(matches!(s.hear(true,0,0,None,Instant::now()),Heard::Ignored),"没有进程的闪动不处理");
    }
    #[test]
    fn dingtalk_paths_always_include_the_company_guard(){
        let paths=dingtalk_paths();
        assert_eq!(paths.last().map(String::as_str),Some("*dingtalk.exe"),"换盘 / 绿色版靠文件名 + 发布者兜底");
    }
}
