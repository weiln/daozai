//! Windows 音量混合器中的播放器音量；只操作匹配的应用会话。
use std::time::{Duration, Instant};

#[derive(Default)]
pub(crate) struct Reader {
    source: String,
    at: Option<Instant>,
    value: Option<(u32, bool)>,
}
impl Reader {
    pub fn poll(&mut self, source:&str)->Option<(u32,bool)> {
        if self.source!=source || self.at.is_none_or(|at|at.elapsed()>=Duration::from_millis(500)) {
            self.source=source.into();self.at=Some(Instant::now());self.value=read(source);
        }
        self.value
    }
}

fn executable(source:&str)->Option<String> {
    let source=source.to_lowercase();
    match source.as_str() {
        "kugou"=>Some("kugou.exe".into()),
        "qqmusic"=>Some("qqmusic.exe".into()),
        "spotify"=>Some("spotify.exe".into()),
        "汽水音乐"=>Some("sodamusic.exe".into()),
        _ if source.ends_with(".exe") && !source.contains(['\\','/'])=>Some(source),
        _=>None,
    }
}

#[cfg(windows)]
mod platform {
    use windows::{core::{Interface,PWSTR},Win32::{
        Foundation::CloseHandle,
        Media::Audio::{AudioSessionStateExpired,IAudioSessionControl2,IAudioSessionManager2,IMMDeviceEnumerator,ISimpleAudioVolume,MMDeviceEnumerator,DEVICE_STATE_ACTIVE,eRender},
        Storage::Packaging::Appx::GetApplicationUserModelId,
        System::{Com::{CoCreateInstance,CoInitializeEx,CLSCTX_ALL,COINIT_MULTITHREADED},Threading::{OpenProcess,PROCESS_QUERY_LIMITED_INFORMATION}},
    }};

    fn matches(pid:u32,source:&str)->bool {
        if source.eq_ignore_ascii_case("kwmusic.exe") && crate::smtc::player_process::image_is(pid,"KwService.exe"){return crate::smtc::kuwo::is_service(pid);}
        if let Some(exe)=super::executable(source){return crate::smtc::player_process::image_is(pid,&exe);}
        if pid==0 || !source.contains('!'){return false;}
        unsafe {
            let Ok(process)=OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION,false,pid) else{return false;};
            let mut id=[0u16;512];let mut length=id.len() as u32;
            let result=GetApplicationUserModelId(process,&mut length,PWSTR(id.as_mut_ptr()));
            let _=CloseHandle(process);
            result.is_ok() && length>0 && (length as usize)<=id.len()
                && String::from_utf16_lossy(&id[..length as usize-1]).eq_ignore_ascii_case(source)
        }
    }

    pub fn sessions(source:&str)->Vec<ISimpleAudioVolume> {
        if source.is_empty(){return Vec::new();}
        unsafe {
            // 采样、控制各用自己的 COM 线程与枚举器，不跨线程传递音频接口。
            thread_local!{static ENUMERATOR:Option<IMMDeviceEnumerator>=unsafe{
                let _=CoInitializeEx(None,COINIT_MULTITHREADED);
                CoCreateInstance(&MMDeviceEnumerator,None,CLSCTX_ALL).ok()
            };}
            ENUMERATOR.with(|enumerator|{
                let mut volumes=Vec::new();
                let Some(enumerator)=enumerator else{return volumes;};
                // 应用可能被路由到非默认输出，遍历当前可用的播放设备。
                let Ok(devices)=enumerator.EnumAudioEndpoints(eRender,DEVICE_STATE_ACTIVE) else{return volumes;};
                for d in 0..devices.GetCount().unwrap_or(0) {
                    let Ok(device)=devices.Item(d) else{continue;};
                    let Ok(manager)=device.Activate::<IAudioSessionManager2>(CLSCTX_ALL,None) else{continue;};
                    let Ok(sessions)=manager.GetSessionEnumerator() else{continue;};
                    for s in 0..sessions.GetCount().unwrap_or(0) {
                        let Ok(session)=sessions.GetSession(s) else{continue;};
                        if session.GetState().ok()==Some(AudioSessionStateExpired){continue;}
                        let Ok(control)=session.cast::<IAudioSessionControl2>() else{continue;};
                        if !control.GetProcessId().ok().is_some_and(|pid|matches(pid,source)){continue;}
                        if let Ok(volume)=session.cast::<ISimpleAudioVolume>(){volumes.push(volume);}
                    }
                }
                volumes
            })
        }
    }
}

#[cfg(windows)]
pub fn read(source:&str)->Option<(u32,bool)> {
    platform::sessions(source).iter().filter_map(|session|unsafe {
        Some(((session.GetMasterVolume().ok()?*100.0).round() as u32,session.GetMute().ok()?.as_bool()))
    }).reduce(|a,b|(a.0.max(b.0),a.1 && b.1))
}

