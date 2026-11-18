//! Мелкие помощники для UTF-16 ↔ String в Win32-вызовах.

/// `&str` → буфер UTF-16 с завершающим нулём (для SetWindowTextW и т.п.).
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Записывает текст в окно: буфер живёт до конца вызова.
///
/// Заменяет собою прежний `wptr`: тот возвращал PCWSTR на ВРЕМЕННЫЙ Vec —
/// память освобождалась сразу после выхода из функции, и SetWindowTextW
/// читал уже освобождённый буфер (в теле ответа вместо цитаты оставалось
/// 3 символа мусора).
pub fn set_window_text(
    hwnd: windows::Win32::Foundation::HWND,
    text: &str,
) -> windows::core::Result<()> {
    let buf = wide(text);
    unsafe {
        windows::Win32::UI::WindowsAndMessaging::SetWindowTextW(
            hwnd,
            windows::core::PCWSTR(buf.as_ptr()),
        )
    }
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
