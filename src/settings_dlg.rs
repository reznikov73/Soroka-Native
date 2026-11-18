//! Окно настроек (TASK-N09): тема, поведение (трей/уведомления/автозапуск),
//! интервал автопроверки для будущего трея (N07), ключ GigaChat (N13).
//!
//! settings.json общий с Soroka (`%APPDATA%\Soroka\settings.json`): читаем
//! всё, пишем только свои ключи (`native_theme`, `native_autosync_min`) и
//! поля `ui.*` Soroka — read-modify-write, остальное содержимое файла
//! сохраняется (см. settings.rs). Ключ GigaChat — в
//! `%APPDATA%\Soroka\gigachat.json` (как в Soroka, TASK-041).
//!
//! Геометрию окна не храним (layout.json — только main/compose/account,
//! формат зафиксирован для синтетических тестов AutoClaw): диалог маленький
//! и редкий, системная позиция достаточна.

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{DEFAULT_GUI_FONT, HGDIOBJ};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{BST_CHECKED, BST_UNCHECKED};
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::{
    BM_GETCHECK, BM_SETCHECK, BN_CLICKED, BS_AUTOCHECKBOX, CB_ADDSTRING, CB_GETCURSEL,
    CB_SETCURSEL, CBS_DROPDOWNLIST, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow,
    ES_AUTOHSCROLL, GWLP_USERDATA, GetClientRect, GetWindowLongPtrW, IDC_ARROW, LoadCursorW,
    MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MessageBoxW, RegisterClassW, SendMessageW,
    WINDOW_EX_STYLE, WINDOW_STYLE, WM_COMMAND, WM_CREATE, WM_DESTROY, WM_NCCREATE, WM_SETFONT,
    WM_SIZE, WNDCLASSW, WS_CHILD, WS_OVERLAPPEDWINDOW, WS_TABSTOP, WS_VISIBLE,
};
use windows::core::{PCWSTR, w};

use crate::ai::gigachat;
use crate::settings::{self, NativeView};
use crate::state::AppState;
use crate::wide::{read_text, set_window_text, wide};

/// Главное окно перечитывает настройки и применяет тему после сохранения.
pub const WM_APP_SETTINGS_SAVED: u32 = crate::account_dlg::WM_APP_ACCOUNT_CLOSED + 1;
/// Главное окно чистит st.hwnd_settings при закрытии окна настроек.
pub const WM_APP_SETTINGS_CLOSED: u32 = WM_APP_SETTINGS_SAVED + 1;

const WINDOW_CLASS: PCWSTR = w!("SorokaSettingsWnd");

const IDC_THEME: i32 = 400; // combobox: 0 — тёмная, 1 — светлая
const IDC_CHK_TRAY: i32 = 401;
const IDC_CHK_NOTIF: i32 = 402;
const IDC_CHK_AUTOSTART: i32 = 403;
const IDC_AUTOSYNC: i32 = 404;
const IDC_GIGA_KEY: i32 = 405;
const IDC_BTN_GIGA_CLEAR: i32 = 406;
const IDC_BTN_SAVE: i32 = 407;
const IDC_BTN_CANCEL: i32 = 408;
const IDC_KEYINFO: i32 = 409;
const IDC_STATUS: i32 = 410;

/// Состояние окна настроек.
struct DlgState {
    parent: HWND,
    hwnd_theme: HWND,
    hwnd_chk_tray: HWND,
    hwnd_chk_notif: HWND,
    hwnd_chk_autostart: HWND,
    hwnd_autosync: HWND,
    hwnd_giga_key: HWND,
    hwnd_btn_giga_clear: HWND,
    hwnd_btn_save: HWND,
    hwnd_btn_cancel: HWND,
    hwnd_keyinfo: HWND,
    hwnd_status: HWND,
    /// Ключ лежал в файле при открытии: пустое поле при сохранении —
    /// удалить файловый ключ.
    had_file_key: bool,
}

