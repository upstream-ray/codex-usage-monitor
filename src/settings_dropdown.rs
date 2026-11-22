//! Fluent-styled drop-down flyout for settings choices.
//!
//! Uses a lightweight native popup window with Windows 11 rounded corners and
//! DWM drop shadow, custom DIB rendering via `raster`, color swatches, keyboard
//! navigation, wheel scrolling, and a short fade/slide entrance.
use super::*;
use std::cell::RefCell;
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VIRTUAL_KEY, VK_DOWN, VK_END, VK_ESCAPE, VK_HOME,
    VK_RETURN, VK_SPACE, VK_UP,
};

const POPUP_CLASS: PCWSTR = w!("CodexUsageSettingsDropdown");
const ANIM_TIMER: usize = 1;
const ANIM_DURATION: Duration = Duration::from_millis(120);
const ITEM_HEIGHT: i32 = 34;
const ITEM_GAP: i32 = 2;
const PAD_X: i32 = 5;
const PAD_Y: i32 = 5;
const MAX_HEIGHT: i32 = 340;

thread_local! {
    static ACTIVE: RefCell<Option<PopupState>> = const { RefCell::new(None) };
}

struct PopupState {
    hwnd: HWND,
    items: Vec<OptionItem>,
    selected: usize,
    highlighted: Option<usize>,
    pressed: Option<usize>,
    theme: Theme,
    fonts: Fonts,
    dpi: u32,
    scroll: i32,
    content_height: i32,
    opens_upward: bool,
    start: Instant,
    animating: bool,
    buffer: Option<Bitmap>,
    result: Option<usize>,
    done: bool,
}

impl PopupState {
    fn px(&self, value: i32) -> i32 {
        scale(value, self.dpi)
    }

    fn pxf(&self, value: f32) -> f32 {
        value * self.dpi as f32 / 96.0
    }

    fn item_step(&self) -> i32 {
        self.px(ITEM_HEIGHT + ITEM_GAP)
    }

    fn item_rect(&self, index: usize, width: i32) -> RECT {
        let pad_x = self.px(PAD_X);
        let pad_y = self.px(PAD_Y);
        let top = pad_y + index as i32 * self.item_step() - self.scroll;
        RECT {
            left: pad_x,
            top,
            right: (width - pad_x).max(pad_x),
            bottom: top + self.px(ITEM_HEIGHT),
        }
    }

    fn hit_item(&self, y: i32) -> Option<usize> {
        let pad_y = self.px(PAD_Y);
        let rel = y + self.scroll - pad_y;
        if rel < 0 {
            return None;
        }
        let step = self.item_step();
        let index = (rel / step) as usize;
        let offset_in_step = rel % step;
        if index < self.items.len() && offset_in_step < self.px(ITEM_HEIGHT) {
            Some(index)
        } else {
            None
        }
    }

    unsafe fn client(&self) -> RECT {
        let mut rect = RECT::default();
        let _ = GetClientRect(self.hwnd, &mut rect);
        rect
    }

    fn max_scroll(&self, client_h: i32) -> i32 {
        (self.content_height - client_h).max(0)
    }

    unsafe fn ensure_visible(&mut self, index: usize) {
        let client_h = self.client().bottom;
        let pad_y = self.px(PAD_Y);
        let top = pad_y + index as i32 * self.item_step();
        let bottom = top + self.px(ITEM_HEIGHT);
        let max_s = self.max_scroll(client_h);
        if top - pad_y < self.scroll {
            self.scroll = (top - pad_y).clamp(0, max_s);
        } else if bottom + pad_y > self.scroll + client_h {
            self.scroll = (bottom + pad_y - client_h).clamp(0, max_s);
        }
    }

    unsafe fn scroll_by(&mut self, delta: i32) {
        let client_h = self.client().bottom;
        let next = (self.scroll + delta).clamp(0, self.max_scroll(client_h));
        if next != self.scroll {
            self.scroll = next;
            let _ = InvalidateRect(self.hwnd, None, false);
        }
    }

    unsafe fn close(&mut self, picked: Option<usize>) {
        if self.done {
            return;
        }
        self.result = picked;
        self.done = true;
        let _ = KillTimer(self.hwnd, ANIM_TIMER);
        let _ = ShowWindow(self.hwnd, SW_HIDE);
    }

