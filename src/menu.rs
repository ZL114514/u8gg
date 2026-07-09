use embedded_graphics::{
    draw_target::DrawTarget,
    pixelcolor::BinaryColor,
    prelude::*,
    primitives::{Circle, CornerRadii, PrimitiveStyle, Rectangle, RoundedRectangle},
};
use u8g2_fonts::{FontRenderer, types::FontColor, types::VerticalPosition};

pub struct BufCanvas(pub [u8; 1024]);
impl BufCanvas {
    pub fn new() -> Self {
        Self([0; 1024])
    }
    pub fn clear(&mut self) {
        self.0 = [0; 1024];
    }
    fn set_pixel(&mut self, x: i32, y: i32, val: bool) {
        if x < 0 || x >= 128 || y < 0 || y >= 64 {
            return;
        }
        let idx = (y as usize / 8) * 128 + x as usize;
        let bit = y as u8 % 8;
        if val {
            self.0[idx] |= 1 << bit;
        } else {
            self.0[idx] &= !(1 << bit);
        }
    }
    pub fn invert_rect(&mut self, x: i32, y: i32, w: u32, h: u32, r: u32) {
        let x1 = x.max(0);
        let y1 = y.max(0);
        let x2 = (x + w as i32).min(128);
        let y2 = (y + h as i32).min(64);
        let rr = r.min(w / 2).min(h / 2) as i32;
        for px in x1..x2 {
            for py in y1..y2 {
                let dx = if px < x + rr {
                    x + rr - px
                } else if px >= x + w as i32 - rr {
                    px - (x + w as i32 - 1 - rr)
                } else {
                    0
                };
                let dy = if py < y + rr {
                    y + rr - py
                } else if py >= y + h as i32 - rr {
                    py - (y + h as i32 - 1 - rr)
                } else {
                    0
                };
                if dx * dx + dy * dy > rr * rr {
                    continue;
                }
                let idx = (py as usize / 8) * 128 + px as usize;
                self.0[idx] ^= 1 << (py as u8 % 8);
            }
        }
    }
}
impl DrawTarget for BufCanvas {
    type Color = BinaryColor;
    type Error = core::convert::Infallible;
    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for p in pixels {
            self.set_pixel(p.0.x, p.0.y, p.1 == BinaryColor::On);
        }
        Ok(())
    }
}
impl OriginDimensions for BufCanvas {
    fn size(&self) -> Size {
        Size::new(128, 64)
    }
}

#[derive(Clone, Copy)]
pub enum MenuItem {
    Label {
        label: &'static str,
    },
    Button {
        label: &'static str,
        action: fn(&mut MenuEngine),
    },
    Toggle {
        label: &'static str,
        bind: usize,
    },
    Slider {
        label: &'static str,
        bind: usize,
        min: u8,
        max: u8,
    },
    Submenu {
        label: &'static str,
        page: &'static MenuPage,
    },
}
#[derive(Clone, Copy)]
pub struct MenuPage {
    pub title: &'static str,
    pub items: &'static [MenuItem],
}

const ANIM_FRAMES: u8 = 8;
const EASE_LUT: [u8; (ANIM_FRAMES + 1) as usize] = [0, 84, 147, 193, 223, 242, 251, 254, 255];

pub struct Anim {
    cur: f32,
    target: f32,
    start: f32,
    frame: u8,
    pub done: bool,
}
impl Anim {
    pub fn new(v: f32) -> Self {
        Self {
            cur: v,
            target: v,
            start: v,
            frame: ANIM_FRAMES,
            done: true,
        }
    }
    pub fn snap(&mut self, v: f32) {
        self.cur = v;
        self.target = v;
        self.start = v;
        self.frame = ANIM_FRAMES;
        self.done = true;
    }
    pub fn set(&mut self, t: f32) {
        if (t - self.target).abs() > 0.3 {
            self.start = self.cur;
            self.target = t;
            self.frame = 0;
            self.done = false;
        }
    }
    pub fn update(&mut self) -> f32 {
        if self.done {
            return self.cur;
        }
        self.frame += 1;
        if self.frame >= ANIM_FRAMES {
            self.cur = self.target;
            self.done = true;
            return self.cur;
        }
        let t = EASE_LUT[self.frame as usize] as f32 / 255.0;
        self.cur = self.start + (self.target - self.start) * t;
        self.cur
    }
    pub fn val(&self) -> f32 {
        self.cur
    }
}

