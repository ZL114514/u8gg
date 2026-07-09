use crate::menu::{BufCanvas, MenuEngine, MenuPage};
use crate::qrcode;
use crate::ui::{ButtonState, Element, Font};
use crate::wifi::WifiStatus;
use core::option::Option as Opt;
use embedded_graphics::{draw_target::DrawTarget, pixelcolor::BinaryColor, prelude::*};
use u8g2_fonts::{FontRenderer, types::FontColor, types::VerticalPosition};

// ===== 字符集 (K1/K2 滚动选择) =====
const CHARSET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ !@#$%^&*()-_+=[]{}|;:',.<>?/`~\"";

#[derive(Clone, Copy, PartialEq)]
pub enum AppScreen {
    Menu,
    ButtonTest,
    /// 显示 WiFi QR 码
    WifiQr,
    /// 手动输入 WiFi (SSID + 密码)
    WifiInput,
}

/// 输入阶段
#[derive(Clone, Copy, PartialEq)]
enum InputPhase {
    /// 正在输 SSID
    Ssid,
    /// 正在输密码
    Password,
    /// 正在连接中
    Connecting,
    /// 已连接
    Connected,
    /// 连接失败
    Failed,
}

const INPUT_BUF_LEN: usize = 32;

pub struct ViewModel {
    pub menu: MenuEngine,
    pub screen: AppScreen,
    pub btn: [bool; 4],
    // WiFi QR 相关
    pub wifi_ssid: &'static str,
    pub wifi_pass: &'static str,
    cached_qr: Opt<qrcode::QrCode>,
    // WiFi 连接状态 (从主循环写入)
    wifi_status: WifiStatus,
    wifi_connected: bool,
    wifi_connect_pending: bool,
    // WiFi 输入相关
    input_phase: InputPhase,
    /// SSID 输入缓冲区
    ssid_buf: [u8; INPUT_BUF_LEN],
    ssid_len: u8,
    /// 密码输入缓冲区
    pass_buf: [u8; INPUT_BUF_LEN],
    pass_len: u8,
    /// 当前字符选择器索引 (指向 CHARSET)
    char_idx: u8,
    /// 进入密码还是 SSID 的闪烁光标
    show_cursor: bool,
    tick_count: u8,
}

impl ViewModel {
    pub fn new(root: &'static MenuPage) -> Self {
        Self {
            menu: MenuEngine::new(root),
            screen: AppScreen::Menu,
            btn: [false; 4],
            wifi_ssid: "",
            wifi_pass: "",
            cached_qr: Opt::None,
            wifi_status: WifiStatus::Idle,
            wifi_connected: false,
            wifi_connect_pending: false,
            input_phase: InputPhase::Ssid,
            ssid_buf: [0; INPUT_BUF_LEN],
            ssid_len: 0,
            pass_buf: [0; INPUT_BUF_LEN],
            pass_len: 0,
            char_idx: 0,
            show_cursor: true,
            tick_count: 0,
        }
    }

    pub fn set_wifi(&mut self, ssid: &'static str, pass: &'static str) {
        self.wifi_ssid = ssid;
        self.wifi_pass = pass;
    }

