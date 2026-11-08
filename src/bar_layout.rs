//! 常驻组件按实际占位排布，并在开关变化时连续补位。
use crate::anim::{smoothstep, Spring};

pub(crate) const CLOCK: usize = 0;
pub(crate) const BATTERY: usize = 1;
pub(crate) const QUOTA: usize = 2;
pub(crate) fn quota_slot(position:usize)->usize {
    if position<3 {QUOTA+position} else {WIDTHS.len()+position-3}
}
/// 系统监控环（CPU / 内存 / GPU），固定排在额度环之前。
pub(crate) const SYSMON: usize = 6;
pub(crate) fn sysmon_slot(position:usize)->usize { SYSMON + position }
pub(crate) const SYSMON_COUNT: usize = 3;
/// 收起态监控环的排布依据：`shown` 为勾选，`readable` 为此刻是否读得到。
/// 只有「勾选且读得到」的环占位；读不到的环自动让出槽位，组宽随之收缩。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) struct SysmonSpec {
    pub shown: [bool; 3],
    pub readable: [bool; 3],
}
impl SysmonSpec {
    /// 三个读数都显示、都就读（旧版行为）。
    pub(crate) const ALL: Self = Self { shown: [true; 3], readable: [true; 3] };
    /// 该环是否占据槽位：勾选了、并且此刻确实读得到。
    ///
    /// 勾选却读不到（例如没有 GPU 计数器）时不占位，监控组随之自动缩进；
    /// 读数恢复后环会重新出现，其它组件位置不跳动。
    pub(crate) fn visible(&self, index: usize) -> bool {
        self.shown.get(index).copied().unwrap_or(false) && self.readable.get(index).copied().unwrap_or(false)
    }
    /// `size` 个环、间距 34 时每个环中心相对组左缘的位置。
    pub(crate) fn centers(&self, size: usize) -> Vec<f32> {
        (0..size.min(SYSMON_COUNT)).filter(|i| self.visible(*i)).enumerate().map(|(k, _)| 13.5 + k as f32 * 34.0).collect()
    }
}
pub(crate) const MUSIC: usize = 9;
pub(crate) const FOCUS: usize = 10;
pub(crate) const BELL: usize = 11;
pub(crate) const PET: usize = 12;
const WIDTHS: [f32; 13] = [44.0,20.0,27.0,27.0,27.0,22.0,27.0,27.0,27.0,14.0,crate::focus_bar::WIDTH,20.0,32.0];

/// 可拖动的常驻组：电量、吉祥物、时间、系统监控、AI 额度、音乐、专注。
pub(crate) const ORDER_SLOTS: [usize;7]=[BATTERY,PET,CLOCK,SYSMON,QUOTA,MUSIC,FOCUS];
pub(crate) const DEFAULT_ORDER: [usize;7]=[BATTERY,PET,CLOCK,SYSMON,QUOTA,MUSIC,FOCUS];
pub(crate) fn valid_order(order:&[usize])->bool {
    order.len()==ORDER_SLOTS.len() && ORDER_SLOTS.iter().all(|slot|order.iter().filter(|value|*value==slot).count()==1)
}
/// 兼容旧版 6 项顺序：补入系统监控（插在额度之前）。
pub(crate) fn normalize_order(order:&[usize])->Option<Vec<usize>> {
    if valid_order(order) { return Some(order.to_vec()); }
    if order.len()==6 && order.iter().all(|s|ORDER_SLOTS.contains(s)) && order.iter().collect::<std::collections::HashSet<_>>().len()==6 {
        let mut out=order.to_vec();
        let at=out.iter().position(|s|*s==QUOTA).unwrap_or(out.len());
        out.insert(at,SYSMON);
        return Some(out);
    }
    None
}

pub(crate) fn animations_enabled()->bool {
    #[cfg(windows)] {
        static ENABLED:std::sync::OnceLock<bool>=std::sync::OnceLock::new();
        *ENABLED.get_or_init(||{
            use windows::Win32::{Foundation::BOOL,UI::WindowsAndMessaging::*};
            let mut enabled=BOOL(1);
            unsafe {let _=SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION,0,Some((&mut enabled as *mut BOOL).cast()),SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));}
            enabled.as_bool()
        })
    }
    #[cfg(not(windows))] {true}
}

#[derive(Clone, Debug)]
pub(crate) struct Layout {
    pub width: f32,
    pub x: Vec<Option<f32>>,
    pub widths:Vec<f32>,
    quotas:usize,
}

