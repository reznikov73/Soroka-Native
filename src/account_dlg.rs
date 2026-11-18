//! Окно ящика: добавление учётной записи (email, пароль, IMAP/SMTP),
//! проверка соединения (Job::TestLogin), сохранение в accounts.json.
//!
//! accounts.json общий с Soroka (%APPDATA%\Soroka\accounts.json) — ящики,
//! настроенные в старом клиенте, видны сразу.

use std::sync::mpsc::Sender;

use windows::core::{PCWSTR, w};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{DEFAULT_GUI_FONT, HGDIOBJ};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, SetFocus};
use windows::Win32::UI::WindowsAndMessaging::{
    BN_CLICKED, CreateWindowExW, CW_USEDEFAULT, DefWindowProcW, DestroyWindow,
    ES_AUTOHSCROLL, ES_PASSWORD, GWLP_USERDATA, GetClientRect, GetWindowLongPtrW, IDC_ARROW,
    LoadCursorW, MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MessageBoxW, WM_COMMAND,
    WM_CREATE, WM_DESTROY, WM_NCCREATE, WM_SETFONT, WM_SIZE, WINDOW_EX_STYLE, WINDOW_STYLE,
    WNDCLASSW, WS_CHILD, WS_OVERLAPPEDWINDOW, WS_TABSTOP, WS_VISIBLE, RegisterClassW,
};

use crate::backend::{Job, LoginOutcome, WM_APP_LOGIN_RESULT};
use crate::state::AppState;
use crate::wide::{read_text, set_window_text, wide};

/// Главное окно перестраивает дерево после сохранения ящика.
pub const WM_APP_ACCOUNT_SAVED: u32 = crate::compose::WM_APP_COMPOSE_CLOSED + 1;
/// Главное окно чистит st.hwnd_account при закрытии окна ящика.
pub const WM_APP_ACCOUNT_CLOSED: u32 = WM_APP_ACCOUNT_SAVED + 1;

const WINDOW_CLASS: PCWSTR = w!("SorokaAccountWnd");

const IDC_EMAIL: i32 = 300;
const IDC_PASS: i32 = 301;
const IDC_IMAP_HOST: i32 = 302;
const IDC_IMAP_PORT: i32 = 303;
const IDC_SMTP_HOST: i32 = 304;
const IDC_SMTP_PORT: i32 = 305;
const IDC_BTN_TEST: i32 = 306;
const IDC_BTN_SAVE: i32 = 307;
const IDC_BTN_CANCEL: i32 = 308;
const IDC_STATUS: i32 = 309;

/// Состояние окна ящика.
struct AccountState {
    parent: HWND,
    backend: Sender<Job>,
    hwnd_email: HWND,
    hwnd_pass: HWND,
    hwnd_imap_host: HWND,
    hwnd_imap_port: HWND,
    hwnd_smtp_host: HWND,
    hwnd_smtp_port: HWND,
    hwnd_btn_test: HWND,
    hwnd_btn_save: HWND,
    hwnd_btn_cancel: HWND,
    hwnd_status: HWND,
    testing: bool,
}

/// Открывает окно добавления ящика (одно на приложение). Возвращает HWND.
pub fn open(hwnd_main: HWND, st: &AppState) -> Option<HWND> {
    if let Some(h) = st.hwnd_account {
        let _ = unsafe { SetFocus(Some(h)) };
        return None;
    }
    let Some(backend) = &st.backend else {
        return None;
    };
    let backend_tx = backend.sender();

    let hmod = unsafe { GetModuleHandleW(None) }.expect("GetModuleHandleW");
    let wc = WNDCLASSW {
        lpfnWndProc: Some(wndproc),
        hInstance: windows::Win32::Foundation::HINSTANCE(hmod.0),
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW) }.expect("LoadCursorW"),
        lpszClassName: WINDOW_CLASS,
        ..Default::default()
    };
    unsafe { RegisterClassW(&wc) };

    let state = Box::new(AccountState {
        parent: hwnd_main,
        backend: backend_tx,
        hwnd_email: HWND::default(),
        hwnd_pass: HWND::default(),
        hwnd_imap_host: HWND::default(),
        hwnd_imap_port: HWND::default(),
        hwnd_smtp_host: HWND::default(),
        hwnd_smtp_port: HWND::default(),
        hwnd_btn_test: HWND::default(),
        hwnd_btn_save: HWND::default(),
        hwnd_btn_cancel: HWND::default(),
        hwnd_status: HWND::default(),
        testing: false,
    });
    let raw = Box::into_raw(state);

    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            WINDOW_CLASS,
            w!("Добавить ящик — POS Mail"),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            560,
            420,
            Some(hwnd_main),
            None,
            Some(windows::Win32::Foundation::HINSTANCE(hmod.0)),
            Some(raw as *const std::ffi::c_void),
        )
        .expect("создать окно ящика")
    };
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::ShowWindow(
            hwnd,
            windows::Win32::UI::WindowsAndMessaging::SW_SHOW,
        );
    }
    Some(hwnd)
}

