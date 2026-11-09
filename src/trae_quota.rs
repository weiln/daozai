//! Trae CN 网页账号积分：只读查询，令牌来自本机浏览器或手动粘贴。
//! 令牌是 www.trae.cn 的 Cloud-IDE-Token，接口要求 Authorization: Cloud-IDE-JWT <令牌>。
use super::{Account,Snapshot};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD,Engine};
use chrono::Datelike;
use serde_json::{json,Value};
use std::{fs,io::Read,path::PathBuf,sync::mpsc,time::SystemTime};

const ENDPOINT:&str="https://api.trae.cn/trae/api/v2/pay/user_current_entitlement_list";
const ORIGIN:&str="https://www.trae.cn";
const MARKER:&str="Cloud-IDE-Token";
const JWT_HEAD:&str="eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9.";
const MAX_FILE:u64=8*1024*1024;
const LOGIN_ERROR:&str="Trae CN 登录已失效，请在浏览器重新登录 www.trae.cn 后重新连接";
const PARSE_ERROR:&str="Trae CN 额度接口已变化，暂时无法读取";
const MISSING_ERROR:&str="未找到 Trae CN 登录令牌：请先在浏览器打开 www.trae.cn 登录，或在下方粘贴令牌";

#[derive(Clone,Debug)]
struct Claims{user:String,exp:i64}
fn claims(token:&str)->Option<Claims>{
    if token.len()<64 || token.len()>4096 || !token.bytes().all(|b|b.is_ascii_alphanumeric()||matches!(b,b'-'|b'_'|b'.')){return None;}
    let parts=token.split('.').collect::<Vec<_>>();
    if parts.len()!=3 || parts.iter().any(|part|part.is_empty()){return None;}
    let payload=URL_SAFE_NO_PAD.decode(parts[1]).ok()?;
    let value:Value=serde_json::from_slice(&payload).ok()?;
    let user=value.pointer("/data/id").and_then(Value::as_str).filter(|id|!id.is_empty()&&id.len()<=64&&id.bytes().all(|b|b.is_ascii_digit()))?.to_owned();
    let exp=value["exp"].as_i64().filter(|exp|*exp>0)?;
    Some(Claims{user,exp})
}
/// 手动粘贴时允许带 "Cloud-IDE-JWT" 前缀，统一取出纯令牌。
fn token(raw:&str)->Result<String,String>{
    let raw=raw.trim();
    let raw=raw.strip_prefix("Cloud-IDE-JWT").unwrap_or(raw).trim_start_matches(':').trim();
    claims(raw).ok_or_else(||"请粘贴 www.trae.cn 控制台里的 Cloud-IDE-Token 令牌".to_string())?;
    Ok(raw.to_owned())
}
fn slot(user:&str)->String{ring::digest::digest(&ring::digest::SHA256,user.as_bytes()).as_ref().iter().map(|b|format!("{b:02x}")).collect()}
#[derive(serde::Serialize,serde::Deserialize)]
struct Session{token:String,user:String}
fn session_path(slot:&str)->Result<PathBuf,String>{
    if slot.len()!=64 || !slot.bytes().all(|b|b.is_ascii_hexdigit()){return Err(LOGIN_ERROR.into());}
    Ok(crate::model::config_dir().join("credentials").join(format!("trae-{slot}.bin")))
}
fn save(session:&Session)->Result<String,String>{
    let slot=slot(&session.user);let path=session_path(&slot)?;
    let raw=serde_json::to_vec(session).map_err(|_|LOGIN_ERROR)?;
    let sealed=super::gemini::protect(&raw,true)?;
    fs::create_dir_all(path.parent().unwrap()).and_then(|_|crate::model::atomic_write(&path,&sealed)).map_err(|_|"无法保存 Trae CN 登录令牌".to_string())?;
    Ok(slot)
}
fn load(account:&Account)->Result<Session,String>{
    let expected=account.slot.as_deref().ok_or(LOGIN_ERROR)?;let path=session_path(expected)?;
    let raw=fs::read(path).map_err(|_|LOGIN_ERROR)?;let raw=super::gemini::protect(&raw,false)?;
    let session:Session=serde_json::from_slice(&raw).map_err(|_|LOGIN_ERROR)?;
    if slot(&session.user)!=expected{return Err("Trae CN 账号不一致，请重新连接".into());}
    Ok(session)
}
pub fn forget(account:&Account){if let Some(path)=account.slot.as_deref().and_then(|slot|session_path(slot).ok()){let _=fs::remove_file(path);}}