/// Открывает окно настроек (одно на приложение). Возвращает HWND.
pub fn open(hwnd_main: HWND, st: &AppState) -> Option<HWND> {
    if let Some(h) = st.hwnd_settings {
        let _ = unsafe { SetFocus(Some(h)) };
        return None;
    }

    let hmod = unsafe { GetModuleHandleW(None) }.expect("GetModuleHandleW");
    let wc = WNDCLASSW {
        lpfnWndProc: Some(wndproc),
        hInstance: windows::Win32::Foundation::HINSTANCE(hmod.0),
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW) }.expect("LoadCursorW"),
        lpszClassName: WINDOW_CLASS,
        ..Default::default()
    };
    unsafe { RegisterClassW(&wc) };

    let state = Box::new(DlgState {
        parent: hwnd_main,
        hwnd_theme: HWND::default(),
        hwnd_chk_tray: HWND::default(),
        hwnd_chk_notif: HWND::default(),
        hwnd_chk_autostart: HWND::default(),
        hwnd_autosync: HWND::default(),
        hwnd_giga_key: HWND::default(),
        hwnd_btn_giga_clear: HWND::default(),
        hwnd_btn_save: HWND::default(),
        hwnd_btn_cancel: HWND::default(),
        hwnd_keyinfo: HWND::default(),
        hwnd_status: HWND::default(),
        had_file_key: false,
    });
    let raw = Box::into_raw(state);

    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            WINDOW_CLASS,
            w!("Настройки — POS Mail"),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            540,
            400,
            Some(hwnd_main),
            None,
            Some(windows::Win32::Foundation::HINSTANCE(hmod.0)),
            Some(raw as *const std::ffi::c_void),
        )
        .expect("создать окно настроек")
    };
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::ShowWindow(
            hwnd,
            windows::Win32::UI::WindowsAndMessaging::SW_SHOW,
        );
    }
    Some(hwnd)
}

unsafe fn st_of(hwnd: HWND) -> Option<&'static mut DlgState> {
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut DlgState;
    if ptr.is_null() {
        None
    } else {
        Some(unsafe { &mut *ptr })
    }
}

// 9 параметров — обычная сигнатура CreateWindowExW; разбивать ради линта не будем.
#[allow(clippy::too_many_arguments)]
unsafe fn child(
    parent: HWND,
    class: PCWSTR,
    text: Option<&str>,
    style: u32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    id: i32,
) -> HWND {
    let hmod = unsafe { GetModuleHandleW(None) }.expect("GetModuleHandleW");
    let mut text_buf = text.map(wide);
    let text_ptr = match text_buf.as_mut() {
        Some(b) => PCWSTR(b.as_mut_ptr()),
        None => PCWSTR::null(),
    };
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            text_ptr,
            WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | style),
            x,
            y,
            w,
            h,
            Some(parent),
            // ID контрола передаётся в hMenu дочернего окна — по нему WM_COMMAND
            // и разбирает нажатия (IDC_THEME, IDC_CHK_TRAY и т.д.).
            Some(windows::Win32::UI::WindowsAndMessaging::HMENU(
                id as isize as *mut std::ffi::c_void,
            )),
            Some(windows::Win32::Foundation::HINSTANCE(hmod.0)),
            None,
        )
    }
    .expect("создать контрол")
}

unsafe fn set_font(hwnd: HWND, font: HGDIOBJ) {
    unsafe {
        SendMessageW(
            hwnd,
            WM_SETFONT,
            Some(WPARAM(font.0 as usize)),
            Some(LPARAM(1)),
        );
    }
}

unsafe fn move_window(hwnd: HWND, x: i32, y: i32, w: i32, h: i32) {
    let _ = unsafe {
        windows::Win32::UI::WindowsAndMessaging::SetWindowPos(
            hwnd,
            None,
            x,
            y,
            w,
            h,
            windows::Win32::UI::WindowsAndMessaging::SWP_NOZORDER,
        )
    };
}

unsafe fn layout(hwnd: HWND, st: &DlgState) {
    let mut rc = RECT::default();
    if unsafe { GetClientRect(hwnd, &mut rc) }.is_err() {
        return;
    }
    let w = rc.right;
    let h = rc.bottom;
    let m = 14;
    unsafe {
        move_window(st.hwnd_theme, m + 190, 16, (w - m * 2 - 190).max(180), 24);
        move_window(st.hwnd_chk_tray, m, 52, (w - m * 2).max(200), 20);
        move_window(st.hwnd_chk_notif, m, 78, (w - m * 2).max(200), 20);
        move_window(st.hwnd_chk_autostart, m, 104, (w - m * 2).max(200), 20);
        move_window(st.hwnd_autosync, m + 330, 140, 70, 24);
        move_window(
            st.hwnd_giga_key,
            m + 190,
            174,
            (w - m * 2 - 190).max(180),
            24,
        );
        move_window(st.hwnd_keyinfo, m, 206, (w - m * 2).max(200), 34);
        move_window(st.hwnd_btn_giga_clear, m, h - 44, 120, 30);
        move_window(st.hwnd_btn_save, (w - 240).max(m + 130), h - 44, 120, 30);
        move_window(st.hwnd_btn_cancel, (w - 110).max(m + 260), h - 44, 100, 30);
        move_window(st.hwnd_status, m, h - 76, (w - m * 2).max(200), 24);
    }
}

