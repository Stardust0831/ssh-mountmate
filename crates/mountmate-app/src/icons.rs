//! Native icon pixels generated from the same SVG as the packaged ICO and ICNS.
//! Raw RGBA keeps window and tray creation independent of runtime image decoders.

const WINDOW_SIZE: u32 = 256;
const TRAY_SIZE: u32 = 32;
const WINDOW_RGBA: &[u8; 256 * 256 * 4] =
    include_bytes!("../../../assets/ssh-mountmate-logo-256.rgba");
const TRAY_RGBA: &[u8; 32 * 32 * 4] = include_bytes!("../../../assets/ssh-mountmate-logo-32.rgba");

pub(crate) fn window_icon() -> iced::window::Icon {
    iced::window::icon::from_rgba(WINDOW_RGBA.to_vec(), WINDOW_SIZE, WINDOW_SIZE)
        .expect("bundled window icon has valid RGBA dimensions")
}

pub(crate) fn tray_icon() -> Result<tray_icon::Icon, String> {
    tray_icon::Icon::from_rgba(TRAY_RGBA.to_vec(), TRAY_SIZE, TRAY_SIZE)
        .map_err(|error| error.to_string())
}
