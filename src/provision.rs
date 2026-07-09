//! SoftAP 配网模块 (方案 D)
//!
//! WifiPhy — smoltcp Device 桥接 (需要硬件上调试)
//! Provisioning — AP 模式 + 凭据接收状态机

extern crate alloc;

use esp_radio::wifi::{
    ap::AccessPointConfig, Config, WifiController, Ssid,
};
use core::task::{Context, RawWaker, RawWakerVTable, Waker};

// ================================================================
//  NOOP WAKER (给 embassy-net Driver 用)
// ================================================================

fn noop_waker() -> Waker {
    const VTABLE: RawWakerVTable = RawWakerVTable::new(
        |p| RawWaker::new(p, &VTABLE),
        |_| {}, |_| {}, |_| {},
    );
    let raw = RawWaker::new(core::ptr::null(), &VTABLE);
    unsafe { Waker::from_raw(raw) }
}

// ================================================================
//  WifiPhy — 桥接 esp-radio → smoltcp
//  编译时无法完全验证, 需要真实硬件
// ================================================================

pub struct WifiPhy<'a> {
    pub iface: &'a mut esp_radio::wifi::Interface<'a>,
}

// ================================================================
//  配网凭据
// ================================================================

pub struct WifiCreds {
    pub ssid: heapless::String<32>,
    pub password: heapless::String<64>,
    pub received: bool,
}

const AP_SSID: &str = "u8gg-Config";

pub struct Provisioning {
    pub creds: WifiCreds,
    pub started: bool,
    tick_ms: u32,
}

impl Provisioning {
    pub fn new() -> Self {
        Self {
            creds: WifiCreds {
                ssid: heapless::String::new(),
                password: heapless::String::new(),
                received: false,
            },
            started: false,
            tick_ms: 0,
        }
    }

    /// 开 AP 模式
    pub fn start(&mut self, ctrl: &mut WifiController<'_>) {
        let ap_cfg = Config::AccessPoint(
            AccessPointConfig::default()
                .with_ssid(Ssid::from(AP_SSID)),
        );
        let _ = ctrl.set_config(&ap_cfg);
        self.started = true;
    }

    /// 每帧调用
    pub fn tick(&mut self, dt_ms: u32) {
        self.tick_ms = self.tick_ms.wrapping_add(dt_ms);
    }

    pub fn has_creds(&self) -> bool { self.creds.received }
}
