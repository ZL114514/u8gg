//! QR Code rendering component for 128x64 OLED displays.

use embedded_graphics::{
    draw_target::DrawTarget,
    pixelcolor::BinaryColor,
    prelude::*,
    primitives::{PrimitiveStyle, Rectangle},
};

/// Render a QR code centered on a 128x64 target using standard embedded-graphics primitives.
/// Draws black modules on the target.
pub fn render_qr<D: DrawTarget<Color = BinaryColor>>(
    target: &mut D,
    qr: &embedded_qr::QrMatrix<embedded_qr::Version10>,
    module_size: u32,
    quiet_zone: u32,
) {
    let total = qr.width() as u32 * module_size + quiet_zone * 2;
    let ox = (128i32 - total as i32) / 2;
    let oy = (64i32 - total as i32) / 2;
    for y in 0..qr.width() as u32 {
        for x in 0..qr.width() as u32 {
            if qr.get(x as usize, y as usize) {
                let _ = Rectangle::new(
                    Point::new(
                        ox + quiet_zone as i32 + (x * module_size) as i32,
                        oy + quiet_zone as i32 + (y * module_size) as i32,
                    ),
                    Size::new(module_size, module_size),
                )
                .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
                .draw(target);
            }
        }
    }
}
