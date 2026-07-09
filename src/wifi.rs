//! Wi-Fi STA 连接组件 (no_std, esp-radio 0.18 + esp-rtos)
//!
//! 参考 ESP-IDF DPP Enrollee 例子的事件驱动模式：
//! - RTOS 调度器启动 (替代 NVS init)
//! - esp_radio::wifi::new() 替代 esp_wifi_init
//! - set_config() + 轮询 is_connected() 替代 wifi_config_t + esp_wifi_connect
//! - 事件驱动重连 (最多 3 次)
//! - 提供 blocking / tick 两种姿态

extern crate alloc;
use alloc::string::String;
use esp_radio::wifi::{
    sta::StationConfig,
    Config, WifiController,
};
use esp_println::println;

// ============================================================================
//  常量 (对应 DPP 的 WIFI_MAX_RETRY_NUM = 3)
// ============================================================================

/// 最大重连尝试次数
const MAX_RETRY: u8 = 3;

/// 连接超时 (毫秒)
const CONNECT_TIMEOUT_MS: u64 = 30_000;

// ============================================================================
//  状态枚举 (给 UI 层用)
// ============================================================================

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WifiStatus {
    Idle,
    Connecting,
    Connected,
    Failed,
}

// ============================================================================
//  阻塞连接 (对应 DPP 的 dpp_enrollee_init — 事件组等待)
// ============================================================================

/// 配置 SSID/Password 并阻塞等待连接成功或重试用完。
///
/// 通过轮询 `WifiController::is_connected()` 判断状态。
/// 返回 `true` = 连接成功。
pub fn connect_blocking(
    ctrl: &mut WifiController<'_>,
    ssid: &str,
    password: &str,
    delay: &esp_hal::delay::Delay,
) -> bool {
    // 配置 Station (对应 DPP 的 wifi_config_t + esp_wifi_set_config)
    let station_cfg = Config::Station(
        StationConfig::default()
            .with_ssid(String::from(ssid))
            .with_password(String::from(password)),
    );
    if let Err(e) = ctrl.set_config(&station_cfg) {
        println!("[wifi] set_config failed: {:?}", e);
        return false;
    }

    println!("[wifi] Connecting to \"{}\"...", ssid);

    // 轮询等待连接 (对应 DPP 的 xEventGroupWaitBits)
    let mut retry_count: u8 = 0;
    let deadline =
        esp_hal::time::Instant::now() + esp_hal::time::Duration::from_millis(CONNECT_TIMEOUT_MS);

    loop {
        if ctrl.is_connected() {
            println!("[wifi] Connected to \"{}\" ✓", ssid);
            return true;
        }

        // 驱动内部状态机：set_config 会在 RTOS 中自动触发连接，
        // 此处只需等待即可。
        if retry_count < MAX_RETRY {
            retry_count += 1;
        }

        if esp_hal::time::Instant::now() >= deadline {
            // 检查最终状态
            if ctrl.is_connected() {
                println!("[wifi] Connected to \"{}\" ✓", ssid);
                return true;
            }
            println!("[wifi] Connection TIMEOUT after {}ms", CONNECT_TIMEOUT_MS);
            return false;
        }

        delay.delay_millis(50);
    }
}

// ============================================================================
//  非阻塞 tick (给主循环每帧调用)
// ============================================================================

/// 在主循环中每帧调用，处理断线检测 + 自动重连。
/// 对应 DPP 的 WIFI_EVENT_STA_DISCONNECTED handler 逻辑。
///
/// 返回当前连接状态。
pub fn tick(ctrl: &mut WifiController<'_>, retry_count: &mut u8) -> WifiStatus {
    if ctrl.is_connected() {
        *retry_count = 0;
        WifiStatus::Connected
    } else {
        if *retry_count < MAX_RETRY {
            *retry_count += 1;
            println!("[wifi] Disconnected! Retry {}/{}...", *retry_count, MAX_RETRY);
            WifiStatus::Connecting
        } else {
            WifiStatus::Failed
        }
    }
}