    pub fn handle_key(
        &mut self,
        up: bool,
        down: bool,
        enter: bool,
        back: bool,
        long_back: bool,
        long_enter: bool,
    ) {
        match self.screen {
            AppScreen::Menu => {
                self.menu.handle_key(up, down, enter, back, long_back);

                // 检查 SHOW_QR 触发
                if self.menu.bind_bools[super::SHOW_QR] {
                    self.menu.bind_bools[super::SHOW_QR] = false;
                    if self.cached_qr.is_none() && !self.wifi_ssid.is_empty() {
                        self.cached_qr = qrcode::encode_wifi(self.wifi_ssid, self.wifi_pass).ok();
                    }
                    self.screen = AppScreen::WifiQr;
                    self.menu.dirty = true;
                }
                // 检查 WIFI_MANUAL 触发
                if self.menu.bind_bools[super::WIFI_MANUAL] {
                    self.menu.bind_bools[super::WIFI_MANUAL] = false;
                    self.reset_input();
                    self.screen = AppScreen::WifiInput;
                    self.menu.dirty = true;
                }
            }
            AppScreen::ButtonTest => {
                if back {
                    self.screen = AppScreen::Menu;
                    self.menu.dirty = true;
                }
            }
            AppScreen::WifiQr => {
                if back || long_back || enter {
                    self.screen = AppScreen::Menu;
                    self.menu.dirty = true;
                }
            }
            AppScreen::WifiInput => {
                match self.input_phase {
                    InputPhase::Ssid | InputPhase::Password => {
                        let buf = if self.input_phase == InputPhase::Ssid {
                            &mut self.ssid_buf
                        } else {
                            &mut self.pass_buf
                        };
                        let len = if self.input_phase == InputPhase::Ssid {
                            &mut self.ssid_len
                        } else {
                            &mut self.pass_len
                        };

                        // 字符滚动
                        if up {
                            // 上升到前一个字符
                            if self.char_idx > 0 {
                                self.char_idx -= 1;
                            } else {
                                self.char_idx = (CHARSET.len() - 1) as u8;
                            }
                            self.menu.dirty = true;
                        }
                        if down {
                            self.char_idx = ((self.char_idx as usize + 1) % CHARSET.len()) as u8;
                            self.menu.dirty = true;
                        }

                        // 确认字符 (Enter = K4)
                        if enter && (*len as usize) < INPUT_BUF_LEN {
                            buf[*len as usize] = CHARSET[self.char_idx as usize];
                            *len += 1;
                            self.char_idx = 0;
                            self.menu.dirty = true;
                        }

                        // 退格 (Back = K3)
                        if back && *len > 0 {
                            *len -= 1;
                            buf[*len as usize] = 0;
                            self.char_idx = 0;
                            self.menu.dirty = true;
                        }

                        // 长按 K4 = 下一步/连接
                        if long_enter {
                            match self.input_phase {
                                InputPhase::Ssid => {
                                    if self.ssid_len > 0 {
                                        self.input_phase = InputPhase::Password;
                                        self.char_idx = 0;
                                        self.menu.dirty = true;
                                    }
                                }
                                InputPhase::Password => {
                                    // 开始连接
                                    if self.ssid_len > 0 {
                                        self.input_phase = InputPhase::Connecting;
                                        self.wifi_connect_pending = true;
                                        self.menu.dirty = true;
                                    }
                                }
                                _ => {}
                            }
                        }
                        // 长按 K3 = 退出输入
                        if long_back {
                            self.reset_input();
                            self.screen = AppScreen::Menu;
                            self.menu.dirty = true;
                        }
                    }
                    InputPhase::Connecting => {
                        // 连接中——按 Back 取消
                        if back || long_back {
                            self.reset_input();
                            self.screen = AppScreen::Menu;
                            self.menu.dirty = true;
                        }
                    }
                    InputPhase::Connected | InputPhase::Failed => {
                        // 完成/失败——任意键返回
                        if back || enter || long_back {
                            self.reset_input();
                            self.screen = AppScreen::Menu;
                            self.menu.dirty = true;
                        }
                    }
                }
            }
        }
    }

    fn reset_input(&mut self) {
        self.input_phase = InputPhase::Ssid;
        self.ssid_len = 0;
        self.ssid_buf = [0; INPUT_BUF_LEN];
        self.pass_len = 0;
        self.pass_buf = [0; INPUT_BUF_LEN];
        self.char_idx = 0;
    }

    /// 主循环每帧调用，驱动异步操作 (扫描/连接)
    pub fn tick(&mut self) {
        self.tick_count = self.tick_count.wrapping_add(1);
        self.show_cursor = self.tick_count & 8 != 0; // 约 8 帧闪烁
    }