    unsafe fn paint(&mut self, target: HDC) {
        let client = self.client();
        if !has_area(client) {
            return;
        }
        let mut buffer = match self.buffer.take() {
            Some(bmp) if bmp.bounds() == client => bmp,
            _ => match Bitmap::new(client.right, client.bottom) {
                Some(bmp) => bmp,
                None => return,
            },
        };

        let elapsed = self.start.elapsed();
        let t = if self.animating {
            ease_out(elapsed, ANIM_DURATION)
        } else {
            1.0
        };
        let slide_px = ((1.0 - t) * self.px(8) as f32).round() as i32;
        let y_shift = if self.opens_upward {
            slide_px
        } else {
            -slide_px
        };

        let surface = if self.theme.dark {
            0x2C_2C2C
        } else {
            0xFC_FCFC
        };
        let border = if self.theme.dark {
            0x42_4242
        } else {
            0xD6_D6D6
        };
        let hover_bg = if self.theme.dark {
            0x38_3838
        } else {
            0xF1_F1F1
        };
        let pressed_bg = if self.theme.dark {
            0x32_3232
        } else {
            0xE9_E9E9
        };
        let selected_bg = if self.theme.dark {
            0x35_3535
        } else {
            0xEF_EFEF
        };

        let mut drawing = Drawing::for_bitmap(&mut buffer);
        fill_rect(drawing.dc, client, surface);

        let radius = self.pxf(CONTROL_RADIUS);
        let bar_w = self.pxf(3.0);
        let bar_h = self.pxf(16.0);
        let swatch_size = self.px(14);
        let pad_text = self.px(12);
        let scrollable = self.content_height > client.bottom;
        let item_right_trim = if scrollable { self.px(8) } else { 0 };
        let item_w = client.right - item_right_trim;

        // First pass: anti-aliased item pills, selection indicators, color swatches, mini scrollbar.
        if let Some(mut canvas) = drawing.canvas(client) {
            for (index, item) in self.items.iter().enumerate() {
                let rect = translated(self.item_rect(index, item_w), 0, y_shift);
                if !has_area(intersection(rect, client)) {
                    continue;
                }
                let is_selected = index == self.selected;
                let is_highlighted = self.highlighted == Some(index);
                let is_pressed = self.pressed == Some(index);
                let enabled = item.action.is_some();

                if enabled && (is_pressed || is_highlighted || is_selected) {
                    let bg = if is_pressed {
                        pressed_bg
                    } else if is_highlighted {
                        hover_bg
                    } else {
                        selected_bg
                    };
                    canvas.fill(rect, radius, opaque(bg));
                }

                if is_selected {
                    let cy = (rect.top + rect.bottom) as f32 * 0.5;
                    canvas.pill(
                        rect.left as f32,
                        cy - bar_h * 0.5,
                        rect.left as f32 + bar_w,
                        cy + bar_h * 0.5,
                        bar_w * 0.5,
                        opaque(self.theme.accent),
                    );
                }

                if let Some(color) = item.swatch {
                    let diameter = swatch_size as f32;
                    let cx = (rect.left + pad_text) as f32 + diameter * 0.5;
                    let cy = (rect.top + rect.bottom) as f32 * 0.5;
                    canvas.circle(cx, cy, diameter, opaque(self.theme.control_border));
                    canvas.circle(cx, cy, diameter - self.pxf(2.0), opaque(color));
                }
            }

            if scrollable {
                let track_top = self.px(6) as f32;
                let track_bottom = (client.bottom - self.px(6)) as f32;
                let track_h = (track_bottom - track_top).max(1.0);
                let thumb_h = (track_h * (client.bottom as f32 / self.content_height as f32))
                    .max(self.px(24) as f32)
                    .min(track_h);
                let max_s = self.max_scroll(client.bottom).max(1) as f32;
                let thumb_top = track_top + (track_h - thumb_h) * (self.scroll as f32 / max_s);
                let bar_right = (client.right - self.px(3)) as f32;
                let bar_left = bar_right - self.pxf(3.0);
                let thumb_color = if self.theme.dark {
                    0x9F_9F9F
                } else {
                    0x7A_7A7A
                };
                canvas.pill(
                    bar_left,
                    thumb_top,
                    bar_right,
                    thumb_top + thumb_h,
                    self.pxf(1.5),
                    0xB800_0000 | thumb_color,
                );
            }

            // Crisp 1px inner border fallback for Windows 10 (on Win11 DWM also renders a rounded border).
            if windows_build() < 22000 {
                canvas.stroke(client, 0.0, opaque(border), 1.0);
            }
        }

        // Second pass: ClearType labels.
        let dc = drawing.dc;
        for (index, item) in self.items.iter().enumerate() {
            let rect = translated(self.item_rect(index, item_w), 0, y_shift);
            if !has_area(intersection(rect, client)) {
                continue;
            }
            let enabled = item.action.is_some();
            let is_selected = index == self.selected;
            let is_pressed = self.pressed == Some(index);
            let mut text_rect = inset(rect, pad_text, 0);
            if item.swatch.is_some() {
                text_rect.left += swatch_size + self.px(8);
            }
            let color = if !enabled {
                self.theme.disabled
            } else if is_pressed {
                self.theme.secondary
            } else {
                self.theme.text
            };
            draw_text(
                dc,
                if is_selected {
                    self.fonts.strong
                } else {
                    self.fonts.body
                },
                color,
                &item.label,
                text_rect,
                DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
            );
        }

        let _ = buffer.blit(target, client, 0, 0);
        self.buffer = Some(buffer);
    }
}