impl Layout {
    pub fn new(battery:bool,quotas:usize,music:bool,focus:bool,bell:bool,pet:bool)->Self {
        Self::with_sysmon(battery,quotas,false,music,focus,bell,pet)
    }

    /// `sysmon` 为真时显示 CPU / 内存 / GPU 三个环，作为独立可拖动组排在额度环之前。
    pub fn with_sysmon(battery:bool,quotas:usize,sysmon:bool,music:bool,focus:bool,bell:bool,pet:bool)->Self {
        Self::with_sysmon_spec(battery,quotas,if sysmon {SysmonSpec::ALL}else{SysmonSpec::default()},music,focus,bell,pet)
    }

    /// 按设置里勾选的读数（CPU / 内存 / GPU 任意组合）排布系统监控环。
    pub fn with_sysmon_spec(battery:bool,quotas:usize,sysmon:SysmonSpec,music:bool,focus:bool,bell:bool,pet:bool)->Self {
        fn place(cursor:&mut f32,width:f32)->f32 {let x=*cursor+width/2.0;*cursor+=width+12.0;x}
        let mut widths=WIDTHS.to_vec(); widths.resize(WIDTHS.len()+quotas.saturating_sub(3),27.0);
        let mut x=vec![None;widths.len()];let mut cursor=16.0;
        let count=quotas;
        // 只按真正占位的环数量排布：少一个环，组宽就少一截，其它组件自动补位。
        let sysmon_count=(0..SYSMON_COUNT).filter(|i|sysmon.visible(*i)).count();
        let ring_width=|n:usize| n as f32*27.0+n.saturating_sub(1) as f32*7.0;
        // 默认顺序：电量、吉祥物、时间、系统监控、额度、音乐、专注。无电池时额度提到最前。
        // 无电池时额度（含监控环）提前补位到时钟左侧。
        let order: &[usize] = if battery { &DEFAULT_ORDER } else { &[SYSMON,QUOTA,CLOCK,PET,MUSIC,FOCUS,BATTERY] };
        for &slot in order {
            match slot {
                BATTERY if battery=>x[slot]=Some(place(&mut cursor,WIDTHS[slot])),
                PET if pet=>x[slot]=Some(place(&mut cursor,WIDTHS[PET])),
                CLOCK=>x[slot]=Some(place(&mut cursor,WIDTHS[slot])),
                SYSMON if sysmon_count>0=>{
                    let total=ring_width(sysmon_count);
                    let left=place(&mut cursor,total)-total/2.0;
                    let centers=sysmon.centers(SYSMON_COUNT);
                    for (i,cx) in (0..SYSMON_COUNT).filter(|i|sysmon.visible(*i)).zip(centers) {x[sysmon_slot(i)]=Some(left+cx);}
                },
                QUOTA if count>0=>{
                    let total=ring_width(count);
                    let left=place(&mut cursor,total)-total/2.0;
                    let mut cx=left+13.5;
                    for i in 0..count {x[quota_slot(i)]=Some(cx);cx+=34.0;}
                },
                MUSIC if music&&!bell=>x[slot]=Some(place(&mut cursor,WIDTHS[slot])),
                FOCUS if focus=>x[slot]=Some(place(&mut cursor,WIDTHS[slot])),
                _=>{},
            }
        }
        if bell {x[BELL]=Some(place(&mut cursor,WIDTHS[BELL]));}
        let width=cursor-12.0+16.0;
        for value in x.iter_mut().flatten() {*value-=width/2.0;}
        Self {width:width.max(96.0),x,widths,quotas}
    }
    pub fn quota_slots(&self)->impl Iterator<Item=usize> { (0..self.quotas).map(quota_slot) }
    /// 全部圆环槽位（系统监控环 + 额度环），用于时钟淡出与重排。
    pub fn ring_slots(&self)->Vec<usize> {
        let mut out:Vec<usize>=(0..SYSMON_COUNT).map(sysmon_slot).filter(|i|self.x[*i].is_some()).collect();
        out.extend(self.quota_slots().filter(|i|self.x[*i].is_some()));
        out
    }
    pub fn balance_tail(&mut self,position:usize,extra:f32){
        let slot=quota_slot(position);
        let Some(center)=self.x[slot] else{return;};
        for (i,x) in self.x.iter_mut().enumerate(){if i!=slot{if let Some(x)=x{*x+=if *x<center{-extra/2.0}else{extra/2.0};}}}
        self.widths[slot]+=extra;self.width+=extra;
    }
    pub fn widen_clock(&mut self, extra:f32) {
        let clock=self.x[CLOCK].unwrap();
        for (slot,x) in self.x.iter_mut().enumerate() {
            if slot!=CLOCK { if let Some(x)=x { *x+=if *x<clock {-extra/2.0}else{extra/2.0}; } }
        }
        self.width+=extra;
    }
    pub fn reorder(&mut self,order:&[usize]) {
        if !valid_order(order){return;}
        let original=self.x.clone();let mut cursor=-self.width/2.0+16.0;
        for slot in order.iter().copied().chain(std::iter::once(BELL)) {
            // 系统监控环与额度环各自成组，可独立拖动。
            let slots:Vec<usize>=match slot {
                SYSMON=>(0..SYSMON_COUNT).map(sysmon_slot).filter(|i|original[*i].is_some()).collect(),
                QUOTA=>self.quota_slots().collect(),
                _=>vec![slot],
            };
            let left=slots.iter().filter_map(|&i|original[i].map(|x|x-self.widths[i]/2.0)).fold(f32::INFINITY,f32::min);
            let right=slots.iter().filter_map(|&i|original[i].map(|x|x+self.widths[i]/2.0)).fold(f32::NEG_INFINITY,f32::max);
            if !left.is_finite(){continue;}
            for i in slots {if let Some(x)=original[i]{self.x[i]=Some(cursor+x-left);}}
            cursor+=right-left+12.0;
        }
    }
    /// 圆环换边经过时钟时，暂时淡出时钟，避免移动中相互遮挡。
    pub fn clearance(&self,slot:usize)->f32 {
        let Some(x)=self.x[slot] else{return 0.0;};
        let distance=self.ring_slots().into_iter().filter_map(|i|self.x[i].map(|other|(other-x).abs()-(self.widths[i]-27.0)/2.0)).fold(f32::INFINITY,f32::min);
        let quota=if slot==CLOCK {smoothstep(34.0,44.0,distance)}else{smoothstep(22.0,34.0,distance)};
        quota*self.pet_clearance(slot)
    }
    pub fn pet_clearance(&self,slot:usize)->f32 {
        match (self.x[slot],self.x[PET]) {
            (Some(x),Some(pet))=>smoothstep(self.widths[slot]/2.0+14.0,self.widths[slot]/2.0+24.0,(x-pet).abs()),
            _=>1.0,
        }
    }
}