unsafe fn st_of(hwnd: HWND) -> Option<&'static mut AccountState> {
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut AccountState;
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
            // и разбирает нажатия (IDC_EMAIL, IDC_IMAP_HOST и т.д.).
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
        windows::Win32::UI::WindowsAndMessaging::SendMessageW(
            hwnd,
            WM_SETFONT,
            Some(WPARAM(font.0 as usize)),
            Some(LPARAM(1)),
        );
    }
}

unsafe fn set_text(hwnd: HWND, text: &str) -> windows::core::Result<()> {
    set_window_text(hwnd, text)
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

unsafe fn layout(hwnd: HWND, st: &AccountState) {
    let mut rc = RECT::default();
    if unsafe { GetClientRect(hwnd, &mut rc) }.is_err() {
        return;
    }
    let w = rc.right;
    let h = rc.bottom;
    let m = 14;
    unsafe {
        move_window(st.hwnd_email, m + 130, 16, (w - m * 2 - 130).max(160), 24);
        move_window(st.hwnd_pass, m + 130, 48, (w - m * 2 - 130).max(160), 24);
        move_window(st.hwnd_imap_host, m + 130, 80, (w - m * 2 - 130).max(160), 24);
        move_window(st.hwnd_imap_port, m + 130, 112, 110, 24);
        move_window(st.hwnd_smtp_host, m + 130, 144, (w - m * 2 - 130).max(160), 24);
        move_window(st.hwnd_smtp_port, m + 130, 176, 110, 24);
        move_window(st.hwnd_btn_test, m, h - 44, 190, 30);
        move_window(st.hwnd_btn_save, m + 210, h - 44, 130, 30);
        move_window(st.hwnd_btn_cancel, m + 350, h - 44, 100, 30);
        move_window(st.hwnd_status, m, h - 74, (w - m * 2).max(200), 24);
    }
}

unsafe extern "system" fn wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_NCCREATE => {
            let cs = unsafe {
                &*(lparam.0
                    as *const windows::Win32::UI::WindowsAndMessaging::CREATESTRUCTW)
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
            let edit_style = WS_TABSTOP.0 | ES_AUTOHSCROLL as u32;
            unsafe {
                let fields: [(&str, i32, i32, bool); 6] = [
                    ("Email:", 16, IDC_EMAIL, false),
                    ("Пароль:", 48, IDC_PASS, true),
                    ("IMAP-сервер:", 80, IDC_IMAP_HOST, false),
                    ("IMAP-порт:", 112, IDC_IMAP_PORT, false),
                    ("SMTP-сервер:", 144, IDC_SMTP_HOST, false),
                    ("SMTP-порт:", 176, IDC_SMTP_PORT, false),
                ];
                for (label, y, id, is_pass) in fields {
                    let h = child(
                        hwnd,
                        w!("STATIC"),
                        Some(label),
                        0,
                        14,
                        y + 3,
                        118,
                        20,
                        -1,
                    );
                    set_font(h, hfont);
                    let (class, style, text) = if is_pass {
                        (w!("EDIT"), WS_TABSTOP.0 | ES_AUTOHSCROLL as u32 | ES_PASSWORD as u32, None)
                    } else {
                        (w!("EDIT"), edit_style, None)
                    };
                    let h = child(hwnd, class, text, style, 144, y, 200, 24, id);
                    set_font(h, hfont);
                    match id {
                        IDC_EMAIL => st.hwnd_email = h,
                        IDC_PASS => st.hwnd_pass = h,
                        IDC_IMAP_HOST => st.hwnd_imap_host = h,
                        IDC_IMAP_PORT => st.hwnd_imap_port = h,
                        IDC_SMTP_HOST => st.hwnd_smtp_host = h,
                        IDC_SMTP_PORT => st.hwnd_smtp_port = h,
                        _ => {}
                    }
                }
                // Порты по умолчанию.
                let _ = set_text(st.hwnd_imap_port, "993");
                let _ = set_text(st.hwnd_smtp_port, "465");

                st.hwnd_btn_test = child(
                    hwnd,
                    w!("BUTTON"),
                    Some("Проверить соединение"),
                    WS_TABSTOP.0,
                    14,
                    220,
                    190,
                    28,
                    IDC_BTN_TEST,
                );
                set_font(st.hwnd_btn_test, hfont);
                st.hwnd_btn_save = child(
                    hwnd,
                    w!("BUTTON"),
                    Some("Сохранить"),
                    WS_TABSTOP.0,
                    14,
                    250,
                    130,
                    30,
                    IDC_BTN_SAVE,
                );
                st.hwnd_btn_cancel = child(
                    hwnd,
                    w!("BUTTON"),
                    Some("Отмена"),
                    WS_TABSTOP.0,
                    154,
                    250,
                    100,
                    30,
                    IDC_BTN_CANCEL,
                );
                set_font(st.hwnd_btn_save, hfont);
                set_font(st.hwnd_btn_cancel, hfont);
                st.hwnd_status = child(
                    hwnd,
                    w!("STATIC"),
                    Some(""),
                    0,
                    14,
                    290,
                    300,
                    20,
                    IDC_STATUS,
                );
                set_font(st.hwnd_status, hfont);
                layout(hwnd, st);
                let _ = SetFocus(Some(st.hwnd_email));
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
                    IDC_BTN_TEST => {
                        unsafe { on_test(hwnd) };
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
        WM_APP_LOGIN_RESULT => {
            let Some(st) = (unsafe { st_of(hwnd) }) else {
                return LRESULT(0);
            };
            let outcome = unsafe { Box::from_raw(lparam.0 as *mut LoginOutcome) };
            st.testing = false;
            unsafe {
                let _ = EnableWindow(st.hwnd_btn_test, true);
                let mark = if outcome.ok { "OK: " } else { "ошибка: " };
                let _ = set_text(st.hwnd_status, &format!("{mark}{}", outcome.msg));
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            if let Some(st) = unsafe { st_of(hwnd) } {
                unsafe {
                    let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                        Some(st.parent),
                        WM_APP_ACCOUNT_CLOSED,
                        WPARAM(0),
                        LPARAM(0),
                    );
                    windows::Win32::UI::WindowsAndMessaging::SetWindowLongPtrW(
                        hwnd,
                        GWLP_USERDATA,
                        0,
                    );
                    drop(Box::from_raw(st as *mut AccountState));
                }
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Домен из email.
fn email_domain(email: &str) -> String {
    email
        .rsplit('@')
        .next()
        .unwrap_or("")
        .trim()
        .to_lowercase()
}

/// IMAP-хост по домену; SMTP — из IMAP («imap.X» → «smtp.X»).
fn default_hosts(email: &str, imap_host: &str) -> (String, String) {
    let domain = email_domain(email);
    let imap = if imap_host.trim().is_empty() {
        if domain.is_empty() {
            String::new()
        } else {
            format!("imap.{domain}")
        }
    } else {
        imap_host.trim().to_string()
    };
    let smtp = match imap.strip_prefix("imap.") {
        Some(rest) if !rest.is_empty() => format!("smtp.{rest}"),
        _ => {
            if domain.is_empty() {
                String::new()
            } else {
                format!("smtp.{domain}")
            }
        }
    };
    (imap, smtp)
}

/// Считать поля формы в (email, пароль, imap_host, imap_port, smtp_host, smtp_port).
unsafe fn read_form(st: &AccountState) -> Form {
    let email = read_text(st.hwnd_email);
    let password = read_text(st.hwnd_pass);
    let imap_host_raw = read_text(st.hwnd_imap_host);
    let smtp_host_raw = read_text(st.hwnd_smtp_host);
    let (imap_host, smtp_host) = default_hosts(&email, &imap_host_raw);
    let _ = smtp_host_raw;
    let imap_port = read_text(st.hwnd_imap_port).trim().parse::<u16>().unwrap_or(993);
    let smtp_port = read_text(st.hwnd_smtp_port).trim().parse::<u16>().unwrap_or(465);
    Form {
        email,
        password,
        imap_host,
        imap_port,
        smtp_host,
        smtp_port,
    }
}

struct Form {
    email: String,
    password: String,
    imap_host: String,
    imap_port: u16,
    smtp_host: String,
    smtp_port: u16,
}

/// «Проверить соединение» → Job::TestLogin.
unsafe fn on_test(hwnd: HWND) {
    let Some(st) = (unsafe { st_of(hwnd) }) else {
        return;
    };
    if st.testing {
        return;
    }
    let form = unsafe { read_form(st) };
    if form.email.trim().is_empty() || form.password.is_empty() {
        unsafe {
            MessageBoxW(
                Some(hwnd),
                w!("Укажите email и пароль."),
                w!("POS Mail"),
                MB_OK | MB_ICONERROR,
            );
        }
        return;
    }
    if form.imap_host.is_empty() {
        unsafe {
            MessageBoxW(
                Some(hwnd),
                w!("Не удалось определить IMAP-сервер: укажите его вручную."),
                w!("POS Mail"),
                MB_OK | MB_ICONERROR,
            );
        }
        return;
    }
    st.testing = true;
    unsafe {
        let _ = EnableWindow(st.hwnd_btn_test, false);
        let _ = set_text(st.hwnd_status, "проверяю соединение...");
    }
    st.backend
        .send(Job::TestLogin {
            hwnd_reply: hwnd,
            host: form.imap_host,
            port: form.imap_port,
            user: form.email,
            password: form.password,
        })
        .ok();
}

/// «Сохранить»: upsert в accounts.json → дерево главного окна.
unsafe fn on_save(hwnd: HWND) {
    let Some(st) = (unsafe { st_of(hwnd) }) else {
        return;
    };
    let form = unsafe { read_form(st) };
    if form.email.trim().is_empty() || form.password.is_empty() {
        unsafe {
            MessageBoxW(
                Some(hwnd),
                w!("Укажите email и пароль."),
                w!("POS Mail"),
                MB_OK | MB_ICONERROR,
            );
        }
        return;
    }
    if form.imap_host.is_empty() {
        unsafe {
            MessageBoxW(
                Some(hwnd),
                w!("Не удалось определить IMAP-сервер: укажите его вручную."),
                w!("POS Mail"),
                MB_OK | MB_ICONERROR,
            );
        }
        return;
    }
    match crate::mail::accounts::upsert_account(
        None,
        &form.email,
        Some(&form.password),
        &form.imap_host,
        form.imap_port,
        &form.smtp_host,
        form.smtp_port,
        0,     // автопроверка — Фаза 2
        false, // чистка корзины при выходе — Фаза 2
    ) {
        Ok(_) => {
            unsafe {
                MessageBoxW(
                    Some(hwnd),
                    w!("Ящик сохранён."),
                    w!("POS Mail"),
                    MB_OK | MB_ICONINFORMATION,
                );
                let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                    Some(st.parent),
                    WM_APP_ACCOUNT_SAVED,
                    WPARAM(0),
                    LPARAM(0),
                );
                let _ = DestroyWindow(hwnd);
            }
        }
        Err(e) => {
            unsafe {
                let _ = set_text(st.hwnd_status, &format!("не сохранено: {e}"));
            }
        }
    }
}
