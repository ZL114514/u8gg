use crate::menu::{BufCanvas, MenuEngine, MenuPage};
use crate::ui::{ButtonState, Element, Font};
use embedded_graphics::{draw_target::DrawTarget, pixelcolor::BinaryColor, prelude::*};
use u8g2_fonts::{FontRenderer, types::FontColor, types::VerticalPosition};

#[derive(Clone, Copy, PartialEq)]
pub enum AppScreen {
    Menu,
    ButtonTest,
}

pub struct ViewModel {
    pub menu: MenuEngine,
    pub screen: AppScreen,
    pub btn: [bool; 4],
}

impl ViewModel {
    pub fn new(root: &'static MenuPage) -> Self {
        Self {
            menu: MenuEngine::new(root),
            screen: AppScreen::Menu,
            btn: [false; 4],
        }
    }

    pub fn handle_key(
        &mut self,
        up: bool,
        down: bool,
        enter: bool,
        back: bool,
        long_back: bool,
        _long_enter: bool,
    ) {
        match self.screen {
            AppScreen::Menu => {
                self.menu.handle_key(up, down, enter, back, long_back);
            }
            AppScreen::ButtonTest => {
                if back {
                    self.screen = AppScreen::Menu;
                    self.menu.dirty = true;
                }
            }
        }
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
                        Element::Button(
                            "K1 UP  ",
                            Font::Ascii6x10,
                            if self.btn[0] {
                                ButtonState::Pressed
                            } else {
                                ButtonState::Normal
                            },
                        ),
                        Element::Button(
                            "K2 DOWN",
                            Font::Ascii6x10,
                            if self.btn[1] {
                                ButtonState::Pressed
                            } else {
                                ButtonState::Normal
                            },
                        ),
                        Element::Button(
                            "K3 BACK",
                            Font::Ascii6x10,
                            if self.btn[2] {
                                ButtonState::Pressed
                            } else {
                                ButtonState::Normal
                            },
                        ),
                        Element::Button(
                            "K4 OK  ",
                            Font::Ascii6x10,
                            if self.btn[3] {
                                ButtonState::Pressed
                            } else {
                                ButtonState::Normal
                            },
                        ),
                    ],
                    4,
                    20,
                    3,
                );
            }
        }
    }

    /// 将 BufCanvas 逐像素拷贝到 display 的 DrawTarget
    pub fn blit<D: DrawTarget<Color = BinaryColor>>(buf: &BufCanvas, target: &mut D) {
        let pixels = (0i32..128).flat_map(move |x| {
            (0i32..64).map(move |y| {
                let on = (buf.0[(y as usize / 8) * 128 + x as usize] >> (y as u8 % 8)) & 1 != 0;
                Pixel(
                    Point::new(x, y),
                    if on {
                        BinaryColor::On
                    } else {
                        BinaryColor::Off
                    },
                )
            })
        });
        let _ = target.draw_iter(pixels);
    }
}
