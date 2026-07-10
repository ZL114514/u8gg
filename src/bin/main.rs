#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::String;
use embedded_qr::{QrBuilder, Version10};
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

#[path = "../menu.rs"]
mod menu;
#[path = "../provision.rs"]
mod provision;
#[path = "../ui.rs"]
mod ui;
#[path = "../viewmodel.rs"]
mod viewmodel;

#[path = "../wifi.rs"]
mod wifi;
use menu::{BufCanvas, MenuEngine, MenuItem, MenuPage};
use viewmodel::{AppScreen, ViewModel};

// ===== 应用描述符 =====
esp_bootloader_esp_idf::esp_app_desc!();

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
define_binds! { FPS, SLIDER, TOGGLE, SHOW_QR, WIFI_MANUAL, AP_SELECTED, APPROV, NETINFO }

const WIFI_SSID: &str = "MikuStation";
const WIFI_PASS: &str = "HARDCORE";

// ===== 静态菜单页 =====
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
        MenuItem::Label { label: "no_std + esp-radio" },
    ],
};
/// WiFi 扫描入口页 (扫描前的占位)
static PAGE_WIFI_SCAN: MenuPage = MenuPage {
    title: "WiFi扫描",
    items: &[MenuItem::Label { label: "正在扫描..." }],
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
        MenuItem::Submenu { label: "WiFi扫描", page: &PAGE_WIFI_SCAN },
        MenuItem::Toggle { label: "AP配网", bind: APPROV },
        MenuItem::Submenu { label: "网络信息", page: &NETINFO_PAGE },
        MenuItem::Submenu { label: "关于", page: &PAGE_ABOUT },
    ],
};

// ===== 动态网络信息页 =====
const MAX_NETINFO_ITEMS: usize = 6;
static mut NETINFO_ITEMS: core::mem::MaybeUninit<[MenuItem; MAX_NETINFO_ITEMS]> =
    core::mem::MaybeUninit::uninit();
static mut NETINFO_PAGE: MenuPage = MenuPage { title: "网络信息", items: &[] };
static mut NETINFO_BUFS: [[u8; 48]; MAX_NETINFO_ITEMS] = [[0; 48]; MAX_NETINFO_ITEMS];

/// 动态构建网络信息页
fn build_netinfo_page(wifi_connected: bool, ssid: &str, mac: &str) -> &'static MenuPage {
    unsafe {
        let items_ptr = NETINFO_ITEMS.as_mut_ptr() as *mut MenuItem;
        let bufs = &mut NETINFO_BUFS;

        let lines = [
            ("状态", if wifi_connected { "已连接" } else { "未连接" }),
            ("SSID", ssid),
            ("MAC", mac),
            ("TCP端口", "4545"),
        ];
        let mut n = 0usize;
        for &(label, value) in &lines {
            let buf = &mut bufs[n];
            let mut pos = 0;
            for &b in label.as_bytes() {
                if pos >= 46 { break; }
                buf[pos] = b;
                pos += 1;
            }
            buf[pos] = b':'; pos += 1;
            buf[pos] = b' '; pos += 1;
            for &b in value.as_bytes() {
                if pos >= 47 { break; }
                buf[pos] = b;
                pos += 1;
            }
            buf[pos] = 0;
            let s: &str = core::str::from_utf8_unchecked(&buf[..pos]);
            let s_static: &'static str = &*(s as *const str);
            core::ptr::write(items_ptr.add(n), MenuItem::Label { label: s_static });
            n += 1;
        }
        NETINFO_PAGE.items = core::slice::from_raw_parts(items_ptr, n);
        &*(&raw const NETINFO_PAGE)
    }
}
const MAX_SCAN_AP: usize = 8;

static mut SCAN_ITEMS: core::mem::MaybeUninit<[MenuItem; MAX_SCAN_AP]> = core::mem::MaybeUninit::uninit();
static mut SCAN_PAGE: MenuPage = MenuPage { title: "WiFi网络", items: &[] };
static mut SCAN_SSID_BUFS: [[u8; 40]; MAX_SCAN_AP] = [[0; 40]; MAX_SCAN_AP];
static mut SCAN_AP_COUNT: usize = 0;