pub(super) fn compute_popup_rect(
    anchor: RECT,
    work: RECT,
    desired_width: i32,
    content_height: i32,
    max_height: i32,
    gap: i32,
) -> (RECT, bool) {
    let work_w = (work.right - work.left).max(120);
    let work_h = (work.bottom - work.top).max(80);
    let width = desired_width.min(work_w - 8).max(80);
    let below_avail = (work.bottom - (anchor.bottom + gap)).max(0);
    let above_avail = (anchor.top - gap - work.top).max(0);
    let want_h = content_height.min(max_height).min(work_h - 8).max(36);

    let (top, height, opens_upward) = if below_avail >= want_h || below_avail >= above_avail {
        let h = want_h.min(below_avail.max(36));
        let t = (anchor.bottom + gap).min(work.bottom - h);
        (t.max(work.top), h, false)
    } else {
        let h = want_h.min(above_avail.max(36));
        let t = (anchor.top - gap - h).max(work.top);
        (t, h, true)
    };

    let left = anchor
        .left
        .clamp(work.left + 4, (work.right - width - 4).max(work.left + 4));
    (
        RECT {
            left,
            top,
            right: left + width,
            bottom: top + height,
        },
        opens_upward,
    )
}

pub(super) fn next_enabled_index(
    items: &[OptionItem],
    current: Option<usize>,
    step: isize,
) -> Option<usize> {
    if items.is_empty() || step == 0 {
        return None;
    }
    let len = items.len() as isize;
    let mut index = match current {
        Some(i) => i as isize,
        None if step > 0 => -1,
        None => len,
    };
    for _ in 0..items.len() {
        index = (index + step).rem_euclid(len);
        if items[index as usize].action.is_some() {
            return Some(index as usize);
        }
    }
    None
}