pub(crate) struct Motion {
    pub target: Layout,
    positions: Vec<Spring>,
    initialized: bool,
}
impl Default for Motion {
    fn default()->Self {Self {target:Layout::new(false,0,false,false,false,false),positions:vec![Spring::new(0.0,360.0,0.76);13],initialized:false}}
}
impl Motion {
    pub fn step(&mut self,target:Layout,dt:f32,animate:bool) {
        self.positions.resize(target.x.len(),Spring::new(0.0,360.0,0.76));
        for (i,x) in target.x.iter().enumerate() {
            if let Some(x)=x {
                let spring=&mut self.positions[i];spring.target=*x;
                if !self.initialized || self.target.x.get(i).copied().flatten().is_none() || !animate {spring.snap(*x);}else{spring.step(dt);}
            }
        }
        self.target=target;self.initialized=true;
    }
    pub fn current(&self)->Layout {
        let mut layout=self.target.clone();
        for (i,x) in layout.x.iter_mut().enumerate() {if x.is_some(){*x=Some(self.positions[i].pos);}}
        layout
    }
    pub fn position(&self,slot:usize)->f32 {self.positions[slot].pos}
    pub fn settled(&self)->bool {self.target.x.iter().enumerate().all(|(i,x)|x.is_none()||self.positions[i].settled(100.0))}
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn deepseek_balance_follows_more_and_never_overlaps(){
        for order in [DEFAULT_ORDER,[FOCUS,MUSIC,SYSMON,QUOTA,CLOCK,PET,BATTERY]] {for quotas in 1..=6 {for battery in [false,true]{
            let mut layout=Layout::with_sysmon(battery,quotas,true,true,true,false,true);layout.reorder(&order);layout.balance_tail(quotas-1,76.0);layout.widen_clock(40.0);
            let mut boxes:Vec<_>=layout.x.iter().enumerate().filter_map(|(i,x)|x.map(|x|{let width=layout.widths[i]+if i==CLOCK{40.0}else{0.0};(x-width/2.0,x+width/2.0)})).collect();boxes.sort_by(|a,b|a.0.total_cmp(&b.0));
            assert!(boxes.windows(2).all(|p|p[1].0-p[0].1>=6.9));assert!(boxes.last().unwrap().1<=layout.width/2.0-15.9);
            assert!(layout.width<crate::app::WIN_W);
        }}}
    }
    #[test]
    fn sysmon_spec_places_only_selected_rings(){
        // 任意组合：只有勾选的环占位，组宽随之变化，且互不重叠。
        for bits in 1u8..8 {
            let shown=[bits&1!=0,bits&2!=0,bits&4!=0];
            let spec=SysmonSpec{shown,readable:[true;3]};
            let n=shown.iter().filter(|on|**on).count();
            for quotas in 0..=4 {for battery in [false,true] {
                let mut layout=Layout::with_sysmon_spec(battery,quotas,spec,true,true,false,true);
                layout.reorder(&DEFAULT_ORDER);
                let rings:Vec<_>=(0..SYSMON_COUNT).filter(|i|spec.visible(*i)).map(|i|layout.x[sysmon_slot(i)]).collect();
                assert_eq!(rings.iter().filter(|x|x.is_some()).count(),n);
                let mut boxes:Vec<_>=layout.x.iter().enumerate().filter_map(|(i,x)|x.map(|x|(x-layout.widths[i]/2.0,x+layout.widths[i]/2.0))).collect();
                boxes.sort_by(|a,b|a.0.total_cmp(&b.0));
                assert!(boxes.windows(2).all(|p|p[1].0-p[0].1>=6.9),"组合 {bits} 的圆环挤在一起");
            }}
        }
        // 一个都不勾：监控组不占位。
        let layout=Layout::with_sysmon_spec(true,2,SysmonSpec{shown:[false;3],readable:[true;3]},true,true,false,true);
        assert!((0..SYSMON_COUNT).all(|i|layout.x[sysmon_slot(i)].is_none()));
        assert_eq!(SysmonSpec::ALL.centers(3),vec![13.5,47.5,81.5]);
        assert_eq!(SysmonSpec{shown:[true,false,true],readable:[true;3]}.centers(3),vec![13.5,47.5]);
    }