/// AP 选择动作函数 (每个索引一个, 通过函数指针表)
fn ap_sel_0(e: &mut MenuEngine) { e.bind_bools[AP_SELECTED] = true; e.bind_u8s[0] = 0; }
fn ap_sel_1(e: &mut MenuEngine) { e.bind_bools[AP_SELECTED] = true; e.bind_u8s[0] = 1; }
fn ap_sel_2(e: &mut MenuEngine) { e.bind_bools[AP_SELECTED] = true; e.bind_u8s[0] = 2; }
fn ap_sel_3(e: &mut MenuEngine) { e.bind_bools[AP_SELECTED] = true; e.bind_u8s[0] = 3; }
fn ap_sel_4(e: &mut MenuEngine) { e.bind_bools[AP_SELECTED] = true; e.bind_u8s[0] = 4; }
fn ap_sel_5(e: &mut MenuEngine) { e.bind_bools[AP_SELECTED] = true; e.bind_u8s[0] = 5; }
fn ap_sel_6(e: &mut MenuEngine) { e.bind_bools[AP_SELECTED] = true; e.bind_u8s[0] = 6; }
fn ap_sel_7(e: &mut MenuEngine) { e.bind_bools[AP_SELECTED] = true; e.bind_u8s[0] = 7; }

const AP_SEL_FNS: [fn(&mut MenuEngine); MAX_SCAN_AP] = [
    ap_sel_0, ap_sel_1, ap_sel_2, ap_sel_3,
    ap_sel_4, ap_sel_5, ap_sel_6, ap_sel_7,
];

/// 用扫描结果构建动态菜单页, 返回 &'static MenuPage
fn build_scan_page(scan_results: &[wifi::ScanResult]) -> &'static MenuPage {
    let n = scan_results.len().min(MAX_SCAN_AP);
    unsafe { SCAN_AP_COUNT = n; }

    let items_ptr = core::ptr::addr_of_mut!(SCAN_ITEMS) as *mut MenuItem;
    for i in 0..n {
        let ssid = &scan_results[i].ssid;
        unsafe {
            let buf = &mut SCAN_SSID_BUFS[i];
            let mut pos = 0;
            for &b in ssid.as_bytes() {
                if pos >= 31 { break; }
                buf[pos] = b;
                pos += 1;
            }
            buf[pos] = 0;
            let s: &str = core::str::from_utf8_unchecked(&buf[..pos]);
            let s_static: &'static str = &*(s as *const str);
            core::ptr::write(items_ptr.add(i), MenuItem::Button {
                label: s_static,
                action: AP_SEL_FNS[i],
            });
        }
    }
    // 剩余位置写入空 Label (保证安全)
    for i in n..MAX_SCAN_AP {
        unsafe {
            core::ptr::write(items_ptr.add(i), MenuItem::Label { label: "" });
        }
    }
    unsafe {
        let items_slice = core::slice::from_raw_parts(items_ptr, n);
        SCAN_PAGE.items = items_slice;
        &*(&raw const SCAN_PAGE)
    }
}

// ===== 自旋延时 =====
fn spin_delay(ms: u32) {
    for _ in 0..ms * 16000 { core::hint::spin_loop(); }
}

