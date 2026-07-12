#![allow(dead_code, unused_imports)]

use embedded_graphics::{pixelcolor::BinaryColor, prelude::*};
use esp_idf_svc::hal::gpio::{Input, PinDriver, Pull};
use esp_idf_svc::hal::i2c::{I2cConfig, I2cDriver};
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::log::EspLogger;
use esp_idf_svc::sys::link_patches;
use log::info;
use ssd1306::{prelude::*, I2CDisplayInterface, Ssd1306};

// ===== 模块 (源文件在 src/ 下) =====
#[path = "../menu.rs"]
mod menu;
#[path = "../ui.rs"]
mod ui;
#[path = "../viewmodel.rs"]
mod viewmodel;
#[path = "../wifi.rs"]
mod wifi;
#[path = "../qrcode.rs"]
mod qrcode;
// provision.rs 为未接线的死代码(引用已移除的 esp-radio/smoltcp)，不参与编译

use menu::*;
use ui::*;
use viewmodel::*;

// ===== ESP32-C3 引脚 (按 u8gg-c3 备份) =====
// K1=GPIO1, K2=GPIO2, K3=GPIO3, K4=GPIO4; OLED SDA=GPIO8, SCL=GPIO9

macro_rules! define_binds {
    ($($name:ident),+ $(,)?) => { define_binds!(@step 0usize, $($name),+); };
    (@step $n:expr, $first:ident, $($rest:ident),+ $(,)?) => {
        pub const $first: usize = $n;
        define_binds!(@step ($n) + 1, $($rest),+);
    };
    (@step $n:expr, $first:ident) => {
        pub const $first: usize = $n;
    };
}
define_binds! { FPS, SLIDER, TOGGLE, SHOW_QR, WIFI_MANUAL }

const WIFI_SSID: &str = "your_wifi_ssid";
const WIFI_PASS: &str = "your_wifi_password";

static PAGE_SUB: MenuPage = MenuPage {
    title: "子菜单",
    items: &[
        MenuItem::Submenu { label: "更深一层", page: &PAGE_DEEP },
        MenuItem::Submenu { label: "返回", page: &PAGE_MAIN },
    ],
};
static PAGE_DEEP: MenuPage = MenuPage {
    title: "深层",
    items: &[MenuItem::Submenu { label: "返回", page: &PAGE_SUB }],
};
static PAGE_ABOUT: MenuPage = MenuPage {
    title: "关于",
    items: &[
        MenuItem::Label { label: "u8gg v0.1" },
        MenuItem::Label { label: "ESP32-C3" },
        MenuItem::Label { label: "Apache-2.0" },
        MenuItem::Label { label: "Powered by Deepseek V4 Flash" },
    ],
};
static PAGE_MAIN: MenuPage = MenuPage {
    title: "主菜单",
    items: &[
        MenuItem::Submenu { label: "子菜单", page: &PAGE_SUB },
        MenuItem::Toggle { label: "FPS ", bind: FPS },
        MenuItem::Slider { label: "亮度", bind: SLIDER, min: 0, max: 255 },
        MenuItem::Toggle { label: "按键测试", bind: TOGGLE },
        MenuItem::Toggle { label: "WiFi QR", bind: SHOW_QR },
        MenuItem::Toggle { label: "手动配网", bind: WIFI_MANUAL },
        MenuItem::Submenu { label: "关于", page: &PAGE_ABOUT },
    ],
};

