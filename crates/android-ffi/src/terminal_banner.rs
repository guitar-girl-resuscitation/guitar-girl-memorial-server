//! Small, deterministic startup wordmark generator. No game resources, shell,
//! network, Python runtime, or running database are required.
//! ANSI Shadow glyph subset: see THIRD_PARTY_TERMINAL_FONT.md.

fn glyph(c: char) -> [&'static str; 6] {
    match c {
        'A' => [" █████╗ ", "██╔══██╗", "███████║", "██╔══██║", "██║  ██║", "╚═╝  ╚═╝"],
        'I' => ["██╗", "██║", "██║", "██║", "██║", "╚═╝"],
        'R' => ["██████╗ ", "██╔══██╗", "██████╔╝", "██╔══██╗", "██║  ██║", "╚═╝  ╚═╝"],
        'S' => ["███████╗", "██╔════╝", "███████╗", "╚════██║", "███████║", "╚══════╝"],
        'U' => ["██╗   ██╗", "██║   ██║", "██║   ██║", "██║   ██║", "╚██████╔╝", " ╚═════╝ "],
        'T' => ["████████╗", "╚══██╔══╝", "   ██║   ", "   ██║   ", "   ██║   ", "   ╚═╝   "],
        'E' => ["███████╗", "██╔════╝", "█████╗  ", "██╔══╝  ", "███████╗", "╚══════╝"],
        'K' => ["██╗  ██╗", "██║ ██╔╝", "█████╔╝ ", "██╔═██╗ ", "██║  ██╗", "╚═╝  ╚═╝"],
        _ => [""; 6],
    }
}

/// At least 66 columns: one line; otherwise AIRISU / TEK within 46 columns.
/// A still narrower viewport must scroll horizontally, not wrap a glyph.
pub fn render(columns: u32) -> String {
    let words: &[&str] = if columns >= 66 { &["AIRISUTEK"] } else { &["AIRISU", "TEK"] };
    let mut result = String::new();
    for (index, word) in words.iter().enumerate() {
        if index != 0 { result.push('\n'); }
        for row in 0..6 {
            let line: String = word.chars().map(|c| glyph(c)[row]).collect();
            result.push_str(line.trim_end());
            result.push('\n');
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn responsive_wordmark_never_wraps_glyphs() {
        for columns in [0, 1, 32, 45, 46, 65, 66, 100, u32::MAX] {
            let text = render(columns);
            let wide = columns >= 66;
            assert_eq!(text.lines().count(), if wide { 6 } else { 13 });
            assert!(text.lines().all(|line| line.chars().count() <= if wide { 66 } else { 46 }));
            assert!(!text.contains('\0'));
        }
        assert_eq!(render(66), render(100));
        assert_eq!(render(46), render(65));
        assert!(render(66).contains("██████╗"));
    }

    #[test]
    fn ffi_reports_capacity_and_never_partially_writes_utf8() {
        let required = unsafe { crate::ggfm_server_startup_banner(66, std::ptr::null_mut(), 0) };
        let mut short = vec![0x55_u8; required - 1];
        assert_eq!(unsafe { crate::ggfm_server_startup_banner(66, short.as_mut_ptr().cast(), short.len()) }, required);
        assert!(short.iter().all(|v| *v == 0x55));
        let mut bytes = vec![0x55_u8; required + 1];
        assert_eq!(unsafe { crate::ggfm_server_startup_banner(66, bytes.as_mut_ptr().cast(), required) }, required);
        assert_eq!(&bytes[..required - 1], render(66).as_bytes());
        assert_eq!(bytes[required - 1], 0);
        assert_eq!(bytes[required], 0x55);
    }
}
