#![no_std]
#![no_main]

use core::hint::spin_loop;
use esp_backtrace as _;
use esp_bootloader_esp_idf::esp_app_desc;
use esp_hal::analog::adc::{Adc, AdcConfig, Attenuation};
use esp_hal::{
    clock::CpuClock,
    delay::Delay,
    gpio::{Input, InputConfig, Level, Output, OutputConfig, Pull},
    i2c::master::{Config as I2cConfig, I2c},
    interrupt::software::SoftwareInterruptControl,
    main,
    time::Rate,
    timer::timg::TimerGroup,
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
#[path = "../ui.rs"]
mod ui;
#[path = "../viewmodel.rs"]
mod viewmodel;

use menu::{BufCanvas, MenuEngine, MenuItem, MenuPage};
use viewmodel::{AppScreen, ViewModel};

// ===== 应用描述符 =====
esp_bootloader_esp_idf::esp_app_desc!();

esp_app_desc!();

// ===== bind 变量索引（自动计数） =====
macro_rules! define_binds {
    ($($name:ident),+ $(,)?) => { define_binds!(@step 0usize, $($name),+); };
    (@step $n:expr, $last:ident $(,)?) => { pub const $last: usize = $n; };
    (@step $n:expr, $first:ident, $($rest:ident),+ $(,)?) => {
        pub const $first: usize = $n;
        define_binds!(@step ($n) + 1, $($rest),+);
    };
}
define_binds! { FPS, SLIDER, TOGGLE, SHOW_QR, WIFI_MANUAL, AP_SELECTED, APPROV, NETINFO }

const WIFI_SSID: &str = "MikuStation";
const WIFI_PASS: &str = "HARDCORE";

// ===== 静态菜单页 =====
static PAGE_SUB: MenuPage = MenuPage {
    title: "子菜单",
    items: &[
        MenuItem::Submenu {
            label: "选项 A",
            page: &PAGE_SUB,
        },
        MenuItem::Label { label: "选项 B" },
        MenuItem::Label { label: "选项 C" },
    ],
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
        MenuItem::Toggle { label: "网络信息", bind: NETINFO },
        MenuItem::Submenu { label: "关于", page: &PAGE_ABOUT },
    ],
};

