use crate::menu::{BufCanvas, MenuEngine, MenuPage};
use embedded_graphics::{geometry::Point, pixelcolor::BinaryColor, prelude::*};
use u8g2_fonts::{FontRenderer, types::FontColor, types::VerticalPosition};

// ===== 字符集 (K1/K2 滚动选择) =====
const CHARSET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ !@#$%^&*()-_+=[]{}|;:',.<>?/`~\\\"\"";

#[derive(Clone, Copy, PartialEq)]
pub enum AppScreen {
    Menu,
    WifiInput,
}

#[derive(Clone, Copy, PartialEq)]
enum InputPhase {
    Ssid,
    Password,
    Connecting,
    Connected,
    Failed,
}

const INPUT_BUF_LEN: usize = 32;

pub struct ViewModel {
    pub menu: MenuEngine,
    pub screen: AppScreen,
    wifi_connected: bool,
    pub wifi_connect_pending: bool,
    input_phase: InputPhase,
    ssid_buf: [u8; INPUT_BUF_LEN],
    pub ssid_len: u8,
    pass_buf: [u8; INPUT_BUF_LEN],
    pass_len: u8,
    char_idx: u8,
    show_cursor: bool,
    tick_count: u8,
}

impl ViewModel {
    pub fn new(root: &'static MenuPage) -> Self {
        Self {
            menu: MenuEngine::new(root),
            screen: AppScreen::Menu,
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

    /// 从扫描结果填充 SSID buffer
    pub fn ssid_buf_fill(&mut self, ssid: &str) {
        self.ssid_len = 0;
        for &b in ssid.as_bytes() {
            if (self.ssid_len as usize) < INPUT_BUF_LEN {
                self.ssid_buf[self.ssid_len as usize] = b;
                self.ssid_len += 1;
            }
        }
    }

    /// 进入密码输入阶段
    pub fn start_password_input(&mut self) {
        self.pass_len = 0;
        self.pass_buf = [0; INPUT_BUF_LEN];
        self.char_idx = 0;
        self.input_phase = InputPhase::Password;
        self.screen = AppScreen::WifiInput;
        self.menu.dirty = true;
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
            }
            AppScreen::WifiInput => match self.input_phase {
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
                    if up {
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
                    if enter && (*len as usize) < INPUT_BUF_LEN {
                        buf[*len as usize] = CHARSET[self.char_idx as usize];
                        *len += 1;
                        self.char_idx = 0;
                        self.menu.dirty = true;
                    }
                    if back {
                        if *len > 0 {
                            *len -= 1;
                            buf[*len as usize] = 0;
                            self.char_idx = 0;
                            self.menu.dirty = true;
                        } else {
                            self.reset_input();
                            self.screen = AppScreen::Menu;
                            self.menu.dirty = true;
                        }
                    }
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
                                if self.ssid_len > 0 {
                                    self.input_phase = InputPhase::Connecting;
                                    self.wifi_connect_pending = true;
                                    self.menu.dirty = true;
                                }
                            }
                            _ => {}
                        }
                    }
                    if long_back {
                        self.reset_input();
                        self.screen = AppScreen::Menu;
                        self.menu.dirty = true;
                    }
                }
                InputPhase::Connecting => {
                    if back || long_back {
                        self.reset_input();
                        self.screen = AppScreen::Menu;
                        self.menu.dirty = true;
                    }
                }
                InputPhase::Connected | InputPhase::Failed => {
                    if back || enter || long_back {
                        self.reset_input();
                        self.screen = AppScreen::Menu;
                        self.menu.dirty = true;
                    }
                }
            },
        }
    }

    pub fn reset_input(&mut self) {
        self.input_phase = InputPhase::Ssid;
        self.ssid_len = 0;
        self.ssid_buf = [0; INPUT_BUF_LEN];
        self.pass_len = 0;
        self.pass_buf = [0; INPUT_BUF_LEN];
        self.char_idx = 0;
    }

    pub fn tick(&mut self) {
        self.tick_count = self.tick_count.wrapping_add(1);
        self.show_cursor = self.tick_count & 8 != 0;
    }

    pub fn render(&mut self, buf: &mut BufCanvas) {
        match self.screen {
            AppScreen::Menu => {
                self.menu.render_buf(buf);
            }
            AppScreen::WifiInput => {
                buf.clear();
                let r6 = FontRenderer::new::<u8g2_fonts::fonts::u8g2_font_6x10_tf>()
                    .with_ignore_unknown_chars(true);
                match self.input_phase {
                    InputPhase::Ssid => {
                        let _ = r6.render(
                            "SSID:",
                            Point::new(0, 0),
                            VerticalPosition::Top,
                            FontColor::Transparent(BinaryColor::On),
                            buf,
                        );
                        let s = core::str::from_utf8(&self.ssid_buf[..self.ssid_len as usize])
                            .unwrap_or("");
                        let _ = r6.render(
                            s,
                            Point::new(0, 12),
                            VerticalPosition::Top,
                            FontColor::Transparent(BinaryColor::On),
                            buf,
                        );
                        if self.show_cursor {
                            let cx = (s.len() as i32 * 6).min(120);
                            buf.invert_rect(cx, 12, 6, 10, 1);
                        }
                        let _ = r6.render(
                            "K4=确认 K3=退格",
                            Point::new(0, 52),
                            VerticalPosition::Top,
                            FontColor::Transparent(BinaryColor::On),
                            buf,
                        );
                        let _ = r6.render(
                            "长K4=下一步 长K3=退出",
                            Point::new(0, 62),
                            VerticalPosition::Top,
                            FontColor::Transparent(BinaryColor::On),
                            buf,
                        );
                    }
                    InputPhase::Password => {
                        let _ = r6.render(
                            "密码:",
                            Point::new(0, 0),
                            VerticalPosition::Top,
                            FontColor::Transparent(BinaryColor::On),
                            buf,
                        );
                        let masked_str = core::str::from_utf8(
                            &[b'*'; INPUT_BUF_LEN][..self.pass_len as usize],
                        )
                        .unwrap_or("");
                        let _ = r6.render(
                            masked_str,
                            Point::new(0, 12),
                            VerticalPosition::Top,
                            FontColor::Transparent(BinaryColor::On),
                            buf,
                        );
                        if self.show_cursor && self.pass_len < INPUT_BUF_LEN as u8 {
                            let cx = (self.pass_len as i32 * 6).min(120);
                            buf.invert_rect(cx, 12, 6, 10, 1);
                        }
                        let _ = r6.render(
                            "K4=确认 K3=退格",
                            Point::new(0, 52),
                            VerticalPosition::Top,
                            FontColor::Transparent(BinaryColor::On),
                            buf,
                        );
                        let _ = r6.render(
                            "长K4=连接 长K3=退出",
                            Point::new(0, 62),
                            VerticalPosition::Top,
                            FontColor::Transparent(BinaryColor::On),
                            buf,
                        );
                    }
                    InputPhase::Connecting => {
                        let _ = r6.render(
                            "正在连接 WiFi...",
                            Point::new(0, 24),
                            VerticalPosition::Top,
                            FontColor::Transparent(BinaryColor::On),
                            buf,
                        );
                        let s =
                            core::str::from_utf8(&self.ssid_buf[..self.ssid_len as usize])
                                .unwrap_or("");
                        let _ = r6.render(
                            s,
                            Point::new(0, 36),
                            VerticalPosition::Top,
                            FontColor::Transparent(BinaryColor::On),
                            buf,
                        );
                        let dots = match (self.tick_count / 10) % 4 {
                            0 => "   ",
                            1 => ".  ",
                            2 => ".. ",
                            _ => "...",
                        };
                        let _ = r6.render(
                            dots,
                            Point::new(80, 24),
                            VerticalPosition::Top,
                            FontColor::Transparent(BinaryColor::On),
                            buf,
                        );
                    }
                    InputPhase::Connected => {
                        let _ = r6.render(
                            "✓ 已连接!",
                            Point::new(0, 24),
                            VerticalPosition::Top,
                            FontColor::Transparent(BinaryColor::On),
                            buf,
                        );
                    }
                    InputPhase::Failed => {
                        let _ = r6.render(
                            "✗ 连接失败",
                            Point::new(0, 24),
                            VerticalPosition::Top,
                            FontColor::Transparent(BinaryColor::On),
                            buf,
                        );
                        let _ = r6.render(
                            "按任意键返回",
                            Point::new(0, 36),
                            VerticalPosition::Top,
                            FontColor::Transparent(BinaryColor::On),
                            buf,
                        );
                    }
                }
                if matches!(self.input_phase, InputPhase::Ssid | InputPhase::Password) {
                    let ch = CHARSET[self.char_idx as usize] as char;
                    let mut ch_str = [0u8; 4];
                    let ch_s: &str = ch.encode_utf8(&mut ch_str);
                    let _ = r6.render(
                        ch_s,
                        Point::new(64, 46),
                        VerticalPosition::Top,
                        FontColor::Transparent(BinaryColor::On),
                        buf,
                    );
                }
            }
        }
    }

    pub fn blit<D: embedded_graphics::draw_target::DrawTarget<Color = embedded_graphics::pixelcolor::BinaryColor>>(
        buf: &BufCanvas,
        target: &mut D,
    ) {
        use embedded_graphics::pixelcolor::BinaryColor;
        let pixels = (0i32..128).flat_map(move |x| {
            (0i32..64).map(move |y| {
                let on = (buf.0[(y as usize / 8) * 128 + x as usize] >> (y as u8 % 8)) & 1 != 0;
                embedded_graphics::Pixel(
                    embedded_graphics::geometry::Point::new(x, y),
                    if on { BinaryColor::On } else { BinaryColor::Off },
                )
            })
        });
        let _ = target.draw_iter(pixels);
    }

    pub fn input_ssid(&self) -> &str {
        core::str::from_utf8(&self.ssid_buf[..self.ssid_len as usize]).unwrap_or("")
    }
    pub fn input_pass(&self) -> &str {
        core::str::from_utf8(&self.pass_buf[..self.pass_len as usize]).unwrap_or("")
    }
    pub fn consume_wifi_connect(&mut self) -> bool {
        let p = self.wifi_connect_pending;
        self.wifi_connect_pending = false;
        p
    }
    pub fn is_wifi_connected(&self) -> bool {
        self.wifi_connected
    }
    pub fn set_wifi_connected(&mut self, connected: bool) {
        self.wifi_connected = connected;
        if connected && self.input_phase == InputPhase::Connecting {
            self.input_phase = InputPhase::Connected;
        }
    }
}