fn main() {
    // ESP-IDF 标准库初始化（提供 std 的 newlib 锁等）
    link_patches();
    EspLogger::initialize_default();
    info!("[u8gg] Boot OK");

    // 取出外设（Peripherals 为拥有权，必须按字段取出；modem 交给 WiFi）
    let mut peripherals = Peripherals::take().unwrap();
    let modem = peripherals.modem;
    let pins = peripherals.pins;
    let i2c0 = peripherals.i2c0;
    // 其余外设丢弃

    // ===== WiFi (esp-idf-svc) =====
    let mut wifi_ctl = wifi::init(modem).expect("wifi init failed");
    info!("[u8gg] WiFi initialized");
    let mut wifi_retry = 0u8;

    // ===== 按键 (GPIO1..4, 上拉, 低电平=按下) =====
    let k1 = PinDriver::input(pins.gpio1).unwrap();
    let k2 = PinDriver::input(pins.gpio2).unwrap();
    let k3 = PinDriver::input(pins.gpio3).unwrap();
    let k4 = PinDriver::input(pins.gpio4).unwrap();

    // ===== I2C (SDA=GPIO8, SCL=GPIO9) =====
    let mut i2c = I2cDriver::new(i2c0, pins.gpio8, pins.gpio9, &I2cConfig::new()).unwrap();

    let addr_ok = i2c.write(0x3D, &[0x00], 100).is_ok();
    info!("[u8gg] I2C probe 0x3D: {}", if addr_ok { "OK" } else { "FAIL" });
    if !addr_ok {
        let addr2_ok = i2c.write(0x3C, &[0x00], 100).is_ok();
        info!("[u8gg] I2C probe 0x3C: {}", if addr2_ok { "OK" } else { "FAIL" });
    }
    let iface = if addr_ok {
        I2CDisplayInterface::new_alternate_address(i2c)
    } else {
        I2CDisplayInterface::new(i2c)
    };
    let mut display = Ssd1306::new(iface, DisplaySize128x64, DisplayRotation::Rotate0)
        .into_buffered_graphics_mode();
    let mut oled_ok = false;
    for _ in 0..10 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        if display.init().is_ok() {
            info!("[u8gg] OLED init OK");
            oled_ok = true;
            break;
        }
    }
    if !oled_ok {
        info!("[u8gg] OLED init FAILED — will skip display");
    } else {
        display.clear(BinaryColor::Off).unwrap();
        let _ = display.flush();
    }

    let mut vm = ViewModel::new(&PAGE_MAIN);
    vm.menu.set_page(&PAGE_MAIN);
    vm.menu.dirty = true;
    vm.set_wifi(WIFI_SSID, WIFI_PASS);

    let mut canvas = menu::BufCanvas([0u8; 1024]);

    let mut tick_ms = 0u32;
    let mut fps_counter = 0u32;
    let mut last_fps_print = 0u32;
    let mut pk = [true; 4];
    let mut hold = [0u16; 4];

    loop {
        std::thread::sleep(std::time::Duration::from_millis(5));
        tick_ms = tick_ms.wrapping_add(5);
        fps_counter += 1;

        let c = [
            k1.is_low(),
            k2.is_low(),
            k3.is_low(),
            k4.is_low(),
        ];
        let edge = [c[0] && !pk[0], c[1] && !pk[1], c[2] && !pk[2], c[3] && !pk[3]];
        let mut long_edge = [false; 4];
        let mut repeat_up = false;
        let mut repeat_down = false;

        for i in 0..4 {
            if c[i] {
                hold[i] = hold[i].saturating_add(1);
            } else {
                hold[i] = 0;
            }
            let h = hold[i];
            if i < 2 {
                if h >= 66 {
                    let interval = if h < 90 { 4 } else if h < 120 { 3 } else { 2 };
                    if (h - 66) % interval == 0 {
                        if i == 0 {
                            repeat_up = true;
                        } else {
                            repeat_down = true;
                        }
                    }
                }
            } else if h == 66 {
                long_edge[i] = true;
            }
        }
        pk = c;

        vm.handle_key(
            edge[0] || repeat_up,
            edge[1] || repeat_down,
            edge[3],
            edge[2],
            long_edge[2],
            long_edge[3],
        );

        let wifi_status = wifi::tick(&mut wifi_ctl, &mut wifi_retry);
        if vm.consume_wifi_connect() {
            let ssid = vm.input_ssid();
            let pass = vm.input_pass();
            if !ssid.is_empty() {
                info!("[u8gg] WiFi connect to \"{}\"", ssid);
                let _ = wifi::connect(&mut wifi_ctl, ssid, pass);
                wifi_retry = 0;
            }
        }
        vm.set_wifi_status(wifi_status);
        vm.set_wifi_connected(wifi::is_connected(&wifi_ctl));

        if oled_ok {
            canvas.clear();
            vm.render(&mut canvas);
            ViewModel::blit(&canvas, &mut display);
            let _ = display.flush();
        }

        if tick_ms - last_fps_print >= 5000 {
            info!("[u8gg] FPS: {}", fps_counter * 1000 / (tick_ms - last_fps_print).max(1));
            fps_counter = 0;
            last_fps_print = tick_ms;
        }
    }
}
