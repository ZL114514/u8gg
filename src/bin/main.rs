#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::String;
use embedded_qr::{QrBuilder, QrMatrix, Version10};
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
#[path = "../ui/mod.rs"]
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

const WIFI_SSID: &str = "A320_2.4G";
const WIFI_PASS: &str = "320320320";

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
        MenuItem::Button { label: "网络信息", action: goto_netinfo_action },
        MenuItem::Submenu { label: "关于", page: &PAGE_ABOUT },
    ],
};

fn goto_netinfo_action(e: &mut MenuEngine) {
    e.bind_bools[NETINFO] = true;
}

// ===== QR 缓存 (用于菜单页) =====
static mut QR_CACHE: Option<QrMatrix<Version10>> = None;

/// Custom render: 在菜单上绘制 QR 码 overlay
fn qr_render_custom(buf: &mut BufCanvas, _e: &MenuEngine) {
    unsafe {
        if let Some(ref qr) = QR_CACHE {
            let ms = 2u32;
            let quiet = 3u32;
            let total = qr.width() as u32 * ms + quiet * 2;
            let ox = (128i32 - total as i32) / 2;
            let oy = (64i32 - total as i32) / 2;
            let cy = oy + (total / 2) as i32;
            // 清除中间区域
            let clear_y = (oy).max(16);
            let clear_h = (oy + total as i32 - clear_y).min(64 - clear_y) as u32;
            let clear_w = (128.min(ox + total as i32 + quiet as i32 * 2) - 0.max(ox - quiet as i32 * 2)) as u32;
            if clear_w > 0 && clear_h > 0 {
                let x0 = 0.max(ox - quiet as i32 * 2);
                for py in clear_y..clear_y + clear_h as i32 {
                    for px in x0..x0 + clear_w as i32 {
                        let idx = (py as usize / 8) * 128 + px as usize;
                        if idx < 1024 {
                            buf.0[idx] = 0;
                        }
                    }
                }
            }
            // 绘制 QR 模块
            for y in 0..qr.width() as u32 {
                for x in 0..qr.width() as u32 {
                    if qr.get(x as usize, y as usize) {
                        buf.invert_rect(
                            ox + quiet as i32 + (x * ms) as i32,
                            oy + quiet as i32 + (y * ms) as i32,
                            ms,
                            ms,
                            0,
                        );
                    }
                }
            }
        }
    }
}

/// WiFi QR 显示页 (Custom render)
static PAGE_WIFI_QR: MenuPage = MenuPage {
    title: "WiFi二维码",
    items: &[
        MenuItem::Custom { render: qr_render_custom },
        MenuItem::Label { label: "返回: K3 / K4" },
    ],
};

/// AP 配网入口页
static PAGE_AP_PROV: MenuPage = MenuPage {
    title: "AP配网",
    items: &[
        MenuItem::Label { label: "热点: u8gg-Config" },
        MenuItem::Button { label: "显示QR码", action: ap_show_qr },
        MenuItem::Button { label: "输入目标WiFi", action: ap_wifi_input },
        MenuItem::Submenu { label: "返回主菜单", page: &PAGE_MAIN },
    ],
};

fn ap_show_qr(e: &mut MenuEngine) {
    let wifi_str = alloc::format!("WIFI:T:WPA;S:{};P:{};;", "u8gg-Config", "12345678");
    unsafe {
        QR_CACHE = QrBuilder::<Version10>::new().build(wifi_str.as_bytes()).ok();
    }
    // Navigate to QR display page (duplicate PAGE_WIFI_QR)
    // We re-use the same QR cache + render, so just set_page
    // Actually we need to trigger navigation. Use submenu approach.
    // For now, just set the QR cache; main loop navigates.
    // We'll use a bind flag
    e.bind_bools[SHOW_QR] = true;
}

fn ap_wifi_input(e: &mut MenuEngine) {
    e.bind_bools[WIFI_MANUAL] = true;
}

// ===== 按键测试动态页 =====
static mut BTN_TEST_PAGE: MenuPage = MenuPage { title: "按键测试", items: &[] };
static mut BTN_TEST_ITEMS: core::mem::MaybeUninit<[MenuItem; 4]> = core::mem::MaybeUninit::uninit();
static mut BTN_TEST_LABELS: [[u8; 16]; 4] = [[0; 16]; 4];

