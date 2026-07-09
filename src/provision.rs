//! SoftAP 配网模块

extern crate alloc;
use alloc::string::String;
use alloc::vec::Vec;

use esp_radio::wifi::{ap::AccessPointConfig, Config, WifiController, Ssid};
use esp_println::println;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ProvisionState {
    Idle,
    ApActive,
    GotCreds,
    Done,
    Failed,
}

pub struct WifiCreds {
    pub ssid: String,
    pub password: String,
}

pub struct Provisioning {
    pub state: ProvisionState,
    pub creds: Option<WifiCreds>,
    pub ap_ssid: &'static str,
    pub ap_password: &'static str,
    tick_ms: u32,
}

const AP_SSID: &str = "u8gg-Config";
const AP_PASSWORD: &str = "12345678";

impl Provisioning {
    pub fn new() -> Self {
        Self {
            state: ProvisionState::Idle,
            creds: None,
            ap_ssid: AP_SSID,
            ap_password: AP_PASSWORD,
            tick_ms: 0,
        }
    }

    /// 开启 AP 配网模式
    pub fn start_ap(&mut self, ctrl: &mut WifiController<'_>) {
        let ap_cfg = Config::AccessPoint(
            AccessPointConfig::default()
                .with_ssid(Ssid::from(AP_SSID)),
        );
        let _ = ctrl.set_config(&ap_cfg);
        self.state = ProvisionState::ApActive;
        self.tick_ms = 0;
        println!("[provision] AP mode: ssid=\"{}\"", AP_SSID);
    }

    /// 停止 AP
    pub fn stop_ap(&mut self, ctrl: &mut WifiController<'_>) {
        let sta_cfg = Config::Station(Default::default());
        let _ = ctrl.set_config(&sta_cfg);
        self.state = ProvisionState::Idle;
        println!("[provision] AP stopped");
    }

    /// 保存收到的凭据
    pub fn set_creds(&mut self, ssid: &str, password: &str) {
        let mut c = WifiCreds {
            ssid: String::new(),
            password: String::new(),
        };
        c.ssid.push_str(ssid);
        c.password.push_str(password);
        self.creds = Some(c);
        self.state = ProvisionState::GotCreds;
        println!("[provision] Got creds: ssid=\"{}\"", ssid);
    }

    pub fn has_creds(&self) -> bool {
        self.creds.is_some()
    }

    /// 驱动 AP (定时器)
    pub fn tick(&mut self, dt_ms: u32) {
        self.tick_ms = self.tick_ms.wrapping_add(dt_ms);
    }
}
