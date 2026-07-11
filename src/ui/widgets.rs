use embedded_graphics::{
    draw_target::DrawTarget,
    pixelcolor::BinaryColor,
    prelude::*,
    primitives::{Circle, CornerRadii, PrimitiveStyle, Rectangle, RoundedRectangle},
};
use u8g2_fonts::{FontRenderer, types::FontColor, types::VerticalPosition};

/// 字体预设
pub enum Font {
    Ascii6x10,
    CnWqy12,
    CnUnifont16,
}

/// 按钮状态
#[derive(Clone, Copy)]
pub enum ButtonState {
    Normal,
    Pressed,
    Active,
}

/// UI 元素
pub enum Element<'a> {
    /// 文本
    Text(&'a str, Font, FontColor<BinaryColor>),
    /// 空白
    Spacer(i32),
    /// 按钮 (标签, 字体, 状态)
    Button(&'a str, Font, ButtonState),
    /// 滑条 (当前值, 最大值, 宽度px)
    Slider(u8, u8, i32),
    /// 开关 (状态, 标签)
    Switch(bool, &'a str),
}

fn line_height(font: &Font) -> i32 {
    match font {
        Font::Ascii6x10 => 10,
        Font::CnWqy12 => 14,
        Font::CnUnifont16 => 18,
    }
}

/// 垂直流式布局
pub fn render<D>(target: &mut D, elements: &[Element], start_x: i32, start_y: i32, spacing: i32)
where
    D: DrawTarget<Color = BinaryColor>,
{
    let r6 =
        FontRenderer::new::<u8g2_fonts::fonts::u8g2_font_6x10_tf>().with_ignore_unknown_chars(true);
    let rwqy = FontRenderer::new::<u8g2_fonts::fonts::u8g2_font_wqy12_t_chinese1>()
        .with_ignore_unknown_chars(true);
    let _runi = FontRenderer::new::<u8g2_fonts::fonts::u8g2_font_unifont_t_chinese1>()
        .with_ignore_unknown_chars(true);

    let mut y = start_y;
    for elem in elements {
        match elem {
            Element::Text(text, font, color) => {
                let r = match font {
                    Font::Ascii6x10 => &r6,
                    Font::CnWqy12 => &rwqy,
                    Font::CnUnifont16 => &_runi,
                };
                let _ = r.render(
                    *text,
                    Point::new(start_x, y),
                    VerticalPosition::Top,
                    *color,
                    target,
                );
                y += line_height(font) + spacing;
            }
            Element::Spacer(h) => y += h,
            Element::Button(label, font, state) => {
                let h = line_height(font) + 6;
                let w = 128 - start_x as u32 - 2;
                let btn_rect = Rectangle::new(Point::new(start_x + 1, y), Size::new(w, h as u32));
                // 填充背景（按下的按钮填黑）
                if matches!(state, ButtonState::Pressed | ButtonState::Active) {
                    let _ = btn_rect
                        .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
                        .draw(target);
                }
                // 画边框
                let fill = match state {
                    ButtonState::Normal => BinaryColor::On,
                    ButtonState::Pressed => BinaryColor::Off,
                    ButtonState::Active => BinaryColor::On,
                };
                let _ = RoundedRectangle::new(btn_rect, CornerRadii::new(Size::new(3, 3)))
                    .into_styled(PrimitiveStyle::with_stroke(fill, 1))
                    .draw(target);
                // 居中文字
                let r = match font {
                    Font::Ascii6x10 => &r6,
                    Font::CnWqy12 => &rwqy,
                    Font::CnUnifont16 => &_runi,
                };
                let txt_color = if matches!(state, ButtonState::Pressed) {
                    BinaryColor::Off
                } else {
                    BinaryColor::On
                };
                let _ = r.render(
                    *label,
                    Point::new(start_x + 2, y + 3),
                    VerticalPosition::Top,
                    FontColor::Transparent(txt_color),
                    target,
                );
                y += h + spacing;
            }
            Element::Slider(value, max_val, width) => {
                let w = *width as u32;
                let h = 8u32;
                let sx = start_x + 2;
                let track_y = y + 2;
                // 轨道
                let _ = Rectangle::new(Point::new(sx, track_y), Size::new(w, 3))
                    .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
                    .draw(target);
                let fill_w = if *max_val > 0 {
                    (w as u32 * *value as u32) / *max_val as u32
                } else {
                    0
                };
                if fill_w > 0 {
                    let _ = Rectangle::new(Point::new(sx + 1, track_y + 1), Size::new(fill_w, 1))
                        .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
                        .draw(target);
                }
                // 滑块
                let knob_x = if *max_val > 0 {
                    sx as i32 + (w as i32 * *value as i32) / *max_val as i32
                } else {
                    sx as i32
                };
                let _ = Circle::new(Point::new(knob_x, track_y as i32 + 1), 3)
                    .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
                    .draw(target);
                y += h as i32 + spacing;
            }
            Element::Switch(on, label) => {
                let sw_x = start_x + 80;
                let sw_y = y;
                let sw_w = 22u32;
                let sw_h = 12u32;
                // 开关背景
                let _ = RoundedRectangle::new(
                    Rectangle::new(Point::new(sw_x, sw_y), Size::new(sw_w, sw_h)),
                    CornerRadii::new(Size::new(6, 6)),
                )
                .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
                .draw(target);
                if *on {
                    let _ = RoundedRectangle::new(
                        Rectangle::new(
                            Point::new(sw_x + 1, sw_y + 1),
                            Size::new(sw_w - 2, sw_h - 2),
                        ),
                        CornerRadii::new(Size::new(5, 5)),
                    )
                    .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
                    .draw(target);
                }
                // 滑钮
                let knob_x = if *on {
                    sw_x as i32 + sw_w as i32 - sw_h as i32 - 2i32
                } else {
                    sw_x as i32
                };
                let _ = Circle::new(
                    Point::new(knob_x + sw_h as i32 / 2, sw_y + sw_h as i32 / 2),
                    sw_h / 2 - 1,
                )
                .into_styled(PrimitiveStyle::with_fill(if *on {
                    BinaryColor::Off
                } else {
                    BinaryColor::On
                }));
                // 标签
                let _ = r6.render(
                    *label,
                    Point::new(start_x, y + 1),
                    VerticalPosition::Top,
                    FontColor::Transparent(BinaryColor::On),
                    target,
                );
                y += sw_h as i32 + spacing;
            }
        }
    }
}