fn number(value:&Value)->Option<f64>{value.as_f64().or_else(||value.as_str()?.parse().ok()).filter(|n|n.is_finite())}
fn pack_limit(packs:&[Value])->Option<f64>{
    let mut total=0.0;let mut known=false;
    for pack in packs.iter().filter(|pack|pack["is_hide"]!=true){
        if let Some(limit)=pack.pointer("/entitlement_base_info/quota/credits_limit").and_then(number).filter(|n|*n>0.0){total+=limit;known=true;}
    }
    known.then_some(total)
}
fn pack_used(packs:&[Value])->Option<f64>{
    let mut total=0.0;let mut known=false;
    for pack in packs.iter().filter(|pack|pack["is_hide"]!=true){
        if let Some(used)=pack.pointer("/usage/credits_amount").and_then(number).filter(|n|*n>=0.0){total+=used;known=true;}
    }
    known.then_some(total)
}
fn cycle_text(reset:Option<i64>)->String{
    let Some(at)=reset.and_then(|at|chrono::DateTime::from_timestamp(at,0)) else{return String::new();};
    let at=at.with_timezone(&chrono::Local);
    if at.timestamp()<=chrono::Local::now().timestamp(){return String::new();}
    at.format(if at.year()==chrono::Local::now().year(){"%-m月%-d日重置"}else{"%Y/%-m/%-d 重置"}).to_string()
}
pub fn parse(value:&Value)->Result<Snapshot,String>{
    if value["code"].as_i64().is_some_and(|code|code!=0){return Err(LOGIN_ERROR.into());}
    let packs=value["user_entitlement_pack_list"].as_array().ok_or(PARSE_ERROR)?;
    let now=chrono::Utc::now().timestamp();
    let mut plan=String::new();let mut reset=None;
    for pack in packs.iter().filter(|pack|pack["is_hide"]!=true && pack["status"].as_i64()==Some(1)){
        let desc=pack["display_desc"].as_str().unwrap_or_default();
        if plan.is_empty()&&!desc.is_empty()&&desc!="签到奖励"{plan=desc.into();}
        if let Some(at)=pack["expire_time"].as_i64().filter(|at|*at>now){reset=Some(reset.map_or(at,|old:i64|old.max(at)));}
    }
    let summary=&value["usage_summary"];
    let total=number(&summary["total_amount"]).or_else(||pack_limit(packs)).filter(|n|*n>0.0).ok_or("Trae CN 暂未返回可用的积分额度")?;
    let used=number(&summary["consumed_amount"]).or_else(||pack_used(packs)).unwrap_or(0.0).max(0.0);
    let credits=super::workbuddy::Credits{remaining:(total-used).max(0.0),packages:vec![("总额".into(),total),("已用".into(),used)],cycle:cycle_text(reset)};
    Ok(Snapshot{plan,credits:Some(credits),at:now,..Default::default()})
}
fn request(agent:&ureq::Agent,token:&str)->Result<Snapshot,(String,u64)>{
    let body=json!({"require_usage":true,"full_data":true}).to_string();
    let response=agent.post(ENDPOINT)
        .set("Authorization",&format!("Cloud-IDE-JWT {token}"))
        .set("Content-Type","application/json").set("Accept","application/json")
        .set("Origin",ORIGIN).set("Referer","https://www.trae.cn/dashboard")
        .set("User-Agent","Mozilla/5.0 Daozai")
        .send_string(&body).map_err(|error|match error{
            ureq::Error::Status(401|403,_)=>(LOGIN_ERROR.into(),0),
            ureq::Error::Status(429,response)=>("Trae CN 请求过于频繁，稍后自动重试".into(),response.header("Retry-After").and_then(|value|value.parse().ok()).unwrap_or(180).clamp(30,3600)),
            ureq::Error::Status(status,_)=>(format!("Trae CN 额度服务暂不可用（{status}）"),60),
            _=>("无法连接 Trae CN，请检查网络或代理".into(),30)
        })?;
    let mut raw=String::new();response.into_reader().take(2*1024*1024).read_to_string(&mut raw).map_err(|_|("读取 Trae CN 额度响应失败".into(),30))?;
    if raw.trim_start().starts_with('<'){return Err((LOGIN_ERROR.into(),0));}
    let value=serde_json::from_str(&raw).map_err(|_|(PARSE_ERROR.into(),60))?;
    parse(&value).map_err(|error|(error,60))
}
pub fn fetch(agent:&ureq::Agent,account:&Account)->Result<Snapshot,(String,u64)>{
    let session=load(account).map_err(|error|(error,0))?;
    request(agent,&session.token)
}