pub(super) unsafe fn pick(
    button: HWND,
    items: &[OptionItem],
    selected: usize,
    anchor: RECT,
    theme: Theme,
    fonts: Fonts,
    dpi: u32,
) -> Option<usize> {
    if items.is_empty() {
        return None;
    }

    let owner = GetAncestor(button, GA_ROOT);
    let module = GetModuleHandleW(None).ok()?;
    let instance = HINSTANCE(module.0);
    let cursor = LoadCursorW(None, IDC_ARROW).unwrap_or_default();
    let _ = RegisterClassW(&WNDCLASSW {
        lpfnWndProc: Some(dropdown_proc),
        hInstance: instance,
        lpszClassName: POPUP_CLASS,
        hCursor: cursor,
        ..Default::default()
    });

    // Measure widest label so long localized options are never truncated unnecessarily.
    let dc = GetDC(owner);
    let mut max_label_w = 0;
    let swatch_extra = scale(22, dpi);
    for item in items {
        let mut w = text_width(dc, fonts.strong, &item.label);
        if item.swatch.is_some() {
            w += swatch_extra;
        }
        max_label_w = max_label_w.max(w);
    }
    ReleaseDC(owner, dc);

    let anchor_w = anchor.right - anchor.left;
    let item_step = scale(ITEM_HEIGHT + ITEM_GAP, dpi);
    let content_height =
        scale(PAD_Y * 2, dpi) + items.len() as i32 * item_step - scale(ITEM_GAP, dpi);
    let max_h = scale(MAX_HEIGHT, dpi);
    let scroll_extra = if content_height > max_h {
        scale(10, dpi)
    } else {
        0
    };
    let desired_w = anchor_w.max(max_label_w + scale(PAD_X * 2 + 28, dpi) + scroll_extra);

    let monitor = MonitorFromRect(&anchor, MONITOR_DEFAULTTONEAREST);
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    let work = if GetMonitorInfoW(monitor, &mut info).as_bool() {
        info.rcWork
    } else {
        RECT {
            left: 0,
            top: 0,
            right: anchor.right + 400,
            bottom: anchor.bottom + 400,
        }
    };
    let (popup_rect, opens_upward) = compute_popup_rect(
        anchor,
        work,
        desired_w,
        content_height,
        max_h,
        scale(4, dpi),
    );

    let hwnd = CreateWindowExW(
        WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_LAYERED,
        POPUP_CLASS,
        PCWSTR::null(),
        WS_POPUP | WS_CLIPCHILDREN,
        popup_rect.left,
        popup_rect.top,
        popup_rect.right - popup_rect.left,
        popup_rect.bottom - popup_rect.top,
        owner,
        HMENU::default(),
        instance,
        None,
    )
    .ok()?;

    // Windows 11 native rounded corners and themed thin border with drop shadow.
    let corner = DWMWCP_ROUND;
    let _ = DwmSetWindowAttribute(
        hwnd,
        DWMWA_WINDOW_CORNER_PREFERENCE,
        &corner as *const _ as *const _,
        std::mem::size_of_val(&corner) as u32,
    );
    let border_color = colorref(if theme.dark { 0x45_4545 } else { 0xD6_D6D6 });
    let _ = DwmSetWindowAttribute(
        hwnd,
        DWMWA_BORDER_COLOR,
        &border_color as *const COLORREF as *const _,
        std::mem::size_of::<COLORREF>() as u32,
    );

    let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 1, LWA_ALPHA);

    let mut state = PopupState {
        hwnd,
        items: items.to_vec(),
        selected,
        highlighted: (selected < items.len()).then_some(selected),
        pressed: None,
        theme,
        fonts,
        dpi,
        scroll: 0,
        content_height,
        opens_upward,
        start: Instant::now(),
        animating: true,
        buffer: None,
        result: None,
        done: false,
    };
    if selected < items.len() {
        state.ensure_visible(selected);
    }
    ACTIVE.with(|slot| *slot.borrow_mut() = Some(state));

    SetTimer(hwnd, ANIM_TIMER, 15, None);
    let _ = ShowWindow(hwnd, SW_SHOW);
    let _ = SetForegroundWindow(hwnd);
    let _ = UpdateWindow(hwnd);

    let mut msg = MSG::default();
    loop {
        let finished = ACTIVE.with(|slot| slot.borrow().as_ref().is_none_or(|s| s.done));
        if finished {
            break;
        }
        let status = GetMessageW(&mut msg, HWND::default(), 0, 0).0;
        if status <= 0 {
            if status == 0 {
                PostQuitMessage(msg.wParam.0 as i32);
            }
            break;
        }
        // The click on the dropdown button dismisses the popup and must not reopen it.
        if msg.hwnd == button && matches!(msg.message, WM_LBUTTONDOWN | WM_LBUTTONDBLCLK) {
            with_popup(|state| state.close(None));
            break;
        }
        let _ = TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }

    let _ = DestroyWindow(hwnd);
    ACTIVE.with(|slot| slot.borrow_mut().take().and_then(|state| state.result))
}

fn with_popup<T>(f: impl FnOnce(&mut PopupState) -> T) -> Option<T> {
    ACTIVE.with(|slot| slot.try_borrow_mut().ok()?.as_mut().map(f))
}

