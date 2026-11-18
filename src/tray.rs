//! Иконка в области уведомлений (TASK-N07): порт трея Soroka (TASK-037).
//!
//! Обычная иконка — конверт (`tray.png` Сороки), при непрочитанных — с
//! точкой (`tray-alert-1.png`, TASK-079). Подпись: «POS Mail — N
//! непрочитанных …». Меню правой кнопки: Открыть / Проверить почту /
//! Выход (команды уходят в главное окно тем же WM_COMMAND, что и меню).
//! Клик левой и по balloon-уведомлению показывает окно.
//!
//! Живёт в UI-потоке; все вызовы — из wndproc главного окна.

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIIF_RESPECT_QUIET_TIME, NIM_ADD,
    NIM_DELETE, NIM_MODIFY, NOTIFY_ICON_MESSAGE, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconFromResourceEx, CreatePopupMenu, DestroyMenu, GetCursorPos,
    GetSystemMetrics, HICON, LR_DEFAULTCOLOR, MF_STRING, SM_CXSMICON, TPM_BOTTOMALIGN,
    TPM_LEFTALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenuEx, WM_COMMAND, WM_NULL,
};

use crate::state::AppState;
use crate::wide::wide;
use crate::{ico, notify, ui};

/// Сообщение от иконки трея (lparam — событие мыши/balloon, wparam — id).
pub const WM_APP_TRAY: u32 = crate::WM_APP_ATTACH_CLICK + 1;

/// Идентификатор иконки (uID в NOTIFYICONDATAW).
const TRAY_UID: u32 = 1;

/// Версия формата иконок для CreateIconFromResourceEx (Win32, 0x00030000).
const ICON_VER: u32 = 0x0003_0000;

/// Иконки Сорока (src-tauri/icons), сконвертированные в .ico для среза:
/// окно/панель задач — app.ico, трей — tray.ico и tray-alert.ico.
static APP_ICO: &[u8] = include_bytes!("icons/app.ico");
static TRAY_ICO: &[u8] = include_bytes!("icons/tray.ico");
static TRAY_ALERT_ICO: &[u8] = include_bytes!("icons/tray-alert.ico");

/// Иконка окна/панели задач из app.ico (размер под мелкие системные иконки).
pub fn load_app_icon() -> Option<HICON> {
    load_icon(APP_ICO)
}

/// Иконки трея (обычная и с точкой непрочитанных) — в AppState.
pub fn load_tray_icons(st: &mut AppState) -> bool {
    let plain = load_icon(TRAY_ICO);
    let alert = load_icon(TRAY_ALERT_ICO);
    st.hicon_tray = plain.unwrap_or_default();
    st.hicon_tray_alert = alert.unwrap_or_default();
    plain.is_some() && alert.is_some()
}

/// Разбор .ico → HICON под текущий размер мелких иконок системы.
fn load_icon(data: &[u8]) -> Option<HICON> {
    let size = unsafe { GetSystemMetrics(SM_CXSMICON) }.max(16);
    let frame = ico::best_frame(data, size)?;
    unsafe { CreateIconFromResourceEx(frame, true, ICON_VER, size, size, LR_DEFAULTCOLOR).ok() }
}

/// Основа NOTIFYICONDATAW: окно-владелец, id, сообщение-калбэк.
fn nid(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        hWnd: hwnd,
        uID: TRAY_UID,
        uCallbackMessage: WM_APP_TRAY,
        ..Default::default()
    }
}

/// Копия строки в массив UTF-16 с обрезкой по границе символа (влезает
/// NUL). Для szTip/szInfo/szInfoTitle; суррогатные пары не рвутся.
fn put_wide(dst: &mut [u16], text: &str) {
    let cap = dst.len().saturating_sub(1); // место под завершающий нуль
    let mut n = 0usize;
    'outer: for ch in text.chars() {
        let mut buf = [0u16; 2];
        for unit in ch.encode_utf16(&mut buf) {
            if n >= cap {
                break 'outer;
            }
            dst[n] = *unit;
            n += 1;
        }
    }
    dst[n] = 0;
}

fn notify_icon(dwmessage: NOTIFY_ICON_MESSAGE, d: &NOTIFYICONDATAW) -> bool {
    unsafe { Shell_NotifyIconW(dwmessage, d) }.as_bool()
}

/// Добавить иконку (NIM_ADD) с подписью по текущему числу непрочитанных.
pub fn add(hwnd: HWND, st: &mut AppState) -> bool {
    if st.hicon_tray.is_invalid() {
        return false;
    }
    let mut d = nid(hwnd);
    d.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    d.hIcon = current_icon(st);
    put_wide(&mut d.szTip, &notify::tooltip(st.unread_total));
    st.tray_added = notify_icon(NIM_ADD, &d);
    st.tray_added
}

