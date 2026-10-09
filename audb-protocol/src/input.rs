//! Limits and key names shared by host and device, checked before dispatch.
pub const MAX_TEXT_BYTES: usize = 16384;
pub fn validate_text(text: &str, delay_ms: u64) -> Result<usize, String> {
    let count = text.chars().count();
    if text.len() > MAX_TEXT_BYTES
        || count > 4096
        || delay_ms > 1000
        || (count.saturating_sub(1) as u64).saturating_mul(delay_ms) > 10_000
    {
        return Err("Text limit: 4096 code points, 16 KiB, delay 0..1000 ms, total delay <=10 s; use --delay 0 for longer text".into());
    }
    if text
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\t' | '\n'))
    {
        return Err("Control characters other than tab and newline are unsupported".into());
    }
    if !text.is_empty() && text.chars().all(|c| c == '\n') {
        return Err("Use audb key enter for newline-only input".into());
    }
    Ok(count)
}

pub fn key_code(name: &str) -> Option<(u16, &'static str)> {
    Some(match name.to_ascii_lowercase().as_str() {
        "enter" | "return" | "ret" => (28, "ret"),
        "backspace" | "bs" => (14, "backspace"),
        "delete" | "del" => (111, "delete"),
        "escape" | "esc" => (1, "esc"),
        "tab" => (15, "tab"),
        "space" | "spc" => (57, "spc"),
        "left" => (105, "left"),
        "right" => (106, "right"),
        "up" => (103, "up"),
        "down" => (108, "down"),
        "home" => (102, "home"),
        "end" => (107, "end"),
        "pageup" | "pgup" => (104, "pgup"),
        "pagedown" | "pgdn" => (109, "pgdn"),
        "insert" | "ins" => (110, "insert"),
        "volumeup" | "volup" | "vol+" => (115, "volumeup"),
        "volumedown" | "voldown" | "vol-" => (114, "volumedown"),
        "mute" | "audiomute" => (113, "audiomute"),
        "power" => (116, "power"),
        "capslock" | "caps_lock" => (58, "caps_lock"),
        "shift" | "shift_l" => (42, "shift"),
        "shift_r" => (54, "shift_r"),
        "ctrl" | "control" | "ctrl_l" => (29, "ctrl"),
        "ctrl_r" => (97, "ctrl_r"),
        "alt" | "alt_l" => (56, "alt"),
        "alt_r" => (100, "alt_r"),
        "f1" => (59, "f1"),
        "f2" => (60, "f2"),
        "f3" => (61, "f3"),
        "f4" => (62, "f4"),
        "f5" => (63, "f5"),
        "f6" => (64, "f6"),
        "f7" => (65, "f7"),
        "f8" => (66, "f8"),
        "f9" => (67, "f9"),
        "f10" => (68, "f10"),
        "f11" => (87, "f11"),
        "f12" => (88, "f12"),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unicode_counts_scalars_and_bounds_execution_time() {
        assert_eq!(validate_text("Привет 🦀 e\u{301}\nend", 50).unwrap(), 15);
        assert!(validate_text(&"я".repeat(202), 50).is_err());
        assert!(validate_text(&"я".repeat(4096), 0).is_ok());
        assert!(validate_text(&"a".repeat(4097), 0).is_err());
        assert!(validate_text("abc\0", 0).is_err());
        assert!(validate_text("\n\n", 0).is_err());
    }
    #[test]
    fn key_aliases_are_explicit() {
        assert_eq!(key_code("ENTER"), key_code("ret"));
        assert_eq!(key_code("vol+"), Some((115, "volumeup")));
        assert!(key_code("enter; reboot").is_none());
    }
}