unsafe extern "system" fn dropdown_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_TIMER if wparam.0 == ANIM_TIMER => {
            with_popup(|s| {
                let elapsed = s.start.elapsed();
                if elapsed >= ANIM_DURATION {
                    s.animating = false;
                    let _ = KillTimer(hwnd, ANIM_TIMER);
                    let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
                } else {
                    let t = ease_out(elapsed, ANIM_DURATION);
                    let alpha = (t * 255.0).round().clamp(1.0, 255.0) as u8;
                    let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), alpha, LWA_ALPHA);
                }
                let _ = InvalidateRect(hwnd, None, false);
            });
            return LRESULT(0);
        }
        WM_MOUSEMOVE => {
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            let track = with_popup(|s| {
                let hit = s.hit_item(y).filter(|&i| s.items[i].action.is_some());
                if s.highlighted != hit {
                    s.highlighted = hit;
                    let _ = InvalidateRect(hwnd, None, false);
                }
                true
            })
            .unwrap_or(false);
            if track {
                let mut tme = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                let _ = TrackMouseEvent(&mut tme);
            }
            return LRESULT(0);
        }
        WM_MOUSELEAVE => {
            with_popup(|s| {
                if s.highlighted.is_some() || s.pressed.is_some() {
                    s.highlighted = None;
                    s.pressed = None;
                    let _ = InvalidateRect(hwnd, None, false);
                }
            });
            return LRESULT(0);
        }
        WM_LBUTTONDOWN => {
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            with_popup(|s| {
                let hit = s.hit_item(y).filter(|&i| s.items[i].action.is_some());
                s.pressed = hit;
                s.highlighted = hit;
                let _ = InvalidateRect(hwnd, None, false);
            });
            return LRESULT(0);
        }
        WM_LBUTTONUP => {
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            with_popup(|s| {
                let hit = s.hit_item(y).filter(|&i| s.items[i].action.is_some());
                let pressed = s.pressed.take();
                if let Some(index) = hit.filter(|&i| pressed == Some(i)) {
                    s.close(Some(index));
                } else {
                    let _ = InvalidateRect(hwnd, None, false);
                }
            });
            return LRESULT(0);
        }
        WM_MOUSEWHEEL => {
            let delta = i32::from((wparam.0 >> 16) as u16 as i16);
            with_popup(|s| {
                let step = s.px(48);
                s.scroll_by(-delta * step / 120);
            });
            return LRESULT(0);
        }
        WM_KEYDOWN => {
            let vk = VIRTUAL_KEY(wparam.0 as u16);
            with_popup(|s| match vk {
                VK_ESCAPE => s.close(None),
                VK_RETURN | VK_SPACE => {
                    if let Some(index) = s
                        .highlighted
                        .filter(|&i| s.items.get(i).is_some_and(|item| item.action.is_some()))
                    {
                        s.close(Some(index));
                    } else {
                        s.close(None);
                    }
                }
                VK_DOWN => {
                    if let Some(next) = next_enabled_index(&s.items, s.highlighted, 1) {
                        s.highlighted = Some(next);
                        s.ensure_visible(next);
                        let _ = InvalidateRect(hwnd, None, false);
                    }
                }
                VK_UP => {
                    if let Some(next) = next_enabled_index(&s.items, s.highlighted, -1) {
                        s.highlighted = Some(next);
                        s.ensure_visible(next);
                        let _ = InvalidateRect(hwnd, None, false);
                    }
                }
                VK_HOME => {
                    if let Some(first) = s.items.iter().position(|item| item.action.is_some()) {
                        s.highlighted = Some(first);
                        s.ensure_visible(first);
                        let _ = InvalidateRect(hwnd, None, false);
                    }
                }
                VK_END => {
                    if let Some(last) = s.items.iter().rposition(|item| item.action.is_some()) {
                        s.highlighted = Some(last);
                        s.ensure_visible(last);
                        let _ = InvalidateRect(hwnd, None, false);
                    }
                }
                _ => {}
            });
            return LRESULT(0);
        }
        WM_ACTIVATE if (wparam.0 & 0xFFFF) == 0 => {
            with_popup(|s| s.close(None));
            return LRESULT(0);
        }
        WM_ERASEBKGND => return LRESULT(1),
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let dc = BeginPaint(hwnd, &mut ps);
            with_popup(|s| s.paint(dc));
            let _ = EndPaint(hwnd, &ps);
            return LRESULT(0);
        }
        _ => {}
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