/// Убрать иконку (NIM_DELETE) — при выходе, до DestroyWindow.
pub fn remove(hwnd: HWND, st: &mut AppState) {
    if !st.tray_added {
        return;
    }
    let d = nid(hwnd);
    let _ = notify_icon(NIM_DELETE, &d);
    st.tray_added = false;
}

/// Вернуть иконку после перезапуска Explorer (сообщение TaskbarCreated).
pub fn readd(hwnd: HWND, st: &mut AppState) {
    if st.tray_added {
        remove(hwnd, st);
    }
    add(hwnd, st);
}

/// Обновить подпись и картинку по st.unread_total (порт set_tray_unread:
/// точка непрочитанных + «POS Mail — N …»).
pub fn refresh(hwnd: HWND, st: &mut AppState) {
    if !st.tray_added {
        return;
    }
    let mut d = nid(hwnd);
    d.uFlags = NIF_ICON | NIF_TIP;
    d.hIcon = current_icon(st);
    put_wide(&mut d.szTip, &notify::tooltip(st.unread_total));
    let _ = notify_icon(NIM_MODIFY, &d);
}

/// Balloon-уведомление о новых письмах (порт notify_new_mail; Windows
/// сам решает, показывать всплывашку или собрать в центр уведомлений).
pub fn balloon(hwnd: HWND, title: &str, body: &str) {
    let mut d = nid(hwnd);
    d.uFlags = NIF_INFO;
    put_wide(&mut d.szInfoTitle, title);
    put_wide(&mut d.szInfo, body);
    d.dwInfoFlags = NIIF_INFO | NIIF_RESPECT_QUIET_TIME;
    // Анонимное объединение: uTimeout (с Vista системой игнорируется,
    // но поле заполняем корректным значением).
    d.Anonymous.uTimeout = 10_000;
    let _ = notify_icon(NIM_MODIFY, &d);
}

fn current_icon(st: &AppState) -> HICON {
    if st.unread_total > 0 && !st.hicon_tray_alert.is_invalid() {
        st.hicon_tray_alert
    } else {
        st.hicon_tray
    }
}

/// Меню правой кнопки: Открыть / Проверить почту / Выход (порт меню
/// Soroka TASK-037 без «Свернуть в трей» — сворачивают кнопка «—» и
/// крестик). Выбор уходит в главное окно как WM_COMMAND от меню.
pub fn track_menu(hwnd: HWND) {
    unsafe {
        let Ok(menu) = CreatePopupMenu() else {
            return;
        };
        let _ = AppendMenuW(
            menu,
            MF_STRING,
            ui::IDM_TRAY_OPEN as usize,
            windows::core::PCWSTR(wide("Открыть POS Mail").as_ptr()),
        );
        let _ = AppendMenuW(
            menu,
            MF_STRING,
            ui::IDM_SYNC as usize,
            windows::core::PCWSTR(wide("Проверить почту").as_ptr()),
        );
        let _ = AppendMenuW(
            menu,
            MF_STRING,
            ui::IDM_EXIT as usize,
            windows::core::PCWSTR(wide("Выход").as_ptr()),
        );
        // Позиция — курсор мыши; меню не должно исчезнуть при потере
        // фокуса: SetForegroundWindow до и WM_NULL после (классика KB135788).
        let mut pt = windows::Win32::Foundation::POINT::default();
        if GetCursorPos(&mut pt).is_err() {
            let _ = DestroyMenu(menu);
            return;
        }
        let _ = windows::Win32::UI::WindowsAndMessaging::SetForegroundWindow(hwnd);
        // С TPM_RETURNCMD выбранная команда возвращается вызовом (тип BOOL
        // в windows-rs — старшее слово не теряется, значение маленькое).
        let cmd = TrackPopupMenuEx(
            menu,
            (TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_LEFTALIGN | TPM_BOTTOMALIGN).0,
            pt.x,
            pt.y,
            hwnd,
            None,
        );
        let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
            Some(hwnd),
            WM_NULL,
            WPARAM(0),
            LPARAM(0),
        );
        let _ = DestroyMenu(menu);
        if cmd.0 != 0 {
            // Тот же путь, что и выбор из главного меню: lparam = 0,
            // wparam = команда.
            let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                Some(hwnd),
                WM_COMMAND,
                WPARAM(cmd.0 as usize),
                LPARAM(0),
            );
        }
    }
}
