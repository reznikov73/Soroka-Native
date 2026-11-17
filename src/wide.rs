//! Мелкие помощники для UTF-16 ↔ String в Win32-вызовах.

/// `&str` → буфер UTF-16 с завершающим нулём (для SetWindowTextW и т.п.).
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Указатель на начало буфера `wide()`.
pub fn wptr(s: &str) -> windows::core::PCWSTR {
    windows::core::PCWSTR(wide(s).as_ptr())
}

/// Читает текст из Edit/Static окна (GetWindowTextW).
pub fn read_text(hwnd: windows::Win32::Foundation::HWND) -> String {
    unsafe {
        let len = windows::Win32::UI::WindowsAndMessaging::GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return String::new();
        }
        let mut buf = vec![0u16; len as usize + 1];
        let got = windows::Win32::UI::WindowsAndMessaging::GetWindowTextW(hwnd, &mut buf);
        let got = got.max(0) as usize;
        String::from_utf16_lossy(&buf[..got.min(buf.len() - 1)])
    }
}

/// Экранирование HTML для текстовых вставок в шаблон панели письма.
pub fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}
