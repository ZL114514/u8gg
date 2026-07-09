//! Wi-Fi STA 扫描 + 连接模块 (基于 esp-radio)
//!
//! scan: 同步式阻塞扫描（内部用 noop-waker 轮询）。
//! connect: 用 connect_async 启动连接，之后每帧调 tick() 推进状态机。
//! ctrl 存活于整个程序生命周期 (main 不返回), 故 safe 地 transmute 到 'static。

extern crate alloc;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use core::{
    future::Future,
    pin::Pin,
    task::{Context, Poll, RawWaker, RawWakerVTable, Waker},
};
use esp_println::println;
use esp_radio::wifi::{
    ap::AccessPointInfo,
    scan::ScanConfig,
    sta::StationConfig,
    Config, WifiController,
};

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
type BoxScanFut = Pin<Box<dyn Future<Output = Result<Vec<AccessPointInfo>, esp_radio::wifi::WifiError>>>>;

// ============================================================================
//  常量
// ============================================================================

const MAX_RETRY: u8 = 10;

// ============================================================================
//  扫描结果类型 (方便菜单显示)
// ============================================================================

#[derive(Clone, Debug)]
pub struct ScanResult {
    pub ssid: String,
    pub rssi: i8,
    pub channel: u8,
}

// ============================================================================
//  状态
// ============================================================================

static mut CONNECT_FUT: Option<BoxFut> = None;
static mut SCAN_FUT: Option<BoxScanFut> = None;
static mut SCAN_RESULTS: Option<Vec<AccessPointInfo>> = None;

// ============================================================================
//  扫描 (同步阻塞)
// ============================================================================

/// 执行一次 WiFi 扫描（阻塞式，内部 spin 轮询直到完成）。
/// 返回可用于菜单显示的 ScanResult 列表。
pub fn scan(ctrl: &mut WifiController<'_>) -> Vec<ScanResult> {
    // SAFETY: ctrl 在 main() 中, 程序永不退出
    let ctrl_static: &'static mut WifiController<'static> = unsafe { core::mem::transmute(ctrl) };

    // 确保先停止 connect_async
    unsafe {
        CONNECT_FUT = None;
        SCAN_RESULTS = None;
    }

    // Use a leaked allocation for static lifetime
    let scan_cfg: &'static ScanConfig = Box::leak(Box::new(ScanConfig::default()));
    let fut = ctrl_static.scan_async(scan_cfg);
    let boxed: BoxScanFut = Box::pin(fut);
    unsafe { SCAN_FUT = Some(boxed); }

    println!("[wifi] Scanning...");
    let w = noop_waker();
    let mut cx = Context::from_waker(&w);

    // 轮询直到完成
    loop {
        let p: *mut Option<BoxScanFut> = unsafe { core::ptr::addr_of_mut!(SCAN_FUT) };
        let fut = unsafe { &mut *p };
        if let Some(f) = fut {
            match f.as_mut().poll(&mut cx) {
                Poll::Ready(Ok(results)) => {
                    println!("[wifi] Scan found {} APs", results.len());
                    unsafe { SCAN_RESULTS = Some(results); }
                    *fut = None;
                    break;
                }
                Poll::Ready(Err(e)) => {
                    println!("[wifi] Scan failed: {:?}", e);
                    *fut = None;
                    break;
                }
                Poll::Pending => {
                    // spin
                    for _ in 0..2000 { core::hint::spin_loop(); }
                }
            }
        } else {
            break;
        }
    }

    // 转换结果 (avoid direct static mut ref — use raw ptr)
    let raw = unsafe {
        (*core::ptr::addr_of_mut!(SCAN_RESULTS))
            .take()
            .unwrap_or_default()
    };
    raw.into_iter()
        .map(|r| ScanResult {
            ssid: String::from(r.ssid.as_str()),
            rssi: r.signal_strength,
            channel: r.channel,
        })
        .collect()
}

// ============================================================================
//  连接
// ============================================================================

/// 发起 WiFi 连接 (异步, 之后每帧调 tick() 推进)
pub fn connect(ctrl: &mut WifiController<'_>, ssid: &str, password: &str) {
    let ctrl_static: &'static mut WifiController<'static> = unsafe { core::mem::transmute(ctrl) };

    let station_cfg = Config::Station(
        StationConfig::default()
            .with_ssid(String::from(ssid))
            .with_password(String::from(password)),
    );
    if let Err(e) = ctrl_static.set_config(&station_cfg) {
        println!("[wifi] set_config failed: {:?}", e);
        return;
    }

    let fut = ctrl_static.connect_async();
    let mut boxed: BoxFut = Box::pin(fut);
    // 立即 poll 一次 (首次调用会触发 scan)
    let mut w = noop_waker();
    let mut cx = Context::from_waker(&w);
    let _ = boxed.as_mut().poll(&mut cx);

    unsafe { CONNECT_FUT = Some(boxed); }
    println!("[wifi] Connecting to \"{}\"...", ssid);
}

// ============================================================================
//  状态机 tick
// ============================================================================

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WifiStatus {
    Idle,
    Connecting,
    Connected,
    Failed,
}

/// 每帧调用，推进连接异步状态机
pub fn tick(ctrl: &WifiController<'_>, retry_count: &mut u8) -> WifiStatus {
    let p: *mut Option<BoxFut> = unsafe { core::ptr::addr_of_mut!(CONNECT_FUT) };
    let fut = unsafe { &mut *p };

    let has_fut = fut.is_some();
    if let Some(f) = fut {
        let w = noop_waker();
        let mut cx = Context::from_waker(&w);
        match f.as_mut().poll(&mut cx) {
            Poll::Ready(Ok(_info)) => {
                println!("[wifi] Connected ✓");
                *fut = None;
            }
            Poll::Ready(Err(e)) => {
                println!("[wifi] Connect failed: {:?}", e);
                *fut = None;
            }
            Poll::Pending => {}
        }
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
