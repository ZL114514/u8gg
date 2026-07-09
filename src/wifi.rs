//! Wi-Fi STA 连接组件
//!
//! 用 connect_async 连接并持续轮询其 future 推进状态机。
//! ctrl 存活于整个程序生命周期 (main 不返回), 故 safe 地 transmute 到 'static。

extern crate alloc;
use alloc::boxed::Box;
use alloc::string::String;

use core::{
    future::Future,
    pin::Pin,
    task::{Context, Poll, RawWaker, RawWakerVTable, Waker},
};
use esp_radio::wifi::{
    sta::StationConfig,
    Config, WifiController,
};
use esp_println::println;

// ============================================================================
//  NOOP WAKER
// ============================================================================

fn noop_waker() -> Waker {
    const VTABLE: RawWakerVTable = RawWakerVTable::new(
        |p| RawWaker::new(p, &VTABLE),
        |_| {}, |_| {}, |_| {},
    );
    let raw = RawWaker::new(core::ptr::null(), &VTABLE);
    unsafe { Waker::from_raw(raw) }
}

type BoxFut = Pin<Box<dyn Future<Output = Result<esp_radio::wifi::ConnectedStationInfo, esp_radio::wifi::WifiError>>>>;

// ============================================================================
//  常量
// ============================================================================

const MAX_RETRY: u8 = 10;

// ============================================================================
//  状态枚举
// ============================================================================

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WifiStatus {
    Idle,
    Connecting,
    Connected,
    Failed,
}

// ============================================================================
//  连接状态机
// ============================================================================

static mut CONNECT_FUT: Option<BoxFut> = None;

/// 配置并连接, 之后每帧调 tick() 推进。
pub fn connect(ctrl: &mut WifiController<'_>, ssid: &str, password: &str) {
    let station_cfg = Config::Station(
        StationConfig::default()
            .with_ssid(String::from(ssid))
            .with_password(String::from(password)),
    );
    if let Err(e) = ctrl.set_config(&station_cfg) {
        println!("[wifi] set_config failed: {:?}", e);
        return;
    }

    // SAFETY: ctrl 在 main() 中, 程序永不退出, 生命周期等价于 'static
    let ctrl_static: &'static mut WifiController<'static> = unsafe { core::mem::transmute(ctrl) };
    let fut = ctrl_static.connect_async();
    let mut boxed = Box::pin(fut);

    // 首次 poll 触发 connect_impl
    let w = noop_waker();
    let mut cx = Context::from_waker(&w);
    let _ = boxed.as_mut().poll(&mut cx);

    unsafe { CONNECT_FUT = Some(boxed); }
    println!("[wifi] Connecting to \"{}\"...", ssid);
}

/// 每帧调用, 轮询 future 推进连接状态机。
fn poll_fut() -> bool {
    // SAFETY: 单线程, 通过 raw pointer
    let p: *mut Option<BoxFut> = unsafe { core::ptr::addr_of_mut!(CONNECT_FUT) };
    let fut = unsafe { &mut *p };
    if let Some(f) = fut {
        let w = noop_waker();
        let mut cx = Context::from_waker(&w);
        match f.as_mut().poll(&mut cx) {
            Poll::Ready(Ok(info)) => {
                println!("[wifi] Connected ✓ (aid={})", info.aid);
                *fut = None;
                true
            }
            Poll::Ready(Err(e)) => {
                println!("[wifi] Connect failed: {:?}", e);
                *fut = None;
                true
            }
            Poll::Pending => false,
        }
    } else {
        false
    }
}

// ============================================================================
//  非阻塞 tick
// ============================================================================

pub fn tick(ctrl: &mut WifiController<'_>, retry_count: &mut u8) -> WifiStatus {
    let has_fut = unsafe { !(*core::ptr::addr_of!(CONNECT_FUT)).is_none() };
    if has_fut {
        poll_fut();
    }

    if ctrl.is_connected() {
        *retry_count = 0;
        unsafe { CONNECT_FUT = None; }
        WifiStatus::Connected
    } else if has_fut {
        WifiStatus::Connecting
    } else if *retry_count < MAX_RETRY {
        *retry_count += 1;
        println!("[wifi] Disconnected! Retry {}/{}...", *retry_count, MAX_RETRY);
        WifiStatus::Connecting
    } else {
        WifiStatus::Failed
    }
}
