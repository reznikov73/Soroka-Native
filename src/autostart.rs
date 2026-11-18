//! Автозапуск при входе в Windows (TASK-N07): ключ реестра
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.
//!
//! Флаг `ui.autostart` общий с Soroka (settings.json, TASK-099), но ярлык
//! запуска у каждого клиента свой: Сорока пишет свой (tauri-plugin-
//! autostart), мы — значение `SorokaNative` с путём к нашему exe.
//! Синхронизируем при старте и после сохранения настроек.

use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_SZ, RegCloseKey, RegDeleteValueW, RegOpenKeyExW,
    RegSetValueExW,
};
use windows::core::PCWSTR;

/// Имя значения в ...\Run (не «Soroka» — чтобы не столкнуться с ярлыком
/// самой Сороки, если у неё включён автозапуск).
const VALUE_NAME: &str = "SorokaNative";

/// Раздел автозапуска текущего пользователя.
const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";

/// Путь к нашему exe (для значения реестра).
fn exe_path() -> Option<String> {
    let mut buf = [0u16; 1024];
    // 0 — не влезло/не нашли; len == размер буфера — путь обрезан.
    let len = unsafe { GetModuleFileNameW(None, &mut buf) };
    if len == 0 || len as usize >= buf.len() {
        return None;
    }
    Some(String::from_utf16_lossy(&buf[..len as usize]))
}

/// Приведение флага в жизнь: включено — пишем `"...путь\к\POSmail.exe"`,
/// выключено — удаляем значение. Возвращает успех (отсутствующее значение
/// при удалении — тоже успех).
pub fn apply(enabled: bool) -> bool {
    let Some(path) = exe_path() else {
        return false;
    };
    let name = crate::wide::wide(VALUE_NAME);
    let key = crate::wide::wide(RUN_KEY);
    // Открытие не создаёт раздел: Run существует в любой Windows, а если
    // его нет — автозапуск всё равно не работал бы.
    let mut hkey = HKEY::default();
    let opened = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(key.as_ptr()),
            None,
            KEY_SET_VALUE,
            &mut hkey,
        )
    };
    if opened.is_err() {
        return false;
    }
    let result = if enabled {
        // Кавычки: пробелы в пути (C:\Program Files\...) не разорвут команду.
        let quoted = crate::wide::wide(&format!("\"{path}\""));
        // Данные значения реестра REG_SZ — UTF-16 байты вместе с NUL.
        let bytes = unsafe {
            std::slice::from_raw_parts(
                quoted.as_ptr().cast::<u8>(),
                quoted.len() * std::mem::size_of::<u16>(),
            )
        };
        unsafe { RegSetValueExW(hkey, PCWSTR(name.as_ptr()), None, REG_SZ, Some(bytes)) }.is_ok()
    } else {
        let r = unsafe { RegDeleteValueW(hkey, PCWSTR(name.as_ptr())) };
        r.is_ok() || r == ERROR_FILE_NOT_FOUND
    };
    unsafe {
        let _ = RegCloseKey(hkey);
    }
    result
}

#[cfg(test)]
mod tests {
    #[test]
    fn value_name_is_stable() {
        // Имя значения — часть контракта с уже установленными копиями:
        // смена имени оставила бы сиротские записи в реестре.
        assert_eq!(super::VALUE_NAME, "SorokaNative");
        assert!(super::RUN_KEY.ends_with("CurrentVersion\\Run"));
    }
}