/// Текст-пояснение о состоянии ключа GigaChat.
fn key_info_text() -> String {
    let status = gigachat::key_status();
    match status.source.as_str() {
        "env" => {
            "Ключ задан в переменной окружения SOROKA_GIGACHAT_KEY — она имеет\r\n\
             приоритет над файлом; поле ниже меняет только gigachat.json."
        }
        "file" => "Ключ сохранён в %APPDATA%\\Soroka\\gigachat.json (как в Soroka).",
        _ => {
            "Ключ не задан. ИИ-функции Фазы 3 (N13/N14) будут отключены;\r\n\
             почта работает без ключа."
        }
    }
    .to_string()
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_NCCREATE => {
            let cs = unsafe {
                &*(lparam.0 as *const windows::Win32::UI::WindowsAndMessaging::CREATESTRUCTW)
            };
            unsafe {
                windows::Win32::UI::WindowsAndMessaging::SetWindowLongPtrW(
                    hwnd,
                    GWLP_USERDATA,
                    cs.lpCreateParams as isize,
                );
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_CREATE => {
            let Some(st) = (unsafe { st_of(hwnd) }) else {
                return LRESULT(-1);
            };
            let hfont = unsafe { windows::Win32::Graphics::Gdi::GetStockObject(DEFAULT_GUI_FONT) };
            unsafe {
                // Тема оформления.
                let h = child(
                    hwnd,
                    w!("STATIC"),
                    Some("Тема оформления:"),
                    0,
                    14,
                    19,
                    190,
                    20,
                    -1,
                );
                set_font(h, hfont);
                st.hwnd_theme = child(
                    hwnd,
                    w!("COMBOBOX"),
                    None,
                    WS_TABSTOP.0 | CBS_DROPDOWNLIST as u32,
                    204,
                    14,
                    200,
                    200,
                    IDC_THEME,
                );
                set_font(st.hwnd_theme, hfont);
                for item in ["Тёмная (палитра Soroka)", "Светлая"] {
                    let buf = wide(item);
                    SendMessageW(
                        st.hwnd_theme,
                        CB_ADDSTRING,
                        Some(WPARAM(0)),
                        Some(LPARAM(buf.as_ptr() as isize)),
                    );
                }

                // Поведение (поля ui.* Soroka — файл общий).
                st.hwnd_chk_tray = child(
                    hwnd,
                    w!("BUTTON"),
                    Some("Кнопка «—» прячет окно в трей"),
                    WS_TABSTOP.0 | BS_AUTOCHECKBOX as u32,
                    14,
                    52,
                    400,
                    20,
                    IDC_CHK_TRAY,
                );
                set_font(st.hwnd_chk_tray, hfont);
                st.hwnd_chk_notif = child(
                    hwnd,
                    w!("BUTTON"),
                    Some("Уведомления Windows о новых письмах"),
                    WS_TABSTOP.0 | BS_AUTOCHECKBOX as u32,
                    14,
                    78,
                    400,
                    20,
                    IDC_CHK_NOTIF,
                );
                set_font(st.hwnd_chk_notif, hfont);
                st.hwnd_chk_autostart = child(
                    hwnd,
                    w!("BUTTON"),
                    Some("Запускать при входе в Windows"),
                    WS_TABSTOP.0 | BS_AUTOCHECKBOX as u32,
                    14,
                    104,
                    400,
                    20,
                    IDC_CHK_AUTOSTART,
                );
                set_font(st.hwnd_chk_autostart, hfont);

                // Автопроверка (наш ключ; таймер — N07).
                let h = child(
                    hwnd,
                    w!("STATIC"),
                    Some("Проверять почту каждые, минут (0 — вручную):"),
                    0,
                    14,
                    143,
                    310,
                    20,
                    -1,
                );
                set_font(h, hfont);
                st.hwnd_autosync = child(
                    hwnd,
                    w!("EDIT"),
                    None,
                    WS_TABSTOP.0 | ES_AUTOHSCROLL as u32,
                    344,
                    140,
                    70,
                    24,
                    IDC_AUTOSYNC,
                );
                set_font(st.hwnd_autosync, hfont);

                // Ключ GigaChat.
                let h = child(
                    hwnd,
                    w!("STATIC"),
                    Some("Ключ GigaChat (ИИ-помощник, Фаза 3):"),
                    0,
                    14,
                    177,
                    190,
                    20,
                    -1,
                );
                set_font(h, hfont);
                st.hwnd_giga_key = child(
                    hwnd,
                    w!("EDIT"),
                    None,
                    WS_TABSTOP.0 | ES_AUTOHSCROLL as u32,
                    204,
                    174,
                    240,
                    24,
                    IDC_GIGA_KEY,
                );
                set_font(st.hwnd_giga_key, hfont);
                st.hwnd_keyinfo = child(
                    hwnd,
                    w!("STATIC"),
                    Some(""),
                    0,
                    14,
                    206,
                    460,
                    34,
                    IDC_KEYINFO,
                );
                set_font(st.hwnd_keyinfo, hfont);

                // Кнопки.
                st.hwnd_btn_giga_clear = child(
                    hwnd,
                    w!("BUTTON"),
                    Some("Убрать ключ"),
                    WS_TABSTOP.0,
                    14,
                    300,
                    120,
                    30,
                    IDC_BTN_GIGA_CLEAR,
                );
                set_font(st.hwnd_btn_giga_clear, hfont);
                st.hwnd_btn_save = child(
                    hwnd,
                    w!("BUTTON"),
                    Some("Сохранить"),
                    WS_TABSTOP.0,
                    260,
                    300,
                    120,
                    30,
                    IDC_BTN_SAVE,
                );
                set_font(st.hwnd_btn_save, hfont);
                st.hwnd_btn_cancel = child(
                    hwnd,
                    w!("BUTTON"),
                    Some("Отмена"),
                    WS_TABSTOP.0,
                    390,
                    300,
                    100,
                    30,
                    IDC_BTN_CANCEL,
                );
                set_font(st.hwnd_btn_cancel, hfont);
                st.hwnd_status = child(
                    hwnd,
                    w!("STATIC"),
                    Some(""),
                    0,
                    14,
                    330,
                    460,
                    20,
                    IDC_STATUS,
                );
                set_font(st.hwnd_status, hfont);

                fill_from_settings(st);
                layout(hwnd, st);
                let _ = SetFocus(Some(st.hwnd_theme));
            }
            LRESULT(0)
        }
        WM_SIZE => {
            if let Some(st) = unsafe { st_of(hwnd) } {
                unsafe { layout(hwnd, st) };
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_COMMAND => {
            let id = (wparam.0 & 0xFFFF) as i32;
            let code = ((wparam.0 >> 16) & 0xFFFF) as u32;
            if lparam.0 != 0 && code == BN_CLICKED {
                match id {
                    IDC_BTN_GIGA_CLEAR => {
                        unsafe { on_giga_clear(hwnd) };
                        return LRESULT(0);
                    }
                    IDC_BTN_SAVE => {
                        unsafe { on_save(hwnd) };
                        return LRESULT(0);
                    }
                    IDC_BTN_CANCEL => {
                        let _ = unsafe { DestroyWindow(hwnd) };
                        return LRESULT(0);
                    }
                    _ => {}
                }
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_DESTROY => {
            if let Some(st) = unsafe { st_of(hwnd) } {
                unsafe {
                    let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                        Some(st.parent),
                        WM_APP_SETTINGS_CLOSED,
                        WPARAM(0),
                        LPARAM(0),
                    );
                    windows::Win32::UI::WindowsAndMessaging::SetWindowLongPtrW(
                        hwnd,
                        GWLP_USERDATA,
                        0,
                    );
                    drop(Box::from_raw(st as *mut DlgState));
                }
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Заполнить поля текущими значениями (settings.json + gigachat.json).
unsafe fn fill_from_settings(st: &mut DlgState) {
    let view = settings::native_view();
    unsafe {
        // Тема: 0 — тёмная, 1 — светлая.
        let _ = SendMessageW(
            st.hwnd_theme,
            CB_SETCURSEL,
            Some(WPARAM(if view.dark { 0 } else { 1 })),
            Some(LPARAM(0)),
        );
        check(st.hwnd_chk_tray, view.minimize_to_tray);
        check(st.hwnd_chk_notif, view.notifications);
        check(st.hwnd_chk_autostart, view.autostart);
        let _ = set_window_text(st.hwnd_autosync, &view.autosync_min.to_string());
        let key = gigachat::file_key();
        st.had_file_key = key.is_some();
        if let Some(k) = key {
            let _ = set_window_text(st.hwnd_giga_key, &k);
        }
        let _ = set_window_text(st.hwnd_keyinfo, &key_info_text());
    }
}

/// Галочка кнопки-флажка.
unsafe fn check(hwnd: HWND, on: bool) {
    unsafe {
        SendMessageW(
            hwnd,
            BM_SETCHECK,
            Some(WPARAM(
                if on { BST_CHECKED.0 } else { BST_UNCHECKED.0 } as usize
            )),
            Some(LPARAM(0)),
        );
    }
}

/// Прочитать галочку.
unsafe fn is_checked(hwnd: HWND) -> bool {
    let r = unsafe { SendMessageW(hwnd, BM_GETCHECK, Some(WPARAM(0)), Some(LPARAM(0))) };
    r.0 == BST_CHECKED.0 as isize
}

/// «Убрать ключ»: очистить поле (само удаление — при «Сохранить»).
unsafe fn on_giga_clear(hwnd: HWND) {
    let Some(st) = (unsafe { st_of(hwnd) }) else {
        return;
    };
    unsafe {
        let _ = set_window_text(st.hwnd_giga_key, "");
        let _ = set_window_text(
            st.hwnd_keyinfo,
            "Файловый ключ будет удалён при «Сохранить» (env-ключ не трогаем).",
        );
        let _ = SetFocus(Some(st.hwnd_giga_key));
    }
}

/// «Сохранить»: наши ключи + ui.* в settings.json, ключ — в gigachat.json.
unsafe fn on_save(hwnd: HWND) {
    let Some(st) = (unsafe { st_of(hwnd) }) else {
        return;
    };
    // Интервал автопроверки.
    let autosync_raw = read_text(st.hwnd_autosync);
    let Some(autosync_min) = autosync_raw.trim().parse::<u32>().ok() else {
        unsafe {
            MessageBoxW(
                Some(hwnd),
                w!("Интервал автопроверки — целое число минут от 0 до 1440."),
                w!("POS Mail"),
                MB_OK | MB_ICONERROR,
            );
        }
        return;
    };
    if autosync_min > settings::AUTOSYNC_MAX {
        unsafe {
            MessageBoxW(
                Some(hwnd),
                w!("Интервал автопроверки — не больше 1440 минут (сутки)."),
                w!("POS Mail"),
                MB_OK | MB_ICONERROR,
            );
        }
        return;
    }
    let view = NativeView {
        dark: unsafe {
            SendMessageW(
                st.hwnd_theme,
                CB_GETCURSEL,
                Some(WPARAM(0)),
                Some(LPARAM(0)),
            )
            .0 != 1
        },
        autosync_min,
        minimize_to_tray: unsafe { is_checked(st.hwnd_chk_tray) },
        notifications: unsafe { is_checked(st.hwnd_chk_notif) },
        autostart: unsafe { is_checked(st.hwnd_chk_autostart) },
    };

    // Ключ GigaChat: изменили — записать, очистили (а был) — удалить файл.
    let key = read_text(st.hwnd_giga_key);
    let key = key.trim();
    if !key.is_empty() {
        let old = gigachat::file_key();
        if old.as_deref() != Some(key)
            && let Err(e) = gigachat::save_api_key(key)
        {
            let _ = set_window_text(st.hwnd_status, &format!("ключ не сохранён: {e}"));
            return;
        }
    } else if st.had_file_key
        && let Err(e) = gigachat::clear_api_key()
    {
        let _ = set_window_text(st.hwnd_status, &format!("ключ не удалён: {e}"));
        return;
    }

    settings::store_native(&view);
    unsafe {
        MessageBoxW(
            Some(hwnd),
            w!("Настройки сохранены (settings.json общий с Soroka)."),
            w!("POS Mail"),
            MB_OK | MB_ICONINFORMATION,
        );
        let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
            Some(st.parent),
            WM_APP_SETTINGS_SAVED,
            WPARAM(0),
            LPARAM(0),
        );
        let _ = DestroyWindow(hwnd);
    }
}