#[cfg(windows)]
pub fn set_volume(source:&str,percent:u32)->bool {
    apply(source,|session|unsafe {
        session.SetMasterVolume(percent.min(100) as f32/100.0,std::ptr::null()).is_ok()
            && session.SetMute(false,std::ptr::null()).is_ok()
    })
}

#[cfg(windows)]
pub fn set_mute(source:&str,muted:bool)->bool {
    apply(source,|session|unsafe{session.SetMute(muted,std::ptr::null()).is_ok()})
}

#[cfg(windows)]
fn apply(source:&str,change:impl Fn(&windows::Win32::Media::Audio::ISimpleAudioVolume)->bool)->bool {
    let sessions=platform::sessions(source);
    let mut success=!sessions.is_empty();
    for session in &sessions{success=change(session) && success;}
    success
}

#[cfg(not(windows))]
pub fn read(_source:&str)->Option<(u32,bool)>{None}
#[cfg(not(windows))]
pub fn set_volume(_source:&str,_percent:u32)->bool{false}
#[cfg(not(windows))]
pub fn set_mute(_source:&str,_muted:bool)->bool{false}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn player_names_require_an_exact_executable_or_packaged_identity(){
        for (source,exe) in [("QQMusic.exe","qqmusic.exe"),("kugou","kugou.exe"),("汽水音乐","sodamusic.exe")]{assert_eq!(executable(source).as_deref(),Some(exe));}
        for source in ["","music","QQMusic.exe.bak",r"C:\QQMusic.exe","SpotifyAB.SpotifyMusic_123!Spotify"]{assert_eq!(executable(source),None);}
    }
    #[test]
    fn unknown_player_cannot_change_system_volume(){
        assert_eq!(read("daozai-missing-player.exe"),None);
        assert!(!set_volume("daozai-missing-player.exe",100));
        assert!(!set_mute("",true));
    }

    #[test]
    #[cfg(windows)]
    #[ignore="需要本机音频输出设备；创建无声会话验证 Windows 混音器读写"]
    fn live_windows_sessions_change_together_without_changing_master_volume(){
        use windows::Win32::{Media::Audio::{IAudioClient,IMMDeviceEnumerator,ISimpleAudioVolume,MMDeviceEnumerator,AUDCLNT_SHAREMODE_SHARED,eRender,eConsole},System::Com::{CoCreateInstance,CoCreateGuid,CoInitializeEx,CoTaskMemFree,CLSCTX_ALL,COINIT_MULTITHREADED}};
        unsafe {
            let _=CoInitializeEx(None,COINIT_MULTITHREADED);
            let enumerator:IMMDeviceEnumerator=CoCreateInstance(&MMDeviceEnumerator,None,CLSCTX_ALL).unwrap();
            let device=enumerator.GetDefaultAudioEndpoint(eRender,eConsole).unwrap();
            let create=||{
                let client:IAudioClient=device.Activate(CLSCTX_ALL,None).unwrap();
                let format=client.GetMixFormat().unwrap();let guid=CoCreateGuid().unwrap();
                let result=client.Initialize(AUDCLNT_SHAREMODE_SHARED,0,1_000_000,0,format,Some(&guid));
                CoTaskMemFree(Some(format as _));result.unwrap();
                let volume:ISimpleAudioVolume=client.GetService().unwrap();(client,volume)
            };
            let (_first,a)=create();let (_second,b)=create();
            let source=std::env::current_exe().unwrap().file_name().unwrap().to_str().unwrap().to_owned();
            a.SetMasterVolume(0.31,std::ptr::null()).unwrap();b.SetMasterVolume(0.52,std::ptr::null()).unwrap();
            let before=crate::system::Snapshot::read().volume;
            assert_eq!(read(&source),Some((52,false)));
            assert!(set_volume(&source,27));
            assert_eq!(read(&source),Some((27,false)));
            assert!((a.GetMasterVolume().unwrap()-0.27).abs()<0.001);
            assert!((b.GetMasterVolume().unwrap()-0.27).abs()<0.001);
            assert!(set_mute(&source,true));assert!(a.GetMute().unwrap().as_bool() && b.GetMute().unwrap().as_bool());
            assert!(set_volume(&source,19));assert_eq!(read(&source),Some((19,false)));
            assert!(!set_volume("daozai-missing-player.exe",100));assert_eq!(read(&source),Some((19,false)));
            assert!(set_volume(&source,999));assert_eq!(read(&source),Some((100,false)));
            assert_eq!(crate::system::Snapshot::read().volume,before,"应用音量不能改变系统主音量或静音");
            println!("双会话音量、静音、解除静音、边界和系统音量隔离验证通过");
        }
    }
}
