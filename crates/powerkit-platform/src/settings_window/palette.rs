//! Colors of the settings window, after the Windows 11 Fluent design tokens, in
//! light and dark, with the user's accent color.

use crate::shell::registry_get_binary;

/// Colors as 0xAARRGGBB; translucent ones are drawn over the layer beneath.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Palette {
    pub dark: bool,
    pub window: u32,
    pub layer: u32,
    pub layer_stroke: u32,
    pub card: u32,
    pub card_stroke: u32,
    pub text: u32,
    pub text2: u32,
    pub text3: u32,
    pub disabled: u32,
    pub control: u32,
    pub control_hover: u32,
    pub control_pressed: u32,
    pub control_stroke: u32,
    pub control_stroke_bottom: u32,
    /// A text box with the keyboard focus.
    pub field_focus: u32,
    pub subtle_hover: u32,
    pub subtle_pressed: u32,
    pub strong_stroke: u32,
    pub divider: u32,
    pub accent: u32,
    pub accent_hover: u32,
    pub on_accent: u32,
    pub smoke: u32,
    pub dialog: u32,
    pub dialog_footer: u32,
    pub info: (u32, u32),
    pub success: (u32, u32),
    pub warning: (u32, u32),
    pub error: (u32, u32),
}

/// Windows 11's default blue, used when the accent palette cannot be read:
/// (light 2, light 1, dark 1, dark 2).
const DEFAULT_ACCENT: [u32; 4] = [0xFF4C_C2FF, 0xFF00_91F8, 0xFF00_67C0, 0xFF00_3E92];

impl Palette {
    pub fn new(dark: bool, accent: [u32; 4]) -> Palette {
        let [light2, light1, dark1, dark2] = accent;
        if dark {
            Palette {
                dark,
                window: 0xFF20_2020,
                layer: 0x4C3A_3A3A,
                layer_stroke: 0x1900_0000,
                card: 0x0DFF_FFFF,
                card_stroke: 0x1900_0000,
                text: 0xFFFF_FFFF,
                text2: 0xC5FF_FFFF,
                text3: 0x87FF_FFFF,
                disabled: 0x5DFF_FFFF,
                control: 0x0FFF_FFFF,
                control_hover: 0x15FF_FFFF,
                control_pressed: 0x08FF_FFFF,
                control_stroke: 0x12FF_FFFF,
                control_stroke_bottom: 0x18FF_FFFF,
                field_focus: 0xB31E_1E1E,
                subtle_hover: 0x0FFF_FFFF,
                subtle_pressed: 0x0AFF_FFFF,
                strong_stroke: 0x8BFF_FFFF,
                divider: 0x15FF_FFFF,
                accent: light2,
                accent_hover: light1 & 0x00FF_FFFF | 0xE600_0000,
                on_accent: 0xFF00_0000,
                smoke: 0x4D00_0000,
                dialog: 0xFF2B_2B2B,
                dialog_footer: 0xFF20_2020,
                info: (0x08FF_FFFF, light2),
                success: (0xFF39_3D1B, 0xFF6C_CB5F),
                warning: (0xFF43_3519, 0xFFFC_E100),
                error: (0xFF44_2726, 0xFFFF_99A4),
            }
        } else {
            Palette {
                dark,
                window: 0xFFF3_F3F3,
                layer: 0x80FF_FFFF,
                layer_stroke: 0x0F00_0000,
                card: 0xB3FF_FFFF,
                card_stroke: 0x0F00_0000,
                text: 0xE400_0000,
                text2: 0x9E00_0000,
                text3: 0x7200_0000,
                disabled: 0x5C00_0000,
                control: 0xB3FF_FFFF,
                control_hover: 0x80F9_F9F9,
                control_pressed: 0x4DF9_F9F9,
                control_stroke: 0x0F00_0000,
                control_stroke_bottom: 0x2900_0000,
                field_focus: 0xFFFF_FFFF,
                subtle_hover: 0x0900_0000,
                subtle_pressed: 0x0600_0000,
                strong_stroke: 0x9C00_0000,
                divider: 0x1400_0000,
                accent: dark1,
                accent_hover: dark2 & 0x00FF_FFFF | 0xE600_0000,
                on_accent: 0xFFFF_FFFF,
                smoke: 0x4D00_0000,
                dialog: 0xFFFF_FFFF,
                dialog_footer: 0xFFF3_F3F3,
                info: (0x80F6_F6F6, dark1),
                success: (0xFFDF_F6DD, 0xFF0F_7B0F),
                warning: (0xFFFF_F4CE, 0xFF9D_5D00),
                error: (0xFFFD_E7E9, 0xFFC4_2B1C),
            }
        }
    }

    /// The palette for the current Windows settings.
    pub fn current(dark: bool) -> Palette {
        Palette::new(dark, accent().unwrap_or(DEFAULT_ACCENT))
    }
}

/// The user's accent shades from the registry: eight RGBA entries, from lightest
/// (0) to darkest (6).
fn accent() -> Option<[u32; 4]> {
    let raw = registry_get_binary(
        r"Software\Microsoft\Windows\CurrentVersion\Explorer\Accent",
        "AccentPalette",
    )
    .inspect_err(|e| tracing::debug!("accent palette: {e}"))
    .ok()??;
    let shade = |i: usize| {
        let c = raw.get(i * 4..i * 4 + 3)?;
        Some(0xFF00_0000 | u32::from(c[0]) << 16 | u32::from(c[1]) << 8 | u32::from(c[2]))
    };
    Some([shade(1)?, shade(2)?, shade(4)?, shade(5)?])
}

/// `top` drawn over the opaque `bottom`, as one opaque color.
pub(crate) fn over(bottom: u32, top: u32) -> u32 {
    let a = top >> 24;
    let mix = |shift: u32| {
        let (b, t) = ((bottom >> shift) & 0xFF, (top >> shift) & 0xFF);
        ((t * a + b * (255 - a) + 127) / 255) << shift
    };
    0xFF00_0000 | mix(16) | mix(8) | mix(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn over_blends_by_alpha() {
        assert_eq!(over(0xFF00_0000, 0xFFFF_FFFF), 0xFFFF_FFFF);
        assert_eq!(over(0xFF12_3456, 0x00FF_FFFF), 0xFF12_3456);
        assert_eq!(over(0xFF00_0000, 0x80FF_FFFF), 0xFF80_8080);
    }

    #[test]
    fn dark_text_is_light() {
        let d = Palette::new(true, DEFAULT_ACCENT);
        let l = Palette::new(false, DEFAULT_ACCENT);
        assert_eq!(d.text & 0xFF_FFFF, 0xFF_FFFF);
        assert_eq!(l.text & 0xFF_FFFF, 0);
        assert_ne!(d.accent, l.accent);
    }
}