    #[test]
    fn unreadable_rings_release_their_slots_and_shrink_the_monitor_group(){
        // 勾选但读不到的环不再占位：组宽与岛宽都随之收缩，不会留下空槽。
        let all=Layout::with_sysmon_spec(true,2,SysmonSpec{shown:[true;3],readable:[true;3]},true,true,false,true);
        for readable in [[true,true,false],[true,false,false],[false,false,false]] {
            let spec=SysmonSpec{shown:[true;3],readable};
            let layout=Layout::with_sysmon_spec(true,2,spec,true,true,false,true);
            let expected=readable.iter().filter(|r|**r).count();
            let occupied=(0..SYSMON_COUNT).filter(|i|layout.x[sysmon_slot(*i)].is_some()).count();
            assert_eq!(occupied,expected,"读不到的环不应占位：{readable:?}");
            assert!(layout.width<all.width,"少一个环就该窄一点：{readable:?}");
        }
        // 全部读不到：监控组整体让位，宽度等于完全不设监控组。
        let none=Layout::with_sysmon_spec(true,2,SysmonSpec{shown:[true;3],readable:[false;3]},true,true,false,true);
        let off=Layout::with_sysmon_spec(true,2,SysmonSpec::default(),true,true,false,true);
        assert_eq!(none.width,off.width);
        assert_eq!(none.x,off.x);
        // 只剩一个可读：那个环左对齐到组首，中间不留空位。
        let one=Layout::with_sysmon_spec(true,2,SysmonSpec{shown:[true;3],readable:[false,false,true]},true,true,false,true);
        let others:Vec<f32>=one.x.iter().enumerate().filter(|(i,_)|!((SYSMON..SYSMON+SYSMON_COUNT).contains(i))).filter_map(|(_,x)|*x).collect();
        let ring=one.x[sysmon_slot(2)].expect("GPU 可读时应占位");
        assert!(others.iter().all(|x|(*x-ring).abs()>1.0),"唯一的环不应叠在别的组件上");
    }

