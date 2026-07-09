#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::String;
use core::cell::RefCell;
use embedded_graphics::{pixelcolor::BinaryColor, prelude::*};
use esp_backtrace as _;
use esp_hal::{
    clock::CpuClock,
    delay::Delay,
    gpio::{Input, InputConfig, Level, Output, OutputConfig, Pull},
    i2c::master::{Config as I2cConfig, I2c},
    interrupt::software::SoftwareInterruptControl,
    main,
    time::Rate,
    timer::timg::TimerGroup,
    Config,
};
use esp_println::println;
use ssd1306::{prelude::*, I2CDisplayInterface, Ssd1306};

// ===== 堆分配器 =====
esp_bootloader_esp_idf::esp_app_desc!();
#[unsafe(no_mangle)]
static mut HEAP: core::mem::MaybeUninit<[u8; 72 * 1024]> = core::mem::MaybeUninit::uninit();
unsafe fn init_heap() {
    esp_alloc::HEAP.add_region(esp_alloc::HeapRegion::new(
        core::ptr::addr_of_mut!(HEAP) as *mut u8,
        72 * 1024,
        esp_alloc::MemoryCapability::Internal.into(),
    ));
}

#[path = "../menu.rs"]
mod menu;
#[path = "../provision.rs"]
mod provision;
#[path = "../qrcode.rs"]
mod qrcode;
#[path = "../ui.rs"]
mod ui;
#[path = "../viewmodel.rs"]
mod viewmodel;
#[path = "../wifi.rs"]
mod wifi;

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
        MenuItem::Label { label: ("Powered by Deepseek V4 Flash") },
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

#[main]
fn main() -> ! {
    let peripherals = esp_hal::init(Config::default().with_cpu_clock(CpuClock::max()));
    unsafe { init_heap(); }
    println!("[u8gg] Boot OK");

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let sw_int = SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_int.software_interrupt0);
    println!("[u8gg] RTOS started");

    let (mut wifi_ctrl, _interfaces) =
        esp_radio::wifi::new(peripherals.WIFI, Default::default()).unwrap();
    println!("[u8gg] WiFi initialized");
    let mut wifi_retry = 0u8;

    let k1 = Input::new(peripherals.GPIO1, InputConfig::default().with_pull(Pull::Up));
    let k2 = Input::new(peripherals.GPIO2, InputConfig::default().with_pull(Pull::Up));
    let k3 = Input::new(peripherals.GPIO3, InputConfig::default().with_pull(Pull::Up));
    let k4 = Input::new(peripherals.GPIO4, InputConfig::default().with_pull(Pull::Up));

    let mut i2c = I2c::new(
        peripherals.I2C0,
        I2cConfig::default().with_frequency(Rate::from_khz(400)),
    )
    .unwrap()
    .with_sda(peripherals.GPIO8)
    .with_scl(peripherals.GPIO9);
    let addr_ok = i2c.write(0x3D, &[0x00]).is_ok();
    println!("[u8gg] I2C probe 0x3D: {}", if addr_ok { "OK" } else { "FAIL" });
    if !addr_ok {
        // 尝试标准地址 0x3C
        let addr2_ok = i2c.write(0x3C, &[0x00]).is_ok();
        println!("[u8gg] I2C probe 0x3C: {}", if addr2_ok { "OK" } else { "FAIL" });
    }
    let iface = if addr_ok {
        I2CDisplayInterface::new_alternate_address(i2c)
    } else {
        I2CDisplayInterface::new(i2c)
    };
    let mut display = Ssd1306::new(iface, DisplaySize128x64, DisplayRotation::Rotate0)
        .into_buffered_graphics_mode();
    let mut oled_ok = false;
    for i in 0..10 {
        spin_delay(100);
        if display.init().is_ok() {
            println!("[u8gg] OLED init OK (attempt {})", i + 1);
            oled_ok = true;
            break;
        }
    }
    if !oled_ok {
        println!("[u8gg] OLED init FAILED — will skip display");
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
        spin_delay(5);
        tick_ms = tick_ms.wrapping_add(5);
        fps_counter += 1;

        let c = [k1.is_low(), k2.is_low(), k3.is_low(), k4.is_low()];
        let edge = [c[0] && !pk[0], c[1] && !pk[1], c[2] && !pk[2], c[3] && !pk[3]];
        let mut long_edge = [false; 4];
        let mut repeat_up = false;
        let mut repeat_down = false;

        for i in 0..4 {
            if c[i] { hold[i] = hold[i].saturating_add(1); } else { hold[i] = 0; }
            let h = hold[i];
            if i < 2 {
                if h >= 66 {
                    let interval = if h < 90 { 4 } else if h < 120 { 3 } else { 2 };
                    if (h - 66) % interval == 0 {
                        if i == 0 { repeat_up = true; } else { repeat_down = true; }
                    }
                }
            } else if h == 66 {
                long_edge[i] = true;
            }
        }
        pk = c;

        let any_key = edge[0] || edge[1] || edge[2] || edge[3]
            || long_edge[2] || long_edge[3] || repeat_up || repeat_down;

        vm.handle_key(
            edge[0] || repeat_up, edge[1] || repeat_down,
            edge[3], edge[2], long_edge[2], long_edge[3],
        );

        let wifi_status = wifi::tick(&mut wifi_ctrl, &mut wifi_retry);
        if vm.consume_wifi_connect() {
            let ssid = vm.input_ssid();
            let pass = vm.input_pass();
            if !ssid.is_empty() {
                println!("[u8gg] WiFi connect to \"{}\"", ssid);
                wifi::connect(&mut wifi_ctrl, ssid, pass);
                wifi_retry = 0;
            }
        }
        vm.set_wifi_status(wifi_status);
        vm.set_wifi_connected(wifi_ctrl.is_connected());

        if oled_ok {
            canvas.clear();
            vm.render(&mut canvas);
            ViewModel::blit(&canvas, &mut display);
            let _ = display.flush();
        }

        if tick_ms - last_fps_print >= 5000 {
            println!("[u8gg] FPS: {}", fps_counter * 1000 / (tick_ms - last_fps_print).max(1));
            fps_counter = 0;
            last_fps_print = tick_ms;
        }
    }
}

fn spin_delay(ms: u32) {
    for _ in 0..ms * 16000 { core::hint::spin_loop(); }
}