fn r3<D: DrawTarget<Color = BinaryColor>>(
    a: &FontRenderer,
    b: &FontRenderer,
    c: &FontRenderer,
    text: &str,
    pos: Point,
    color: FontColor<BinaryColor>,
    target: &mut D,
) {
    let _ = a.render(text, pos, VerticalPosition::Top, color, target);
    let _ = b.render(text, pos, VerticalPosition::Top, color, target);
    let _ = c.render(text, pos, VerticalPosition::Top, color, target);
}

fn label_text(item: &MenuItem) -> &'static str {
    match item {
        MenuItem::Label { label }
        | MenuItem::Button { label, .. }
        | MenuItem::Toggle { label, .. }
        | MenuItem::Slider { label, .. }
        | MenuItem::Submenu { label, .. } => label,
    }
}
fn text_w(text: &str) -> i32 {
    let mut w = 0i32;
    for ch in text.chars() {
        if ch >= '\u{4E00}' && ch <= '\u{9FFF}' {
            w += 12;
        } else if ch >= '\u{3000}' && ch <= '\u{303F}' {
            w += 12;
        } else if ch >= '\u{FF00}' && ch <= '\u{FFEF}' {
            w += 12;
        } else {
            w += 7;
        }
    }
    w
}
fn cursor_rect(sel: usize, page: &MenuPage, scroll_off: f32) -> (f32, f32) {
    let count = page.items.len();
    let sel = if count == 0 { 0 } else { sel.min(count - 1) };
    let y = HEADER_H as f32 + sel as f32 * ITEM_H as f32 - scroll_off;
    let w = (text_w(label_text(&page.items[sel])) + 8).min(124) as f32;
    (y, w)
}

#[derive(Clone, Copy, PartialEq)]
enum KeyEv {
    Up,
    Down,
    Enter,
    Back,
    LongBack,
}
const KEY_BUF: usize = 16;
const MAX_DEPTH: usize = 8;
const ITEM_H: i32 = 16;
const HEADER_H: i32 = 16;

pub struct MenuEngine {
    pages: [&'static MenuPage; MAX_DEPTH],
    depth: usize,
    sel: usize,
    sel_stack: [usize; MAX_DEPTH],
    enter_label: ::core::option::Option<&'static str>,
    trans_cursor_y: Anim,
    trans_cursor_w: Anim,
    trans_to_title: bool,
    pub cursor_y: Anim,
    pub cursor_w: Anim,
    scroll: Anim,
    pub switching: bool,
    pub dirty: bool,
    editing: bool,
    pub osc_timer: u8,
    trans_old_page: ::core::option::Option<&'static MenuPage>,
    trans_old_depth: usize,
    trans_item_frame: u8,
    trans_phase: u8,
    trans_w_target: u32,
    pub bind_bools: [bool; 16],
    pub bind_u8s: [u8; 16],
    tg_anim_bind: i8,
    tg_anim: Anim,
    kb: [KeyEv; KEY_BUF],
    kb_r: usize,
    kb_w: usize,
    pub cursor_radius: u32,
    pub fps: u16,
    marquee_off: f32,
    pub toast_msg: ::core::option::Option<&'static str>,
    pub toast_buf: [u8; 20],
    pub toast_buf_len: u8,
    toast_timer: u8,
    toast_anim: Anim,
    toast_state: u8, // 0=enter, 1=visible, 2=exit
}

impl MenuEngine {
    pub fn new(root: &'static MenuPage) -> Self {
        Self {
            pages: [root; MAX_DEPTH],
            depth: 0,
            sel: 0,
            sel_stack: [0; MAX_DEPTH],
            enter_label: ::core::option::Option::None,
            trans_cursor_y: Anim::new(0.0),
            trans_cursor_w: Anim::new(0.0),
            trans_to_title: false,
            cursor_y: Anim::new(HEADER_H as f32),
            cursor_w: Anim::new(100.0),
            scroll: Anim::new(0.0),
            switching: false,
            dirty: true,
            editing: false,
            osc_timer: 0,
            trans_old_page: ::core::option::Option::None,
            trans_old_depth: 0,
            trans_item_frame: 0,
            trans_phase: 0,
            trans_w_target: 0,
            bind_bools: [false; 16],
            bind_u8s: [0; 16],
            tg_anim_bind: -1,
            tg_anim: Anim::new(0.0),
            kb: [KeyEv::Up; KEY_BUF],
            kb_r: 0,
            kb_w: 0,
            cursor_radius: 4,
            fps: 0,
            marquee_off: 0.0,
            toast_msg: ::core::option::Option::None,
            toast_buf: [0; 20],
            toast_buf_len: 0,
            toast_timer: 0,
            toast_anim: Anim::new(18.0),
            toast_state: 0,
        }
    }
    pub fn page(&self) -> &MenuPage {
        self.pages[self.depth]
    }
    pub fn selection(&self) -> usize {
        self.sel
    }
    fn aim_cursor(&mut self) {
        let (yt, wt) = cursor_rect(self.sel, self.page(), self.scroll.val());
        self.cursor_y.set(yt);
        self.cursor_w.set(wt);
    }