    #[test]
    fn clock_12h_keeps_components_separate_in_custom_orders(){
        for order in [DEFAULT_ORDER,[FOCUS,MUSIC,SYSMON,QUOTA,CLOCK,PET,BATTERY]] {
            for quotas in 0..=5 {
                let mut layout=Layout::with_sysmon(true,quotas,true,true,true,false,true);
                layout.reorder(&order);layout.widen_clock(40.0);
                let mut boxes:Vec<_>=layout.x.iter().enumerate().filter_map(|(i,x)|x.map(|x|{
                    let width=layout.widths[i]+if i==CLOCK {40.0}else{0.0};
                    (x-width/2.0,x+width/2.0)
                })).collect();
                boxes.sort_by(|a,b|a.0.total_cmp(&b.0));
                assert!(boxes.windows(2).all(|p|p[1].0-p[0].1>=6.9));
                assert!(boxes.first().unwrap().0>=-layout.width/2.0+15.9);
                assert!(boxes.last().unwrap().1<=layout.width/2.0-15.9);
            }
        }
    }
    #[test]
    fn custom_order_preserves_groups_and_hidden_positions(){
        assert!(!valid_order(&[0,0,2,6,7,9]));
        assert!(!valid_order(&[0,1,2]));
        // 旧版 6 项顺序应被规整为 7 项，且监控插在额度之前。
        let migrated=normalize_order(&[BATTERY,PET,CLOCK,QUOTA,MUSIC,FOCUS]).expect("旧顺序应可迁移");
        assert_eq!(migrated.len(),7);
        let at=migrated.iter().position(|s|*s==SYSMON).unwrap();
        let quota=migrated.iter().position(|s|*s==QUOTA).unwrap();
        assert_eq!(at+1,quota,"系统监控应排在额度之前");
        assert!(valid_order(&migrated));
        // 已是 7 项的保持原样；非法值返回 None。
        assert_eq!(normalize_order(&DEFAULT_ORDER).as_deref(),Some(&DEFAULT_ORDER[..]));
        assert!(normalize_order(&[0,0,2]).is_none());
        let order=[FOCUS,MUSIC,SYSMON,QUOTA,CLOCK,PET,BATTERY];
        for battery in [false,true]{for count in 0..=5 {
            let mut layout=Layout::with_sysmon(battery,count,true,true,true,false,true);
            let width=layout.width;layout.reorder(&order);
            assert_eq!(layout.width,width);
            let positions:Vec<_>=order.iter().filter_map(|i|layout.x[*i]).collect();
            assert!(positions.windows(2).all(|p|p[0]<p[1]));
            if count>1 {assert_eq!(layout.x[QUOTA+1].unwrap()-layout.x[QUOTA].unwrap(),34.0);}
            let mut boxes:Vec<_>=layout.x.iter().enumerate().filter_map(|(i,x)|x.map(|x|(x-layout.widths[i]/2.0,x+layout.widths[i]/2.0))).collect();
            boxes.sort_by(|a,b|a.0.total_cmp(&b.0));
            assert!(boxes.windows(2).all(|p|p[1].0-p[0].1>=6.9));
        }}
    }
    #[test]
    fn every_component_combination_fits_without_overlaps(){
        for quotas in 0..=8 {for battery in [false,true] {for music in [false,true] {for focus in [false,true] {for bell in [false,true] {for pet in [false,true] {
            let layout=Layout::new(battery,quotas,music,focus,bell,pet);
            let mut boxes:Vec<_>=layout.x.iter().enumerate().filter_map(|(i,x)|x.map(|x|(x-layout.widths[i]/2.0,x+layout.widths[i]/2.0))).collect();
            boxes.sort_by(|a,b|a.0.total_cmp(&b.0));
            assert_eq!(layout.quota_slots().count(),quotas);
            assert!(boxes.first().unwrap().0>=-layout.width/2.0+15.9);
            assert!(boxes.last().unwrap().1<=layout.width/2.0-15.9);
            assert!(boxes.windows(2).all(|pair|pair[1].0-pair[0].1>=6.9),"组件不能挤在一起：{layout:?}");
            assert_eq!(layout.clearance(CLOCK),1.0);
        }}}}}}
    }
    #[test]
    fn replacing_battery_moves_continuously_and_supports_an_instant_state(){
        let before=Layout::new(true,1,true,false,false,true);let after=Layout::new(false,1,true,false,false,true);
        let mut motion=Motion::default();motion.step(before.clone(),0.0,true);
        motion.step(after.clone(),1.0/60.0,true);let mid=motion.current().x[QUOTA].unwrap();
        assert!(mid<before.x[QUOTA].unwrap()&&mid>after.x[QUOTA].unwrap());
        for _ in 0..120 {motion.step(after.clone(),1.0/60.0,true);}
        assert!((motion.current().x[QUOTA].unwrap()-after.x[QUOTA].unwrap()).abs()<0.01);
        motion.step(before.clone(),1.0/60.0,false);assert_eq!(motion.current().x,before.x);
    }
}