fn build_btn_test_page(pressed: &[bool; 4]) -> &'static MenuPage {
    unsafe {
        let items_ptr = core::ptr::addr_of_mut!(BTN_TEST_ITEMS) as *mut MenuItem;
        let bufs_ptr = core::ptr::addr_of_mut!(BTN_TEST_LABELS) as *mut [u8; 16];
        for i in 0..4 {
            let buf = &mut *bufs_ptr.add(i);
            buf[0] = match i { 0 => b'K', 1 => b'K', 2 => b'K', _ => b'K' };
            buf[1] = match i { 0 => b'1', 1 => b'2', 2 => b'3', _ => b'4' };
            buf[2] = b' ';
            let labels = [b"UP  ", b"DOWN", b"BACK", b"OK  "];
            buf[3..7].copy_from_slice(labels[i]);
            buf[7] = b' ';
            buf[8] = b'[';
            buf[9] = if pressed[i] { b'*' } else { b' ' };
            buf[10] = b']';
            buf[11] = 0;
            let s: &str = core::str::from_utf8_unchecked(&buf[..11]);
            let s_static: &'static str = &*(s as *const str);
            core::ptr::write(items_ptr.add(i), MenuItem::Label { label: s_static });
        }
        let items_slice_ptr = core::ptr::addr_of!(BTN_TEST_ITEMS) as *const MenuItem;
        BTN_TEST_PAGE.items = core::slice::from_raw_parts(items_slice_ptr, 4);
        &*(&raw const BTN_TEST_PAGE)
    }
}

// ===== 动态网络信息页 =====
const MAX_NETINFO_ITEMS: usize = 6;
static mut NETINFO_ITEMS: core::mem::MaybeUninit<[MenuItem; MAX_NETINFO_ITEMS]> =
    core::mem::MaybeUninit::uninit();
static mut NETINFO_PAGE: MenuPage = MenuPage { title: "网络信息", items: &[] };
static mut NETINFO_BUFS: [[u8; 48]; MAX_NETINFO_ITEMS] = [[0; 48]; MAX_NETINFO_ITEMS];