/// 从 Chromium 系浏览器的 Local Storage 里读取 www.trae.cn 保存的令牌。
fn leveldb_dirs()->Vec<PathBuf>{
    let mut roots=Vec::new();
    if let Some(local)=std::env::var_os("LOCALAPPDATA"){
        let local=PathBuf::from(local);
        for relative in ["Microsoft/Edge/User Data","Google/Chrome/User Data","BraveSoftware/Brave-Browser/User Data","Chromium/User Data","Citro Labs/ego lite/User Data"]{
            roots.push(local.join(relative));
        }
    }
    if let Some(roaming)=std::env::var_os("APPDATA"){roots.push(PathBuf::from(roaming).join("Trae CN"));}
    let mut dirs=Vec::new();
    for root in roots{
        let mut profiles=vec![root.clone()];
        if let Ok(entries)=fs::read_dir(&root){profiles.extend(entries.flatten().map(|entry|entry.path()).filter(|path|path.is_dir()));}
        for profile in profiles{
            let dir=profile.join("Local Storage").join("leveldb");
            if dir.is_dir(){dirs.push(dir);}
        }
    }
    dirs
}
fn tokens_in(raw:&[u8])->Vec<String>{
    let text=String::from_utf8_lossy(raw);let mut tokens=Vec::new();let mut from=0;
    while let Some(offset)=text[from..].find(MARKER){
        let start=from+offset;let end=(start+MARKER.len()+4096).min(text.len());
        if let Some(head)=text[start..end].find(JWT_HEAD){
            let begin=start+head;
            let token=text[begin..].chars().take_while(|c|c.is_ascii_alphanumeric()||matches!(c,'-'|'_'|'.')).collect::<String>();
            if token.len()>64{tokens.push(token);}
        }
        from=start+MARKER.len();
    }
    tokens
}
fn browser_token()->Option<(Claims,String)>{
    let now=chrono::Utc::now().timestamp();
    for dir in leveldb_dirs(){
        let Ok(entries)=fs::read_dir(&dir) else{continue;};
        let mut files=entries.flatten().map(|entry|entry.path()).filter(|path|path.is_file()&&matches!(path.extension().and_then(|e|e.to_str()),Some("ldb"|"log")))
            .filter_map(|path|Some((fs::metadata(&path).ok()?.modified().unwrap_or(SystemTime::UNIX_EPOCH),path)))
            .filter(|(_,path)|fs::metadata(path).is_ok_and(|meta|meta.len()<=MAX_FILE)).collect::<Vec<_>>();
        files.sort_by_key(|(modified,_)|std::cmp::Reverse(*modified));
        for (_,path) in files.into_iter().take(24){
            let Ok(raw)=fs::read(&path) else{continue;};
            for candidate in tokens_in(&raw){
                let Some(claims)=claims(&candidate) else{continue;};
                if claims.exp>now+60{return Some((claims,candidate));}
            }
        }
    }
    None
}
/// 本机是否可以免操作识别到一个 Trae CN 账号（不联网，只校验令牌格式与有效期）。
pub fn local_account()->Option<(String,String)>{
    let (claims,_)=browser_token()?;
    Some((slot(&claims.user),"Trae CN".into()))
}
pub struct Connected{pub slot:String,pub label:String,pub snapshot:Snapshot}
pub struct Connect{rx:mpsc::Receiver<Result<Connected,String>>}
impl Connect {
    pub fn start(alias:String,manual:Option<String>,expected:Option<String>)->Self{
        let(tx,rx)=mpsc::channel();
        std::thread::spawn(move||{let result=(||{
            let (claims,token)=match manual{
                Some(raw)=>{let token=token(&raw)?;let claims=claims(&token).ok_or_else(||"Trae CN 令牌无法解析，请重新复制".to_string())?;(claims,token)}
                None=>browser_token().ok_or_else(||MISSING_ERROR.to_string())?,
            };
            if claims.exp<=chrono::Utc::now().timestamp(){return Err("Trae CN 令牌已过期，请在浏览器重新登录后再连接".to_string());}
            let id=slot(&claims.user);
            if expected.as_deref().is_some_and(|slot|slot!=id){return Err("当前浏览器是另一个 Trae CN 账号，请切回原账号后重新连接".into());}
            let snapshot=request(&super::http_agent(),&token).map_err(|error|error.0)?;
            save(&Session{token,user:claims.user})?;
            let label=if alias.trim().is_empty(){"Trae CN".into()}else{alias.trim().to_owned()};
            Ok(Connected{slot:id,label,snapshot})
        })();let _=tx.send(result);});Self{rx}
    }
    pub fn poll(&self)->Option<Result<Connected,String>>{self.rx.try_recv().ok()}
}
#[cfg(test)]
mod tests{
    use super::*;
    fn token_for(exp:i64)->String{
        let payload=json!({"data":{"id":"1757457654759299","type":"user"},"exp":exp,"iat":exp-86400}).to_string();
        format!("{JWT_HEAD}{}.c2ln",URL_SAFE_NO_PAD.encode(payload))
    }
    #[test]fn trae_reads_credits_plan_and_cycle(){
        let now=chrono::Utc::now().timestamp();let reset=now+86400*10;
        let value=json!({"is_credits_billing":true,"usage_summary":{"consumed_amount":171.77,"consumption_ratio":0.0554,"total_amount":3100},
            "user_entitlement_pack_list":[{"display_desc":"免费","status":1,"expire_time":reset,"is_hide":false,"entitlement_base_info":{"quota":{}}},{"display_desc":"签到奖励","status":1,"expire_time":reset+3600,"is_hide":false}]});
        let snapshot=parse(&value).unwrap();let credits=snapshot.credits.clone().unwrap();
        assert_eq!(snapshot.plan,"免费");assert_eq!(credits.remaining,2928.23);assert_eq!(credits.packages[0],("总额".to_string(),3100.0));assert!(credits.cycle.contains("重置"));
        assert_eq!(snapshot.windows.len(),0);
    }
    #[test]fn trae_falls_back_to_pack_quota_and_rejects_broken_payloads(){
        let value=json!({"user_entitlement_pack_list":[{"display_desc":"Pro","status":1,"expire_time":0,"is_hide":false,"entitlement_base_info":{"quota":{"credits_limit":100}},"usage":{"credits_amount":30}}]});
        let credits=parse(&value).unwrap().credits.unwrap();assert_eq!(credits.remaining,70.0);
        for bad in [json!({}),json!({"user_entitlement_pack_list":[]}),json!({"code":1001,"user_entitlement_pack_list":[]}),json!({"usage_summary":{"total_amount":0},"user_entitlement_pack_list":[]})]{
            assert!(parse(&bad).is_err(),"无效数据不能显示成 0 积分");
        }
        assert!(parse(&json!({"usage_summary":{"total_amount":-1,"consumed_amount":0},"user_entitlement_pack_list":[]})).is_err());
    }
    #[test]fn trae_tokens_are_validated_before_use(){
        let valid=token_for(chrono::Utc::now().timestamp()+600);
        assert_eq!(slot(&claims(&valid).unwrap().user).len(),64);
        assert!(token(&format!("Cloud-IDE-JWT {valid}")).is_ok());
        assert!(token("short").is_err());assert!(token("").is_err());
        assert!(claims("a.b.c").is_none());
        let mut parts=valid.split('.');let header=parts.next().unwrap();let payload=parts.next().unwrap();
        assert!(claims(&format!("{header}.{payload}")).is_none(),"缺签名段应拒绝");
        assert!(claims(&format!("{header}.{payload}.bad!sig")).is_none());
    }
    #[test]
    #[ignore="只读核验本机浏览器里的 Trae CN 令牌与积分，不输出令牌"]
    fn trae_live_read_only(){
        let (claims,token)=browser_token().expect("本机浏览器里应有已登录的 Trae CN 令牌");
        let snapshot=request(&super::super::http_agent(),&token).map_err(|error|error.0).expect("查询 Trae CN 积分");
        let credits=snapshot.credits.expect("应返回积分");
        println!("user={} plan={} remaining={} cycle={} detail={:?}",claims.user,snapshot.plan,credits.remaining,credits.cycle,credits.packages);
        assert!(credits.remaining>=0.0);
    }
    #[test]fn trae_local_storage_tokens_are_extracted_from_binary_pages(){
        let token=token_for(chrono::Utc::now().timestamp()+600);
        let mut page=vec![0u8,1,2,255,254];
        page.extend_from_slice(format!("_https://www.trae.cn\u{0}\u{1}Cloud-IDE-Token\u{0}\u{1}").as_bytes());
        page.extend_from_slice(token.as_bytes());page.extend_from_slice(b"\x00\x01other\x00");
        let found=tokens_in(&page);assert_eq!(found,vec![token]);
        assert!(tokens_in(b"Cloud-IDE-Token without a jwt").is_empty());
    }
}
