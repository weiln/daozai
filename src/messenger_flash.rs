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

#[derive(Default)]
struct Burst { last: Option<Instant>, quiet_until: Option<Instant> }
impl Burst {
    fn attention(&mut self, now: Instant) -> bool {
        // 刚看完切走：任务栏补闪不算新提醒，但计入这一轮，持续闪动也继续合并。
        if self.quiet_until.is_some_and(|until| now < until) {self.last=Some(now);return false;}
        // 连续闪动只提醒一次；停止闪动五秒后，下一轮才算新提醒。
        self.last.replace(now).is_none_or(|last| now.duration_since(last) >= Duration::from_secs(5))
    }
    /// 用户刚从这个账号的窗口切走。
    fn left(now: Instant) -> Self { Self{last:Some(now),quiet_until:Some(now+LEAVE_GRACE)} }
}

/// 一个来源（微信 / 钉钉）的合并与待取提醒。纯逻辑，可脱离 Windows 单测。
pub struct Source {
    pub source: crate::app::Messenger,
    /// 每个账号（进程）各自合并连续闪动。
    burst: Vec<(u32,Burst)>,
    /// 尚未取走的提醒：账号进程与闪动的窗口。
    pending: Vec<(u32,isize)>,
    /// 当前正在前台查看的账号进程。
    viewing: Vec<u32>,
}
impl Source {
    pub fn new(source:crate::app::Messenger)->Self { Self{source,burst:Vec::new(),pending:Vec::new(),viewing:Vec::new()} }
    /// 取走新提醒：每项是（账号进程，闪动的窗口）。
    pub fn take(&mut self) -> Vec<(u32,isize)> { std::mem::take(&mut self.pending) }
    pub fn pending_len(&self)->usize { self.pending.len() }
    /// 用户正在前台看这个账号：它自己的提醒作废，其它账号不受影响。
    pub fn enter_view(&mut self,pid:u32){
        self.burst.retain(|(p,_)|*p!=pid);
        self.pending.retain(|(p,_)|*p!=pid);
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
    /// 收到一次闪动：返回是否算新提醒（已写入待取列表）。
    pub fn flash(&mut self,pid:u32,window:isize,now:Instant)->bool{
        let fresh={
            if !self.burst.iter().any(|(p,_)|*p==pid){self.burst.push((pid,Burst::default()));}
            self.burst.iter_mut().find(|(p,_)|*p==pid).is_some_and(|(_,burst)|burst.attention(now))
        };
        if fresh {
            match self.pending.iter_mut().find(|(p,_)|*p==pid){Some(entry)=>entry.1=window,None=>self.pending.push((pid,window))}
        }
        fresh
    }
    /// 外壳报告的激活事件：只用来判断「谁进了前台 / 谁切走了」。
    pub fn foreground(&mut self,foreground:Option<u32>,now:Instant){self.leave_view(foreground,now);}
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
                if let Some(source)=state.source_of(pid) {
                    let paths=state.paths_of(source);
                    let foreground=foreground_main_pid(source,&paths);
                    let now=Instant::now();
                    let ours=state.slot(source);
                    ours.foreground(foreground,now);
                    if foreground==Some(pid){
                        // 正在看这个账号：它的提醒作废，其它账号不受影响。
                        ours.enter_view(pid);
                    }else if wp.0==FLASH && ours.flash(pid,target.0 as isize,now){
                        state.ctx.request_repaint();
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
        assert!(!s.flash(42,0x12,t0+Duration::from_secs(1)),"连续闪动只提醒一次");
        assert_eq!(s.pending_len(),1,"同一账号合并为一条");
        assert!(s.flash(42,0x13,t0+Duration::from_secs(9)),"停止闪动五秒后的下一轮算新提醒");
        assert_eq!(s.take(),vec![(42,0x13)]);
        assert!(s.take().is_empty(),"取走后不再重复");
    }
    #[test]
    fn leaving_a_viewed_account_suppresses_the_tail_flash(){
        let mut s=Source::new(crate::app::Messenger::DingTalk);
        let t0=Instant::now();
        s.enter_view(42);
        assert!(s.viewing.contains(&42),"正在看的账号记入前台");
        s.foreground(None,t0+Duration::from_secs(1));
        assert!(!s.flash(42,0x11,t0+Duration::from_secs(2)),"刚切走 4 秒内的补闪是刚才那次提醒的余波");
        assert!(s.flash(42,0x11,t0+Duration::from_secs(30)),"宽限期过后才提醒");
        assert_eq!(s.take(),vec![(42,0x11)]);
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
    fn dingtalk_paths_always_include_the_company_guard(){
        let paths=dingtalk_paths();
        assert_eq!(paths.last().map(String::as_str),Some("*dingtalk.exe"),"换盘 / 绿色版靠文件名 + 发布者兜底");
    }
}