// ===== 动态扫描结果页 (最多 8 个 AP) =====
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
    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    // 初始化堆分配器 (必须在任何堆分配调用之前)
    unsafe { init_heap(); }
    println!("[u8gg] Boot OK");

    // ===== 启动 RTOS 调度器 (esp-radio 要求，必须在任何 WiFi 调用之前) =====
    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let sw_int = SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_int.software_interrupt0);
    println!("[u8gg] RTOS started");

    // ===== WiFi 连接 (参考 ESP-IDF DPP Enrollee 例子的事件驱动模式) =====
    // 取消注释并填入你的 SSID/密码即可使用:
    // let (mut wifi_ctrl, mut interfaces) = esp_radio::wifi::new(
    //     peripherals.WIFI,
    //     Default::default(),
    // ).unwrap();
    // wifi::connect_blocking(&mut wifi_ctrl, "YourSSID", "YourPassword", &delay);
    // let mut wifi_retry = 0u8;
    // 然后在主循环中加入: wifi::tick(&mut wifi_ctrl, &mut wifi_retry);

    let mut led = Output::new(peripherals.GPIO2, Level::Low, OutputConfig::default());
    led.set_high();
    spin_delay(200);

    // ===== HW-504 摇杆 (ADC 模拟输入) =====
    let mut adc_cfg = AdcConfig::default();
    let mut adc_vry = adc_cfg.enable_pin(peripherals.GPIO34, Attenuation::_11dB);
    let mut adc_vrx = adc_cfg.enable_pin(peripherals.GPIO35, Attenuation::_11dB);
    let mut adc = Adc::new(peripherals.ADC1, adc_cfg);
    let joy_sw = Input::new(
        peripherals.GPIO32,
        InputConfig::default().with_pull(Pull::Up),
    );
    let mut vry_up_prev = false;
    let mut vry_dn_prev = false;
    let mut vrx_lf_prev = false;
    let mut vrx_rt_prev = false;
    let mut joy_sw_prev = false;
    let mut joy_hold_up = 0u16;
    let mut joy_hold_dn = 0u16;

    let k1 = Input::new(
        peripherals.GPIO23,
        InputConfig::default().with_pull(Pull::Up),
    );
    let k2 = Input::new(
        peripherals.GPIO25,
        InputConfig::default().with_pull(Pull::Up),
    );
    let k3 = Input::new(
        peripherals.GPIO26,
        InputConfig::default().with_pull(Pull::Up),
    );
    let k4 = Input::new(
        peripherals.GPIO27,
        InputConfig::default().with_pull(Pull::Up),
    );

    let mut i2c = I2c::new(
        peripherals.I2C0,
        I2cConfig::default().with_frequency(Rate::from_khz(400)),
    )
    .unwrap()
    .with_sda(peripherals.GPIO21)
    .with_scl(peripherals.GPIO22);
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
    let mut ok = false;
    for i in 0..10 {
        spin_delay(100);
        if display.init().is_ok() {
            println!("[u8gg] OLED init OK (attempt {})", i + 1);
            ok = true;
            break;
        }
        println!("[u8gg] OLED init FAIL (attempt {})", i + 1);
    }
    if ok {
        display.set_brightness(Brightness::BRIGHTEST).unwrap();
    }

    use embedded_graphics::pixelcolor::BinaryColor;

    let mut vm = ViewModel::new(&PAGE_MAIN);
    let mut buf = BufCanvas::new();

    let mut pk = [false; 4];
    let mut hold = [0u16; 4];
    let mut repeat_up = false;
    let mut repeat_down = false;
    let mut joy_up = false;
    let mut joy_down = false;
    let mut joy_btn = false;
    let mut joy_left = false;
    let mut loop_tick = 0u16;
    let mut render_tick = 0u16;

    let mut scan_started = false; // 防止重复触发扫描
    let mut provision = provision::Provisioning::new();

    loop {
        loop_tick += 1;
        if loop_tick >= 50 {
            vm.menu.fps = render_tick.min(99);
            render_tick = 0;
            loop_tick = 0;
        }

        let c = [k1.is_low(), k2.is_low(), k3.is_low(), k4.is_low()];
        
        // ---- 读摇杆 ----
        let vry = nb::block!(adc.read_oneshot(&mut adc_vry)).unwrap_or(2048);
        let vrx = nb::block!(adc.read_oneshot(&mut adc_vrx)).unwrap_or(2048);
        let joy_sw_now = joy_sw.is_low();
        let now_up = vry < 800;
        let now_dn = vry > 3200;
        let now_lf = vrx < 800;
        let now_rt = vrx > 3200;

        // 只有当摇杆确实偏离中心时才触发
        // 连发计数器
        if now_up { joy_hold_up = joy_hold_up.saturating_add(1); } else { joy_hold_up = 0; }
        if now_dn { joy_hold_dn = joy_hold_dn.saturating_add(1); } else { joy_hold_dn = 0; }
        // 边沿触发 + 连发
        joy_up = now_up && !vry_up_prev;
        joy_down = now_dn && !vry_dn_prev;
        if joy_hold_up >= 15 && (joy_hold_up - 15) % 3 == 0 { joy_up = true; }
        if joy_hold_dn >= 15 && (joy_hold_dn - 15) % 3 == 0 { joy_down = true; }
        vry_up_prev = now_up;
        vry_dn_prev = now_dn;
        joy_left = now_lf && !vrx_lf_prev;
        joy_btn = joy_sw_now && !joy_sw_prev;
        vrx_lf_prev = now_lf;
        vrx_rt_prev = now_rt;
        joy_sw_prev = joy_sw_now;

        let edge = [
            c[0] && !pk[0],
            c[1] && !pk[1],
            c[2] && !pk[2],
            c[3] && !pk[3],
        ];
        let release = [
            !c[0] && pk[0],
            !c[1] && pk[1],
            !c[2] && pk[2],
            !c[3] && pk[3],
        ];
        let long_edge = [false; 4];
        let mut ok = edge[0]
            || edge[1]
            || edge[2]
            || edge[3]
            || release[0]
            || release[1]
            || release[2]
            || release[3];

        // 连发加速
        repeat_up = false;
        repeat_down = false;
        for i in 0..2 {
            if c[i] {
                hold[i] = hold[i].saturating_add(1);
            } else {
                hold[i] = 0;
            }
            let h = hold[i];
            if h >= 66 {
                // 66*20ms=1.3s 启动连发
                let interval = if h < 90 {
                    4
                } else if h < 120 {
                    3
                } else {
                    2
                };
                if (h - 66) % interval == 0 {
                    if i == 0 {
                        repeat_up = true;
                    } else {
                        repeat_down = true;
                    }
                    ok = true;
                }
            }
        }
        pk = c;

        if edge[0]
            || edge[1]
            || edge[2]
            || edge[3]
            || long_edge[0]
            || long_edge[1]
            || long_edge[2]
            || long_edge[3]
        {
            println!(
                "[u8gg] key: U={} D={} B={} E={} long:B={} E={}",
                edge[0] as u8,
                edge[1] as u8,
                edge[2] as u8,
                edge[3] as u8,
                long_edge[2] as u8,
                long_edge[3] as u8
            );
        }

        vm.handle_key(
            edge[0] || repeat_up || joy_up,
            edge[1] || repeat_down || joy_down,
            edge[3] || joy_btn,
            edge[2] || joy_left,
            long_edge[2],
            long_edge[3],
        );

        render_tick += 1;
        buf.clear();
        vm.render(&mut buf);
        ViewModel::blit(&buf, &mut display);
        let _ = display.flush();
        led.toggle();
        //delay.delay_millis(20);
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
fn spin_delay(ms: u32) {
    for _ in 0..ms * 16000 {
        spin_loop();
    }
}