/// Render a static snapshot of the dropdown flyout for visual testing.
#[cfg(test)]
pub(super) unsafe fn render_preview_bitmap(
    items: &[OptionItem],
    selected: usize,
    highlighted: Option<usize>,
    width: i32,
    theme: Theme,
    fonts: Fonts,
    dpi: u32,
) -> Option<Bitmap> {
    let item_step = scale(ITEM_HEIGHT + ITEM_GAP, dpi);
    let content_height =
        scale(PAD_Y * 2, dpi) + items.len() as i32 * item_step - scale(ITEM_GAP, dpi);
    let height = content_height.min(scale(MAX_HEIGHT, dpi));
    let mut bmp = Bitmap::new(width, height)?;
    let bounds = bmp.bounds();
    let surface = if theme.dark { 0x2C_2C2C } else { 0xFC_FCFC };
    let border = if theme.dark { 0x45_4545 } else { 0xD6_D6D6 };
    let hover_bg = if theme.dark { 0x38_3838 } else { 0xF1_F1F1 };
    let selected_bg = if theme.dark { 0x35_3535 } else { 0xEF_EFEF };
    let pad_x = scale(PAD_X, dpi);
    let pad_y = scale(PAD_Y, dpi);
    let item_h = scale(ITEM_HEIGHT, dpi);
    let pad_text = scale(12, dpi);
    let swatch_size = scale(14, dpi);
    let radius = CONTROL_RADIUS * dpi as f32 / 96.0;
    let bar_w = 3.0 * dpi as f32 / 96.0;
    let bar_h = 16.0 * dpi as f32 / 96.0;

    let mut drawing = Drawing::for_bitmap(&mut bmp);
    fill_rect(drawing.dc, bounds, surface);
    if let Some(mut canvas) = drawing.canvas(bounds) {
        canvas.card(bounds, 8.0 * dpi as f32 / 96.0, surface, border);
        for (index, item) in items.iter().enumerate() {
            let top = pad_y + index as i32 * item_step;
            let rect = RECT {
                left: pad_x,
                top,
                right: width - pad_x,
                bottom: top + item_h,
            };
            if !has_area(intersection(rect, bounds)) {
                continue;
            }
            if highlighted == Some(index) {
                canvas.fill(rect, radius, opaque(hover_bg));
            } else if index == selected {
                canvas.fill(rect, radius, opaque(selected_bg));
            }
            if index == selected {
                let cy = (rect.top + rect.bottom) as f32 * 0.5;
                canvas.pill(
                    rect.left as f32,
                    cy - bar_h * 0.5,
                    rect.left as f32 + bar_w,
                    cy + bar_h * 0.5,
                    bar_w * 0.5,
                    opaque(theme.accent),
                );
            }
            if let Some(color) = item.swatch {
                let d = swatch_size as f32;
                let cx = (rect.left + pad_text) as f32 + d * 0.5;
                let cy = (rect.top + rect.bottom) as f32 * 0.5;
                canvas.circle(cx, cy, d, opaque(theme.control_border));
                canvas.circle(cx, cy, d - 2.0 * dpi as f32 / 96.0, opaque(color));
            }
        }
    }
    let dc = drawing.dc;
    for (index, item) in items.iter().enumerate() {
        let top = pad_y + index as i32 * item_step;
        let rect = RECT {
            left: pad_x,
            top,
            right: width - pad_x,
            bottom: top + item_h,
        };
        if !has_area(intersection(rect, bounds)) {
            continue;
        }
        let mut text_rect = inset(rect, pad_text, 0);
        if item.swatch.is_some() {
            text_rect.left += swatch_size + scale(8, dpi);
        }
        draw_text(
            dc,
            if index == selected {
                fonts.strong
            } else {
                fonts.body
            },
            theme.text,
            &item.label,
            text_rect,
            DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
        );
    }
    Some(bmp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn popup_opens_below_when_space_allows_and_flips_above_near_bottom() {
        let work = RECT {
            left: 0,
            top: 0,
            right: 1280,
            bottom: 720,
        };
        let anchor_top = RECT {
            left: 500,
            top: 100,
            right: 740,
            bottom: 132,
        };
        let (rect, up) = compute_popup_rect(anchor_top, work, 240, 180, 340, 4);
        assert!(!up);
        assert_eq!(rect.top, 136);
        assert_eq!(rect.bottom - rect.top, 180);

        let anchor_bottom = RECT {
            left: 500,
            top: 660,
            right: 740,
            bottom: 692,
        };
        let (rect, up) = compute_popup_rect(anchor_bottom, work, 240, 180, 340, 4);
        assert!(up);
        assert_eq!(rect.bottom, 656);
        assert_eq!(rect.bottom - rect.top, 180);
    }

    #[test]
    fn keyboard_navigation_skips_disabled_items_and_wraps() {
        let items = vec![
            OptionItem::command("A", 1),
            OptionItem {
                label: "Disabled".into(),
                action: None,
                swatch: None,
            },
            OptionItem::command("B", 2),
        ];
        assert_eq!(next_enabled_index(&items, Some(0), 1), Some(2));
        assert_eq!(next_enabled_index(&items, Some(2), 1), Some(0));
        assert_eq!(next_enabled_index(&items, Some(0), -1), Some(2));
    }
}