    pub fn render(&mut self, buf: &mut BufCanvas) {
        match self.screen {
            AppScreen::Menu => {
                self.menu.render_buf(buf);
            }
            AppScreen::ButtonTest => {
                buf.clear();
                let r6 = FontRenderer::new::<u8g2_fonts::fonts::u8g2_font_6x10_tf>()
                    .with_ignore_unknown_chars(true);
                let _ = r6.render(
                    "--- 按键测试 ---",
                    Point::new(4, 2),
                    VerticalPosition::Top,
                    FontColor::Transparent(BinaryColor::On),
                    buf,
                );
                crate::ui::render(
                    buf,
                    &[
                        Element::Button("K1 UP  ", Font::Ascii6x10, if self.btn[0] { ButtonState::Pressed } else { ButtonState::Normal }),
                        Element::Button("K2 DOWN", Font::Ascii6x10, if self.btn[1] { ButtonState::Pressed } else { ButtonState::Normal }),
                        Element::Button("K3 BACK", Font::Ascii6x10, if self.btn[2] { ButtonState::Pressed } else { ButtonState::Normal }),
                        Element::Button("K4 OK  ", Font::Ascii6x10, if self.btn[3] { ButtonState::Pressed } else { ButtonState::Normal }),
                    ],
                    4, 20, 3,
                );
            }
            AppScreen::WifiQr => {
                buf.clear();
                let r6 = FontRenderer::new::<u8g2_fonts::fonts::u8g2_font_6x10_tf>()
                    .with_ignore_unknown_chars(true);

                // 顶部显示 SSID
                /*let label = if self.wifi_ssid.len() > 12 { &self.wifi_ssid[..12] } else { self.wifi_ssid };
                let _ = r6.render(label, Point::new(4, 0), VerticalPosition::Top,
                    FontColor::Transparent(BinaryColor::On), buf);*/

                if let Some(ref qr) = self.cached_qr {
                    let ms = 2u32;
                    let quiet = 3u32;
                    let total = qr.size as u32 * ms + quiet * 2;
                    let ox = (128i32 - total as i32) / 2;
                    let oy = (64i32 - total as i32) / 2;
                    for y in 0..qr.size as u32 {
                        for x in 0..qr.size as u32 {
                            if qr.get(x as usize, y as usize) {
                                buf.invert_rect(
                                    ox + quiet as i32 + (x * ms) as i32,
                                    oy + quiet as i32 + (y * ms) as i32,
                                    ms, ms, 0,
                                );
                            }
                        }
                    }
                } else {
                    let _ = r6.render("生成失败", Point::new(40, 28), VerticalPosition::Top,
                        FontColor::Transparent(BinaryColor::On), buf);
                }
            }
            AppScreen::WifiInput => {
                buf.clear();
                let r6 = FontRenderer::new::<u8g2_fonts::fonts::u8g2_font_6x10_tf>()
                    .with_ignore_unknown_chars(true);

                match self.input_phase {
                    InputPhase::Ssid => {
                        let _ = r6.render("SSID:", Point::new(0, 0), VerticalPosition::Top,
                            FontColor::Transparent(BinaryColor::On), buf);
                        // 显示已输入的 SSID
                        let s = core::str::from_utf8(&self.ssid_buf[..self.ssid_len as usize]).unwrap_or("");
                        let _ = r6.render(s, Point::new(0, 12), VerticalPosition::Top,
                            FontColor::Transparent(BinaryColor::On), buf);
                        // 闪烁光标
                        if self.show_cursor {
                            let cx = (s.len() as i32 * 6).min(120);
                            let _ = buf.invert_rect(cx, 12, 6, 10, 1);
                        }
                        // 底部提示
                        let _ = r6.render("K4=确认 K3=退格", Point::new(0, 52), VerticalPosition::Top,
                            FontColor::Transparent(BinaryColor::On), buf);
                        let _ = r6.render("长K4=下一步 长K3=退出", Point::new(0, 62), VerticalPosition::Top,
                            FontColor::Transparent(BinaryColor::On), buf);
                    }
                    InputPhase::Password => {
                        let _ = r6.render("密码:", Point::new(0, 0), VerticalPosition::Top,
                            FontColor::Transparent(BinaryColor::On), buf);
                        // 密码用 * 显示
                        let mut masked = [b'*'; INPUT_BUF_LEN];
                        let masked_str = core::str::from_utf8(&masked[..self.pass_len as usize]).unwrap_or("");
                        let _ = r6.render(masked_str, Point::new(0, 12), VerticalPosition::Top,
                            FontColor::Transparent(BinaryColor::On), buf);
                        if self.show_cursor && self.pass_len < INPUT_BUF_LEN as u8 {
                            let cx = (self.pass_len as i32 * 6).min(120);
                            let _ = buf.invert_rect(cx, 12, 6, 10, 1);
                        }
                        let _ = r6.render("K4=确认 K3=退格", Point::new(0, 52), VerticalPosition::Top,
                            FontColor::Transparent(BinaryColor::On), buf);
                        let _ = r6.render("长K4=连接 长K3=退出", Point::new(0, 62), VerticalPosition::Top,
                            FontColor::Transparent(BinaryColor::On), buf);
                    }
                    InputPhase::Connecting => {
                        let _ = r6.render("正在连接 WiFi...", Point::new(0, 24), VerticalPosition::Top,
                            FontColor::Transparent(BinaryColor::On), buf);
                        let s = core::str::from_utf8(&self.ssid_buf[..self.ssid_len as usize]).unwrap_or("");
                        let _ = r6.render(s, Point::new(0, 36), VerticalPosition::Top,
                            FontColor::Transparent(BinaryColor::On), buf);
                        // 点动画
                        let dots = match (self.tick_count / 10) % 4 {
                            0 => "   ", 1 => ".  ", 2 => ".. ", _ => "...",
                        };
                        let _ = r6.render(dots, Point::new(80, 24), VerticalPosition::Top,
                            FontColor::Transparent(BinaryColor::On), buf);
                    }
                    InputPhase::Connected => {
                        let _ = r6.render("✓ 已连接!", Point::new(0, 24), VerticalPosition::Top,
                            FontColor::Transparent(BinaryColor::On), buf);
                    }
                    InputPhase::Failed => {
                        let _ = r6.render("✗ 连接失败", Point::new(0, 24), VerticalPosition::Top,
                            FontColor::Transparent(BinaryColor::On), buf);
                        let _ = r6.render("按任意键返回", Point::new(0, 36), VerticalPosition::Top,
                            FontColor::Transparent(BinaryColor::On), buf);
                    }
                }

                // 在 SSID/密码输入模式显示当前选中字符
                if matches!(self.input_phase, InputPhase::Ssid | InputPhase::Password) {
                    let ch = CHARSET[self.char_idx as usize] as char;
                    let mut ch_str = [0u8; 4];
                    let ch_s: &str = ch.encode_utf8(&mut ch_str);
                    let _ = r6.render(ch_s, Point::new(64, 46), VerticalPosition::Top,
                        FontColor::Transparent(BinaryColor::On), buf);
                }
            }
        }
    }

