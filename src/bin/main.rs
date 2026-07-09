#![no_std]
#![no_main]

use core::hint::spin_loop;
use esp_backtrace as _;
use esp_bootloader_esp_idf::esp_app_desc;
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

// ===== 堆分配器 (esp-radio + esp-rtos 需要) =====
use esp_alloc as _;
#[unsafe(no_mangle)]
static mut HEAP: core::mem::MaybeUninit<[u8; 72 * 1024]> = core::mem::MaybeUninit::uninit();
unsafe fn init_heap() {
    esp_alloc::HEAP.add_region(esp_alloc::HeapRegion::new(
        HEAP.as_mut_ptr() as *mut u8,
        72 * 1024,
        esp_alloc::MemoryCapability::Internal.into(),
    ));
}
use ssd1306::{I2CDisplayInterface, Ssd1306, prelude::*};

#[path = "../menu.rs"]
mod menu;
#[path = "../ui.rs"]
mod ui;
#[path = "../viewmodel.rs"]
mod viewmodel;

use menu::{BufCanvas, MenuEngine, MenuItem, MenuPage};
use viewmodel::ViewModel;

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
define_binds! { FPS, SLIDER, TOGGLE }

// ===== 菜单数据 =====
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
        MenuItem::Label {
            label: "u8gg UI v0.3",
        },
        MenuItem::Label {
            label: "ESP32 + SSD1306",
        },
        MenuItem::Label {
            label: "Inspired from OLED_UI",
        },
        MenuItem::Label {
            label: "Apache-2.0",
        },
    ],
};
static PAGE_MAIN: MenuPage = MenuPage {
    title: "主菜单",
    items: &[
        MenuItem::Label { label: "按键" },
        MenuItem::Button {
            label: "Hello",
            action: |e| e.show_toast("你好！Toast !", 30),
        },
        MenuItem::Button {
            label: "SV",
            action: |e| {
                let v = e.bind_u8s[SLIDER];
                e.toast_buf = [0; 20];
                e.toast_buf[..3].copy_from_slice(b"SV=");
                e.toast_buf[3] = b'0' + v / 10;
                e.toast_buf[4] = b'0' + v % 10;
                e.toast_buf_len = 5;
                e.show_toast("", 20);
                e.toast_msg = ::core::option::Option::None;
            },
        },
        MenuItem::Slider {
            label: "滑条",
            bind: SLIDER,
            min: 0,
            max: 100,
        },
        MenuItem::Toggle {
            label: "一个开关",
            bind: TOGGLE,
        },
        MenuItem::Toggle {
            label: "帧率显示",
            bind: FPS,
        },
        MenuItem::Submenu {
            label: "子菜单测试",
            page: &PAGE_SUB,
        },
        MenuItem::Submenu {
            label: "关于",
            page: &PAGE_ABOUT,
        },
    ],
};

#[main]
fn main() -> ! {
    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    // 初始化堆分配器 (必须在任何堆分配调用之前)
    unsafe { init_heap(); }
    let delay = Delay::new();
    println!("[u8gg] Boot OK");

    // ===== 启动 RTOS 调度器 (esp-radio 要求，必须在任何 WiFi 调用之前) =====
    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let sw_int = SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_int.software_interrupt0);
    println!("[u8gg] RTOS started");

    // ===== WiFi 连接 (参考 ESP-IDF DPP Enrollee 例子的事件驱动模式) =====
    // 取消注释并填入你的 SSID/密码即可使用:
    // let (mut wifi_ctrl, _interfaces) = esp_radio::wifi::new(
    //     peripherals.WIFI,
    //     Default::default(),
    // ).unwrap();
    // wifi::connect_blocking(&mut wifi_ctrl, "YourSSID", "YourPassword", &delay);
    // let mut wifi_retry = 0u8;
    // 然后在主循环中加入: wifi::tick(&mut wifi_ctrl, &mut wifi_retry);

    let mut led = Output::new(peripherals.GPIO2, Level::Low, OutputConfig::default());
    led.set_high();
    spin_delay(200);

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
        I2cConfig::default().with_frequency(Rate::from_khz(600)),
    )
    .unwrap()
    .with_sda(peripherals.GPIO21)
    .with_scl(peripherals.GPIO22);
    let addr_ok = i2c.write(0x3D, &[0x00]).is_ok();
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
    let mut loop_tick = 0u16;
    let mut render_tick = 0u16;

    loop {
        loop_tick += 1;
        if loop_tick >= 50 {
            vm.menu.fps = render_tick.min(99);
            render_tick = 0;
            loop_tick = 0;
        }

        let c = [k1.is_low(), k2.is_low(), k3.is_low(), k4.is_low()];
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
            edge[0] || repeat_up,
            edge[1] || repeat_down,
            edge[3],
            edge[2],
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

fn spin_delay(ms: u32) {
    for _ in 0..ms * 16000 {
        spin_loop();
    }
}