    pub fn push_key(&mut self, up: bool, down: bool, enter: bool, back: bool, long_back: bool) {
        let w = self.kb_w;
        let next = (w + 1) % KEY_BUF;
        if next == self.kb_r {
            return;
        }
        let ev = if long_back {
            KeyEv::LongBack
        } else if up {
            KeyEv::Up
        } else if down {
            KeyEv::Down
        } else if enter {
            KeyEv::Enter
        } else if back {
            KeyEv::Back
        } else {
            return;
        };
        self.kb[w] = ev;
        self.kb_w = next;
    }
    pub fn handle_key(&mut self, up: bool, down: bool, enter: bool, back: bool, long_back: bool) {
        self.push_key(up, down, enter, back, long_back);
    }
    fn flush_keys(&mut self) {
        while self.kb_r != self.kb_w {
            let ev = self.kb[self.kb_r];
            self.kb_r = (self.kb_r + 1) % KEY_BUF;
            self.apply_key(ev);
        }
    }
    fn apply_key(&mut self, ev: KeyEv) {
        let count = self.page().items.len();
        let item = if self.sel < count {
            ::core::option::Option::Some(&self.page().items[self.sel])
        } else {
            ::core::option::Option::None
        };
        if self.editing && !self.switching {
            match (ev, item) {
                (
                    KeyEv::Up | KeyEv::Down | KeyEv::Back,
                    ::core::option::Option::Some(MenuItem::Slider { bind, min, max, .. }),
                ) => {
                    if ev == KeyEv::Up && self.bind_u8s[*bind] < *max {
                        self.bind_u8s[*bind] = self.bind_u8s[*bind].saturating_add(5).min(*max);
                    }
                    if (ev == KeyEv::Down || ev == KeyEv::Back) && self.bind_u8s[*bind] > *min {
                        self.bind_u8s[*bind] = self.bind_u8s[*bind].saturating_sub(5).max(*min);
                    }
                    self.dirty = true;
                }
                (KeyEv::Enter, _) => {
                    self.editing = false;
                    self.dirty = true;
                }
                _ => {}
            }
            return;
        }
        match (ev, item) {
            (KeyEv::Up, _) if count > 0 && !self.switching => {
                if self.sel > 0 {
                    self.sel -= 1;
                    self.aim_cursor();
                    self.dirty = true;
                } else if self.osc_timer == 0 {
                    self.osc_timer = 8;
                    self.dirty = true;
                }
            }
            (KeyEv::Down, _) if count > 0 && !self.switching => {
                if self.sel < count - 1 {
                    self.sel += 1;
                    self.aim_cursor();
                    self.dirty = true;
                } else if self.osc_timer == 0 {
                    self.osc_timer = 8;
                    self.dirty = true;
                }
            }
            (KeyEv::Enter, ::core::option::Option::Some(MenuItem::Slider { .. }))
                if !self.switching =>
            {
                self.editing = true;
                self.dirty = true;
            }
            (KeyEv::Enter, ::core::option::Option::Some(MenuItem::Toggle { bind, .. }))
                if !self.switching =>
            {
                let ns = !self.bind_bools[*bind];
                self.bind_bools[*bind] = ns;
                self.tg_anim = Anim::new(if ns { 0.0 } else { 100.0 });
                self.tg_anim.set(if ns { 100.0 } else { 0.0 });
                self.tg_anim_bind = *bind as i8;
                self.dirty = true;
            }
            (KeyEv::Enter, ::core::option::Option::Some(MenuItem::Button { action, .. }))
                if !self.switching =>
            {
                action(self);
                self.dirty = true;
            }
            (KeyEv::Enter, ::core::option::Option::Some(MenuItem::Submenu { page, .. }))
                if !self.switching =>
            {
                if self.depth + 1 < MAX_DEPTH {
                    self.sel_stack[self.depth] = self.sel;
                    let sub = *page;
                    self.trans_old_page = ::core::option::Option::Some(self.pages[self.depth]);
                    self.trans_old_depth = self.depth;
                    let (cy, cw) = cursor_rect(self.sel, self.page(), self.scroll.val());
                    self.trans_cursor_y = Anim::new(cy);
                    self.trans_cursor_y.set(2.0);
                    self.trans_cursor_w = Anim::new(cw);
                    self.trans_cursor_w.set(60.0);
                    self.enter_label =
                        ::core::option::Option::Some(label_text(&self.page().items[self.sel]));
                    self.switching = true;
                    self.trans_item_frame = 0;
                    self.trans_phase = 1;
                    self.depth += 1;
                    self.pages[self.depth] = sub;
                    self.sel = self.sel_stack[self.depth].min(
                        self.page().items.len().saturating_sub(1),
                    );
                    let (ny, nw) = cursor_rect(self.sel, self.page(), 0.0);
                    self.cursor_y.snap(ny);
                    self.cursor_w.snap(nw);
                    self.scroll.snap(0.0);
                    self.dirty = true;
                }
            }
            (KeyEv::Back, _) if !self.switching && self.depth > 0 => {
                self.sel_stack[self.depth] = self.sel;
                self.trans_old_page = ::core::option::Option::Some(self.pages[self.depth]);
                self.trans_old_depth = self.depth;
                let return_sel = self.sel_stack[self.depth - 1];
                let (ry, rw) = cursor_rect(return_sel, self.pages[self.depth - 1], 0.0);
                self.trans_cursor_y = Anim::new(self.cursor_y.val());
                self.trans_cursor_y.set(2.0);
                self.trans_cursor_w = Anim::new(self.cursor_w.val());
                self.trans_cursor_w.set(60.0);
                self.enter_label =
                    ::core::option::Option::Some(label_text(&self.page().items[self.sel]));
                self.switching = true;
                self.trans_item_frame = 0;
                self.trans_phase = 1;
                self.depth -= 1;
                self.sel = return_sel;
                self.cursor_y = Anim::new(ry);
                self.cursor_w = Anim::new(rw);
                self.scroll.snap(0.0);
                self.dirty = true;
            }
            (KeyEv::LongBack, _) => {
                self.depth = 0;
                self.sel = self.sel_stack[0].min(
                    self.page().items.len().saturating_sub(1),
                );
                self.cursor_y = Anim::new(HEADER_H as f32 + self.sel as f32 * ITEM_H as f32);
                self.cursor_w = Anim::new(100.0);
                self.scroll.snap(0.0);
                self.switching = false;
                self.enter_label = ::core::option::Option::None;
                self.dirty = true;
            }
            _ => {}
        }
    }
    /// 从字节切片显示 Toast (支持运行时消息, 最多 TOAST_BUF_LEN 字节)
    pub fn show_toast_bytes(&mut self, msg: &[u8], frames: u8) {
        let n = msg.len().min(self.toast_buf.len());
        self.toast_buf[..n].copy_from_slice(&msg[..n]);
        self.toast_buf_len = n as u8;
        self.toast_timer = frames;
        self.toast_anim = Anim::new(-22.0);
        self.toast_anim.set(18.0);
        self.toast_state = 0;
        self.dirty = true;
    }
    pub fn set_page(&mut self, p: &'static MenuPage) {
        self.pages[self.depth] = p;
        self.sel = 0;
        self.sel_stack[self.depth] = 0;
        self.cursor_y = Anim::new(HEADER_H as f32);
        self.cursor_w = Anim::new(100.0);
        self.scroll.snap(0.0);
        self.dirty = true;
    }
    pub fn show_toast(&mut self, msg: &'static str, frames: u8) {
        self.toast_msg = ::core::option::Option::Some(msg);
        self.toast_timer = frames;
        self.toast_anim = Anim::new(-22.0);
        self.toast_anim.set(18.0);
        self.toast_state = 0;
        self.dirty = true;
    }