    pub fn blit<D: DrawTarget<Color = BinaryColor>>(buf: &BufCanvas, target: &mut D) {
        let pixels = (0i32..128).flat_map(move |x| {
            (0i32..64).map(move |y| {
                let on = (buf.0[(y as usize / 8) * 128 + x as usize] >> (y as u8 % 8)) & 1 != 0;
                Pixel(Point::new(x, y), if on { BinaryColor::On } else { BinaryColor::Off })
            })
        });
        let _ = target.draw_iter(pixels);
    }

    /// 获取输入的 SSID 字符串
    pub fn input_ssid(&self) -> &str {
        core::str::from_utf8(&self.ssid_buf[..self.ssid_len as usize]).unwrap_or("")
    }

    /// 获取输入的密码字符串
    pub fn input_pass(&self) -> &str {
        core::str::from_utf8(&self.pass_buf[..self.pass_len as usize]).unwrap_or("")
    }

    /// 检查并消费 WiFi 连接请求 (被主循环调用)
    pub fn consume_wifi_connect(&mut self) -> bool {
        let p = self.wifi_connect_pending;
        self.wifi_connect_pending = false;
        p
    }

    /// 主循环更新 WiFi 状态
    pub fn set_wifi_status(&mut self, status: WifiStatus) {
        self.wifi_status = status;
    }

    /// 主循环更新 WiFi 连接状态
    pub fn set_wifi_connected(&mut self, connected: bool) {
        self.wifi_connected = connected;
        if connected && self.input_phase == InputPhase::Connecting {
            self.input_phase = InputPhase::Connected;
        } else if !connected && self.input_phase == InputPhase::Connected {
            // 断线暂时不管, 留给 wifi::tick 重连
        }
    }
}