#[main]
fn main() -> ! {
    let peripherals = esp_hal::init(Config::default().with_cpu_clock(CpuClock::max()));
    unsafe { init_heap(); }
    println!("[u8gg] Boot OK (no_std + esp-radio)");

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let sw_int = SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_int.software_interrupt0);
    println!("[u8gg] RTOS started");

    let (mut wifi_ctrl, mut interfaces) =
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

    let mut scan_started = false; // 防止重复触发扫描
    let mut provision = provision::Provisioning::new();

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

        vm.btn = edge;
        vm.handle_key(
            edge[0] || repeat_up, edge[1] || repeat_down,
            edge[3], edge[2], long_edge[2], long_edge[3],
        );

        // ---- AP 配网切换 ----
        if vm.menu.bind_bools[APPROV] {
        let page_title = if vm.screen == AppScreen::Menu { vm.menu.page().title } else { "" };
            vm.menu.bind_bools[APPROV] = false;
            let qr_ssid = "u8gg-Config";
            let qr_pass = "12345678";
            let wifi_str = alloc::format!("WIFI:T:WPA;S:{};P:{};;", qr_ssid, qr_pass);
            vm.cached_qr = QrBuilder::<Version10>::new().build(wifi_str.as_bytes()).ok();
            provision.start_ap(&mut wifi_ctrl);
            vm.screen = viewmodel::AppScreen::ApProvision;
            vm.menu.dirty = true;
            println!("[u8gg] AP provision mode");
        }
        // ---- 网络信息页重建 ----
        if page_title == "网络信息" {
            unsafe {
                let mac = esp_hal::efuse::interface_mac_address(esp_hal::efuse::InterfaceMacAddress::Station);
                let mac_bytes = mac.as_bytes();
                const HEX: &[u8; 16] = b"0123456789ABCDEF";
                let mut mac_str = [0u8; 18];
                for i in 0..6 {
                    mac_str[i*3] = HEX[(mac_bytes[i] >> 4) as usize];
                    mac_str[i*3+1] = HEX[(mac_bytes[i] & 0x0F) as usize];
                    if i < 5 { mac_str[i*3+2] = b':'; }
                }
                mac_str[17] = 0;
                let mac_s = core::str::from_utf8_unchecked(&mac_str[..17]);
                let page = build_netinfo_page(vm.wifi_connected, vm.wifi_ssid, mac_s);
                vm.menu.set_page(page);
                vm.menu.dirty = true;
            }
        }

        // ---- WiFi 扫描 ----

        if page_title == "WiFi扫描" && !scan_started {
            scan_started = true;
            println!("[u8gg] Starting WiFi scan...");
            let results = wifi::scan(&mut wifi_ctrl);
            if results.is_empty() {
                println!("[u8gg] Scan empty");
                vm.menu.set_page(&PAGE_WIFI_SCAN);
                vm.menu.dirty = true;
            } else {
                println!("[u8gg] Scan found {} APs", results.len());
                let page = build_scan_page(&results);
                vm.menu.set_page(page);
                vm.menu.dirty = true;
            }
        }
        if page_title != "WiFi扫描" && page_title != "WiFi网络" {
            scan_started = false;
        }

        // ---- AP 选择 ----
        if vm.menu.bind_bools[AP_SELECTED] {
            vm.menu.bind_bools[AP_SELECTED] = false;
            let idx = vm.menu.bind_u8s[0] as usize;
            unsafe {
                if idx < SCAN_AP_COUNT {
                    let len = SCAN_SSID_BUFS[idx].iter().position(|&b| b == 0).unwrap_or(32);
                    let ssid = core::str::from_utf8(&SCAN_SSID_BUFS[idx][..len]).unwrap_or("");
                    if !ssid.is_empty() {
                        vm.ssid_buf_fill(ssid);
                        vm.start_password_input();
                        println!("[u8gg] Selected AP: \"{}\"", ssid);
                    }
                }
            }
        }

        // ---- WiFi 连接 ----
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

        // ---- OLED 渲染 ----
        if oled_ok {
            canvas.clear();
            vm.render(&mut canvas);
            ViewModel::blit(&canvas, &mut display);
            let _ = display.flush();
        }

        // ---- FPS 打印 ----
        if tick_ms - last_fps_print >= 5000 {
            println!("[u8gg] FPS: {}", fps_counter * 1000 / (tick_ms - last_fps_print).max(1));
            fps_counter = 0;
            last_fps_print = tick_ms;
        }

        // ---- 驱动 AP 网络栈 ----
        provision.tick(5);
    }
}

// ===== 堆分配器 =====
#[unsafe(no_mangle)]
static mut HEAP: core::mem::MaybeUninit<[u8; 72 * 1024]> = core::mem::MaybeUninit::uninit();
unsafe fn init_heap() {
    esp_alloc::HEAP.add_region(esp_alloc::HeapRegion::new(
        core::ptr::addr_of_mut!(HEAP) as *mut u8,
        72 * 1024,
        esp_alloc::MemoryCapability::Internal.into(),
    ));
}