    fn tick(&mut self) {
        self.flush_keys();
        if self.switching {
            self.trans_cursor_y.update();
            self.trans_cursor_w.update();
            self.trans_item_frame = self.trans_item_frame.saturating_add(1);
            if self.trans_phase == 1 && self.trans_cursor_y.done {
                self.trans_phase = 2;
                self.trans_item_frame = 0;
                let (ty, tw) = cursor_rect(self.sel, self.page(), self.scroll.val());
                self.trans_cursor_y = Anim::new(2.0);
                self.trans_cursor_y.set(ty);
                self.trans_w_target = tw as u32;
                self.enter_label =
                    ::core::option::Option::Some(label_text(&self.page().items[self.sel]));
                self.dirty = true;
            } else if self.trans_phase == 2 && self.trans_cursor_y.done {
                self.switching = false;
                self.trans_old_page = ::core::option::Option::None;
                self.trans_phase = 0;
            } else {
                self.dirty = true;
            }
        } else {
            self.cursor_y.update();
            self.cursor_w.update();
            let mut st = self.scroll.val();
            let target_cy = 18.0 + self.sel as f32 * ITEM_H as f32 - st;
            if target_cy < HEADER_H as f32 + 2.0 {
                st -= (HEADER_H as f32 + 2.0 - target_cy).max(0.0);
            }
            if target_cy > 50.0 {
                st += (target_cy - 50.0).max(0.0);
            }
            st = st.clamp(
                0.0,
                (self.page().items.len() as f32 * ITEM_H as f32 - 48.0).max(0.0),
            );
            self.scroll.set(st);
            self.scroll.update();
            self.aim_cursor();
            // 跑马灯: 选定项文字超宽时向左滚动
            let count = self.page().items.len();
            if count > 0 && self.sel < count {
                let tw = text_w(label_text(&self.page().items[self.sel]));
                if tw > 116 {
                    self.marquee_off += 1.0;
                } else {
                    self.marquee_off = 0.0;
                }
            }
        }
        if self.tg_anim_bind >= 0 {
            self.tg_anim.update();
            self.dirty = true;
            if self.tg_anim.done {
                self.tg_anim_bind = -1;
            }
        }
        if self.osc_timer > 0 {
            self.osc_timer -= 1;
            self.dirty = true;
        }
        if self.toast_timer > 0 {
            self.toast_timer -= 1;
            self.toast_anim.update();
            if self.toast_anim.done && self.toast_state == 0 {
                self.toast_state = 1;
            }
            self.dirty = true;
            if self.toast_timer == 0 && self.toast_state == 1 {
                self.toast_state = 2;
                self.toast_anim = Anim::new(18.0);
                self.toast_anim.set(-22.0);
            }
        }
        if self.toast_state == 2 {
            self.toast_anim.update();
            self.dirty = true;
            if self.toast_anim.done {
                self.toast_msg = ::core::option::Option::None;
                self.toast_state = 0;
            }
        }
    }

