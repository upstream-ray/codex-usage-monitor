//! Native Windows color dialog. Cancellation returns no preference change.
use std::sync::Mutex;

use windows::Win32::Foundation::{COLORREF, HWND};
use windows::Win32::UI::Controls::Dialogs::{ChooseColorW, CC_FULLOPEN, CC_RGBINIT, CHOOSECOLORW};

use crate::codex_colors::Rgb;
use crate::native_interop::Color;

pub fn choose(owner: HWND, initial: Color) -> Option<Rgb> {
    static CUSTOM_COLORS: Mutex<[COLORREF; 16]> = Mutex::new([COLORREF(0x00ffffff); 16]);
    let mut custom = CUSTOM_COLORS
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let mut dialog = CHOOSECOLORW {
        lStructSize: std::mem::size_of::<CHOOSECOLORW>() as u32,
        hwndOwner: owner,
        rgbResult: COLORREF(initial.to_colorref()),
        lpCustColors: custom.as_mut_ptr(),
        Flags: CC_RGBINIT | CC_FULLOPEN,
        ..Default::default()
    };
    unsafe {
        ChooseColorW(&mut dialog)
            .as_bool()
            .then(|| Rgb::from_colorref(dialog.rgbResult.0))
    }
}
