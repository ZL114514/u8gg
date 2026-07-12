//! Wi-Fi STA 连接组件 (ESP-IDF std 版)
//!
//! 基于 esp-idf-svc 的 BlockingWifi/EspWifi。对外保留与旧 no_std 版本一致的
//! 接口：`init` / `connect` / `tick` / `is_connected` + `WifiStatus`，使
//! main.rs 的调用方式几乎不变。

use anyhow::Result;
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::modem::Modem;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::wifi::{
    AuthMethod, BlockingWifi, ClientConfiguration, Configuration, EspWifi,
};
use log::info;

const MAX_RETRY: u8 = 10;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WifiStatus {
    Idle,
    Connecting,
    Connected,
    Failed,
}

/// 封装 esp-idf-svc 的阻塞式 WiFi 句柄
pub struct WifiCtl {
    wifi: BlockingWifi<EspWifi<'static>>,
    started: bool,
}

/// 初始化 WiFi 驱动 (占用 modem / sysloop / nvs)，启动但不连接。
pub fn init(modem: Modem) -> Result<WifiCtl> {
    let sysloop = EspSystemEventLoop::take()?;
    let nvs = EspDefaultNvsPartition::take()?;

    let esp_wifi = EspWifi::new(modem, sysloop.clone(), Some(nvs))?;
    let mut wifi = BlockingWifi::wrap(esp_wifi, sysloop)?;

    wifi.set_configuration(&Configuration::Client(ClientConfiguration::default()))?;
    wifi.start()?;
    info!("[wifi] driver started");

    Ok(WifiCtl { wifi, started: true })
}

/// 配置 SSID/密码并尝试连接 (阻塞直到成功或失败)。
pub fn connect(ctl: &mut WifiCtl, ssid: &str, password: &str) -> Result<()> {
    let auth = if password.is_empty() {
        AuthMethod::None
    } else {
        AuthMethod::WPA2Personal
    };
    let client = ClientConfiguration {
        ssid: ssid.try_into().map_err(|_| anyhow::anyhow!("ssid too long"))?,
        password: password
            .try_into()
            .map_err(|_| anyhow::anyhow!("password too long"))?,
        auth_method: auth,
        ..Default::default()
    };
    ctl.wifi.set_configuration(&Configuration::Client(client))?;
    info!("[wifi] Connecting to \"{}\"...", ssid);
    ctl.wifi.connect()?;
    ctl.wifi.wait_netif_up()?;
    info!("[wifi] Connected ✓");
    Ok(())
}

/// 每帧调用：返回当前状态；断线时按 MAX_RETRY 计数并打印。
pub fn tick(ctl: &mut WifiCtl, retry_count: &mut u8) -> WifiStatus {
    if !ctl.started {
        return WifiStatus::Idle;
    }
    match ctl.wifi.is_connected() {
        Ok(true) => {
            *retry_count = 0;
            WifiStatus::Connected
        }
        _ => {
            if *retry_count < MAX_RETRY {
                *retry_count += 1;
                info!("[wifi] Disconnected! Retry {}/{}...", *retry_count, MAX_RETRY);
                WifiStatus::Connecting
            } else {
                WifiStatus::Failed
            }
        }
    }
}

/// 当前是否已连接。
pub fn is_connected(ctl: &WifiCtl) -> bool {
    ctl.wifi.is_connected().unwrap_or(false)
}