    pub fn render_buf(&mut self, buf: &mut BufCanvas) {
        self.dirty = false;
        self.tick();
        let cn = FontRenderer::new::<u8g2_fonts::fonts::u8g2_font_wqy12_t_gb2312>()
            .with_ignore_unknown_chars(true);
        let so = self.scroll.val() as i32;
        // 1) 菜单项 + widget
        let trans_off = if self.switching {
            let f = self.trans_item_frame.min(ANIM_FRAMES);
            EASE_LUT[f as usize] as i32 * 48 / 255
        } else {
            0
        };
        let render_old = self.switching && self.trans_phase == 1;
        let render_items = if render_old && self.trans_old_page.is_some() {
            self.trans_old_page.unwrap().items
        } else {
            self.page().items
        };
        for i in 0..render_items.len() {
            let mut y = HEADER_H + i as i32 * ITEM_H - so;
            if self.switching {
                if render_old {
                    y += trans_off;
                } else {
                    y += 48 - trans_off;
                }
            }
            if y + ITEM_H <= 0 || y >= 64 {
                continue;
            }
            let item = &render_items[i];
            let txt = label_text(item);
            let tw = text_w(txt);
            // 跑马灯: 选中项超宽文字向左循环滚动 (双份渲染实现无缝衔接)
            if i == self.sel && tw > 116 {
                let cycle = tw - 116 + 8;
                let moff = (self.marquee_off as i32) % cycle;
                cn.render(txt, Point::new(4 - moff, y + 2),
                    VerticalPosition::Top, FontColor::Transparent(BinaryColor::On), buf);
                cn.render(txt, Point::new(4 - moff + cycle, y + 2),
                    VerticalPosition::Top, FontColor::Transparent(BinaryColor::On), buf);
            } else {
                cn.render(txt, Point::new(4, y + 2),
                    VerticalPosition::Top, FontColor::Transparent(BinaryColor::On), buf);
            }
            match item {
                MenuItem::Toggle { bind, .. } => {
                    let sw = 32u32;
                    let sh = 10u32;
                    let kp = if self.tg_anim_bind == *bind as i8 {
                        self.tg_anim.val() / 100.0
                    } else if self.bind_bools[*bind] {
                        1.0
                    } else {
                        0.0
                    };
                    let fw = (sw - sh) as f32 * kp;
                    let _ = RoundedRectangle::new(
                        Rectangle::new(Point::new(92, y + 3), Size::new(sw, sh)),
                        CornerRadii::new(Size::new(5, 5)),
                    )
                    .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
                    .draw(buf);
                    if fw > 0.0 {
                        let _ = RoundedRectangle::new(
                            Rectangle::new(Point::new(93, y + 4), Size::new(fw as u32, sh - 2)),
                            CornerRadii::new(Size::new(4, 4)),
                        )
                        .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
                        .draw(buf);
                    }
                    let _ = Circle::new(
                        Point::new(92 + ((sw as i32 - sh as i32) as f32 * kp) as i32, y + 3),
                        sh as u32,
                    )
                    .into_styled(PrimitiveStyle::with_fill(if kp > 0.5 {
                        BinaryColor::Off
                    } else {
                        BinaryColor::On
                    }))
                    .draw(buf);
                }
                MenuItem::Slider { bind, .. } => {
                    let bw = 44u32;
                    let bh = 8u32;
                    let _ = Rectangle::new(Point::new(80, y + 4), Size::new(bw, bh))
                        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
                        .draw(buf);
                    let fw = self.bind_u8s[*bind] as u32 * bw / 100;
                    if fw > 0 {
                        let _ = Rectangle::new(Point::new(81, y + 5), Size::new(fw, bh - 2))
                            .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
                            .draw(buf);
                    }
                    let v = self.bind_u8s[*bind];
                    let r6s = FontRenderer::new::<u8g2_fonts::fonts::u8g2_font_6x10_tf>()
                        .with_ignore_unknown_chars(true);
                    let tens = b'0' + v / 10;
                    let ones = b'0' + v % 10;
                    let ds = [b'0' + v / 10, b'0' + v % 10];
                    let s = if v >= 10 {
                        core::str::from_utf8(&ds[..2]).unwrap_or("")
                    } else {
                        core::str::from_utf8(&ds[1..2]).unwrap_or("")
                    };
                    let _ = r6s.render(
                        s,
                        Point::new(72, y + 1),
                        VerticalPosition::Top,
                        FontColor::Transparent(BinaryColor::On),
                        buf,
                    );
                }
                _ => {}
            }
        }
        // 2) overlay 标题
        let title_page = if self.switching && render_old && self.trans_old_page.is_some() {
            self.trans_old_page.unwrap()
        } else {
            self.page()
        };
        let has_parent = if self.switching && render_old && self.trans_old_page.is_some() {
            self.trans_old_depth > 0
        } else {
            self.depth > 0
        };
        let _ = Rectangle::new(Point::new(0, 0), Size::new(128, HEADER_H as u32))
            .into_styled(PrimitiveStyle::with_fill(BinaryColor::Off))
            .draw(buf);
        if has_parent {
            cn.render(
                "< ",
                Point::new(2, 1),
                VerticalPosition::Top,
                FontColor::Transparent(BinaryColor::On),
                buf,
            );
            cn.render(
                title_page.title,
                Point::new(18, 1),
                VerticalPosition::Top,
                FontColor::Transparent(BinaryColor::On),
                buf,
            );
        } else {
            cn.render(
                title_page.title,
                Point::new(4, 1),
                VerticalPosition::Top,
                FontColor::Transparent(BinaryColor::On),
                buf,
            );
        }
        let _ = Rectangle::new(Point::new(0, HEADER_H - 2), Size::new(128, 1))
            .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
            .draw(buf);
        // 3) 光标
        if self.switching {
            let tcy = self.trans_cursor_y.val() as i32;
            let tcw = if self.trans_phase == 2 {
                self.trans_w_target
            } else {
                self.trans_cursor_w.val() as u32
            };
            if tcw > 4 && tcy >= 0 && tcy < 64 {
                let _ = RoundedRectangle::new(
                    Rectangle::new(Point::new(2, tcy), Size::new(tcw, ITEM_H as u32 - 2)),
                    CornerRadii::new(Size::new(self.cursor_radius, self.cursor_radius)),
                )
                .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
                .draw(buf);
                if let ::core::option::Option::Some(lbl) = self.enter_label {
                    let _ = cn.render(
                        lbl,
                        Point::new(4, tcy + 2),
                        VerticalPosition::Top,
                        FontColor::Transparent(BinaryColor::Off),
                        buf,
                    );
                }
            }
        } else {
            let cy = self.cursor_y.val() as i32;
            let cw = self.cursor_w.val() as u32;
            let ox = match self.osc_timer {
                8 => 5,
                7 => -3,
                6 => 2,
                5 => -1,
                4 => 1,
                _ => 0,
            };
            buf.invert_rect(ox, cy, cw, ITEM_H as u32 - 2, self.cursor_radius);
        }
        // 4) FPS
        if self.bind_bools[0] {
            let r6 = FontRenderer::new::<u8g2_fonts::fonts::u8g2_font_6x10_tf>()
                .with_ignore_unknown_chars(true);
            let tens = (self.fps / 10) as u8;
            let ones = (self.fps % 10) as u8;
            let s = [tens + b'0', ones + b'0', b'f', b'p', b's'];
            let _ = r6.render(
                core::str::from_utf8(&s).unwrap_or("xxfps"),
                Point::new(92, 3),
                VerticalPosition::Top,
                FontColor::Transparent(BinaryColor::On),
                buf,
            );
        }
        // 5) Toast 横幅
        let toast_text = if let ::core::option::Option::Some(msg) = self.toast_msg {
            Some(msg)
        } else if self.toast_buf_len > 0 {
            Some(unsafe {
                core::str::from_utf8_unchecked(&self.toast_buf[..self.toast_buf_len as usize])
            })
        } else {
            None
        };
        if let ::core::option::Option::Some(msg) = toast_text {
            let ty = self.toast_anim.val() as i32;
            let _ = RoundedRectangle::new(
                Rectangle::new(Point::new(2, ty), Size::new(124, 22)),
                CornerRadii::new(Size::new(4, 4)),
            )
            .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
            .draw(buf);
            let _ = Rectangle::new(Point::new(4, ty + 2), Size::new(120, 18))
                .into_styled(PrimitiveStyle::with_fill(BinaryColor::Off))
                .draw(buf);
            let r6 = FontRenderer::new::<u8g2_fonts::fonts::u8g2_font_6x10_tf>()
                .with_ignore_unknown_chars(true);
            let _ = r6.render(
                msg,
                Point::new(6, ty + 6),
                VerticalPosition::Top,
                FontColor::Transparent(BinaryColor::On),
                buf,
            );
        }
    }
}
