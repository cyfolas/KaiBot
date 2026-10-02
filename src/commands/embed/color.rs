//! Цвет embed из названия или HEX.

use serenity::all::Colour;

use crate::error::{AppError, Result};

pub const DEFAULT: Colour = Colour(0x0058_65F2);

/// Именованные цвета — палитра Discord.
const NAMED: &[(&str, u32)] = &[
    ("blurple", 0x0058_65F2),
    ("синий", 0x0058_65F2),
    ("green", 0x0057_F287),
    ("зелёный", 0x0057_F287),
    ("зеленый", 0x0057_F287),
    ("yellow", 0x00FE_E75C),
    ("жёлтый", 0x00FE_E75C),
    ("желтый", 0x00FE_E75C),
    ("fuchsia", 0x00EB_459E),
    ("pink", 0x00EB_459E),
    ("розовый", 0x00EB_459E),
    ("red", 0x00ED_4245),
    ("красный", 0x00ED_4245),
    ("white", 0x00FF_FFFF),
    ("белый", 0x00FF_FFFF),
    ("black", 0x0000_0000),
    ("чёрный", 0x0000_0000),
    ("черный", 0x0000_0000),
    ("dark", 0x002B_2D31),
    ("тёмный", 0x002B_2D31),
    ("темный", 0x002B_2D31),
];

/// Цвет из названия или HEX: `#5865F2`, `0x5865F2`, `5865F2`, `#F00`.
pub fn parse(raw: &str) -> Result<Colour> {
    let text = raw.trim();
    let lower = text.to_lowercase();
    if let Some(&(_, value)) = NAMED.iter().find(|(name, _)| *name == lower) {
        return Ok(Colour(value));
    }

    let hex = text
        .strip_prefix('#')
        .or_else(|| text.strip_prefix("0x"))
        .or_else(|| text.strip_prefix("0X"))
        .unwrap_or(text);
    // `from_str_radix` принимает ведущий `+`, поэтому символы проверяются явно.
    let digits_ok = hex.chars().all(|c| c.is_ascii_hexdigit());

    let value = match hex.len() {
        6 if digits_ok => u32::from_str_radix(hex, 16).ok(),
        3 if digits_ok => u32::from_str_radix(hex, 16).ok().map(expand_short_hex),
        _ => None,
    };
    value.map(Colour).ok_or_else(|| {
        AppError::user(format!(
            "Не удалось распознать цвет «{text}». Используйте HEX (#5865F2, #F00) или название: \
             red, green, yellow, blurple…"
        ))
    })
}

/// `0xRGB` → `0xRRGGBB`.
fn expand_short_hex(short: u32) -> u32 {
    let (r, g, b) = ((short >> 8) & 0xF, (short >> 4) & 0xF, short & 0xF);
    ((r * 0x11) << 16) | ((g * 0x11) << 8) | (b * 0x11)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_named_and_hex_colors() {
        assert_eq!(parse("blurple").unwrap().0, 0x0058_65F2);
        assert_eq!(parse(" Зелёный ").unwrap().0, 0x0057_F287);
        assert_eq!(parse("#ED4245").unwrap().0, 0x00ED_4245);
        assert_eq!(parse("0x57f287").unwrap().0, 0x0057_F287);
        assert_eq!(parse("FFFFFF").unwrap().0, 0x00FF_FFFF);
        assert_eq!(parse("#F0a").unwrap().0, 0x00FF_00AA);
    }

    #[test]
    fn rejects_invalid_colors() {
        for raw in ["", "#12345", "#GGGGGG", "+FFFFF", "#+FF", "purple-ish"] {
            assert!(parse(raw).is_err(), "{raw:?} должен быть отклонён");
        }
    }
}