/// 动态构建网络信息页
fn build_netinfo_page(wifi_connected: bool, ssid: &str, mac: &str) -> &'static MenuPage {
    unsafe {
        let items_ptr = core::ptr::addr_of_mut!(NETINFO_ITEMS) as *mut MenuItem;
        let bufs_ptr = core::ptr::addr_of_mut!(NETINFO_BUFS) as *mut [u8; 48];

        let lines = [
            ("状态", if wifi_connected { "已连接" } else { "未连接" }),
            ("SSID", ssid),
            ("MAC", mac),
            ("TCP端口", "4545"),
        ];
        let mut n = 0usize;
        for &(label, value) in &lines {
            let buf = &mut *bufs_ptr.add(n);
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

// ===== WiFi 扫描动态页 =====
const MAX_SCAN_AP: usize = 8;

static mut SCAN_ITEMS: core::mem::MaybeUninit<[MenuItem; MAX_SCAN_AP]> =
    core::mem::MaybeUninit::uninit();
static mut SCAN_PAGE: MenuPage = MenuPage { title: "WiFi网络", items: &[] };
static mut SCAN_SSID_BUFS: [[u8; 40]; MAX_SCAN_AP] = [[0; 40]; MAX_SCAN_AP];
static mut SCAN_AP_COUNT: usize = 0;

/// 单一 AP 选择动作: 使用光标位置作为索引
fn ap_selected_action(e: &mut MenuEngine) {
    e.bind_bools[AP_SELECTED] = true;
    e.bind_u8s[0] = e.selection() as u8;
}

/// 用扫描结果构建动态菜单页, 返回 &'static MenuPage
fn build_scan_page(scan_results: &[wifi::ScanResult]) -> &'static MenuPage {
    let n = scan_results.len().min(MAX_SCAN_AP);
    unsafe { SCAN_AP_COUNT = n; }

    let items_ptr = core::ptr::addr_of_mut!(SCAN_ITEMS) as *mut MenuItem;
    let bufs_ptr = core::ptr::addr_of_mut!(SCAN_SSID_BUFS) as *mut [u8; 40];
    for i in 0..n {
        let ssid = &scan_results[i].ssid;
        unsafe {
            let buf = &mut *bufs_ptr.add(i);
            let mut pos = 0;
            for &b in ssid.as_bytes() {
                if pos >= 31 { break; }
                buf[pos] = b;
                pos += 1;
            }
            buf[pos] = 0;
            let s: &str = core::str::from_utf8_unchecked(&buf[..pos]);
            let s_static: &'static str = &*(s as *const str);
            core::ptr::write(
                items_ptr.add(i),
                MenuItem::Button {
                    label: s_static,
                    action: ap_selected_action,
                },
            );
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

    let mut canvas = menu::BufCanvas([0u8; 1024]);

    let mut tick_ms = 0u32;
    let mut fps_counter = 0u32;
    let mut last_fps_print = 0u32;
    let mut pk = [true; 4];
    let mut hold = [0u16; 4];

    let mut scan_started = false;
    let mut provision = provision::Provisioning::new();

    loop {
        spin_delay(5);
        tick_ms = tick_ms.wrapping_add(5);
        fps_counter += 1;

        let c = [k1.is_low(), k2.is_low(), k3.is_low(), k4.is_low()];
        let edge = [
            c[0] && !pk[0],
            c[1] && !pk[1],
            c[2] && !pk[2],
            c[3] && !pk[3],
        ];
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

        vm.handle_key(
            edge[0] || repeat_up,
            edge[1] || repeat_down,
            edge[3],
            edge[2],
            long_edge[2],
            long_edge[3],
        );

        // ---- Menu-driven actions (bind flags) ----
        if vm.menu.bind_bools[SHOW_QR] {
            vm.menu.bind_bools[SHOW_QR] = false;
            // Build WiFi QR code and navigate to QR page
            if vm.screen == AppScreen::Menu && vm.menu.page().title == "AP配网" {
                // AP provision requested QR
                // QR already set by ap_show_qr
            } else {
                // Main menu SHOW_QR toggle
                let wifi_str = alloc::format!("WIFI:T:WPA;S:{};P:{};;", WIFI_SSID, WIFI_PASS);
                unsafe {
                    QR_CACHE = QrBuilder::<Version10>::new().build(wifi_str.as_bytes()).ok();
                }
            }
            vm.menu.set_page(&PAGE_WIFI_QR);
            vm.menu.dirty = true;
        }

        if vm.menu.bind_bools[WIFI_MANUAL] {
            vm.menu.bind_bools[WIFI_MANUAL] = false;
            vm.reset_input();
            vm.screen = AppScreen::WifiInput;
        }

        if vm.menu.bind_bools[APPROV] {
            vm.menu.bind_bools[APPROV] = false;
            let wifi_str = alloc::format!("WIFI:T:WPA;S:{};P:{};;", "u8gg-Config", "12345678");
            unsafe {
                QR_CACHE = QrBuilder::<Version10>::new().build(wifi_str.as_bytes()).ok();
            }
            provision.start_ap(&mut wifi_ctrl);
            vm.menu.set_page(&PAGE_AP_PROV);
            vm.menu.dirty = true;
            println!("[u8gg] AP provision mode");
        }

        // ---- 网络信息按钮导航 ----
        if vm.menu.bind_bools[NETINFO] {
            vm.menu.bind_bools[NETINFO] = false;
            unsafe {
                let page = build_netinfo_page(false, "??", "??:??:??:??:??:??");
                vm.menu.set_page(page);
                vm.menu.dirty = true;
            }
        }

        // ---- 按键测试页重建 ----
        let page_title = if vm.screen == AppScreen::Menu {
            vm.menu.page().title
        } else {
            ""
        };

        if page_title == "按键测试" {
            let page = build_btn_test_page(&[c[0], c[1], c[2], c[3]]);
            vm.menu.set_page(page);
            vm.menu.dirty = true;
        }

        // ---- 网络信息页重建 ----
        if page_title == "网络信息" {
            unsafe {
                let mac = esp_hal::efuse::interface_mac_address(
                    esp_hal::efuse::InterfaceMacAddress::Station,
                );
                let mac_bytes = mac.as_bytes();
                const HEX: &[u8; 16] = b"0123456789ABCDEF";
                let mut mac_str = [0u8; 18];
                for i in 0..6 {
                    mac_str[i * 3] = HEX[(mac_bytes[i] >> 4) as usize];
                    mac_str[i * 3 + 1] = HEX[(mac_bytes[i] & 0x0F) as usize];
                    if i < 5 { mac_str[i * 3 + 2] = b':'; }
                }
                mac_str[17] = 0;
                let mac_s = core::str::from_utf8_unchecked(&mac_str[..17]);
                let page = build_netinfo_page(vm.is_wifi_connected(), WIFI_SSID, mac_s);
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
                    let len = SCAN_SSID_BUFS[idx]
                        .iter()
                        .position(|&b| b == 0)
                        .unwrap_or(32);
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
            println!(
                "[u8gg] FPS: {}",
                fps_counter * 1000 / (tick_ms - last_fps_print).max(1)
            );
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
