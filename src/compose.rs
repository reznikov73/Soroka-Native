//! Окно написания письма: Кому/Копия/Тема, тело (multiline Edit),
//! вложения (GetOpenFileNameW, мультивыбор), отправка через бэкенд.
//!
//! Отдельный класс окна «SorokaComposeWnd» — системные контролы, ноль DOM.
//! Результат отправки приходит сообщением WM_APP_SEND_RESULT из рабочего потока.

use std::sync::mpsc::Sender;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{DEFAULT_GUI_FONT, HGDIOBJ};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::Dialogs::{
    GetOpenFileNameW, OFN_ALLOWMULTISELECT, OFN_EXPLORER, OFN_FILEMUSTEXIST, OFN_HIDEREADONLY,
    OFN_PATHMUSTEXIST, OPENFILENAMEW,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, SetFocus};
use windows::Win32::UI::WindowsAndMessaging::{
    BN_CLICKED, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow, ES_AUTOHSCROLL,
    ES_AUTOVSCROLL, ES_MULTILINE, ES_WANTRETURN, GWLP_USERDATA, GetClientRect, GetWindowLongPtrW,
    IDC_ARROW, KillTimer, LB_ADDSTRING, LB_GETCURSEL, LB_RESETCONTENT, LBS_NOTIFY, LoadCursorW,
    MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MB_YESNO, MessageBoxW, RegisterClassW, SetTimer,
    WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLOSE, WM_COMMAND, WM_CREATE, WM_DESTROY, WM_NCCREATE,
    WM_SETFONT, WM_SIZE, WM_TIMER, WNDCLASSW, WS_CHILD, WS_OVERLAPPEDWINDOW, WS_TABSTOP,
    WS_VISIBLE, WS_VSCROLL,
};
use windows::core::{PCWSTR, w};

use crate::backend::{DraftSaved, Job, SendOutcome, WM_APP_DRAFT_SAVED, WM_APP_SEND_RESULT};
use crate::mail::smtp::OutgoingAttachment;
use crate::state::AppState;
use crate::wide::{read_text, set_window_text, wide};

/// Главное окно узнаёт о закрытии редактора.
pub const WM_APP_COMPOSE_CLOSED: u32 = crate::backend::WM_APP_LOGIN_RESULT + 1;

const WINDOW_CLASS: PCWSTR = w!("SorokaComposeWnd");

/// Идентификаторы контролов редактора.
const IDC_TO: i32 = 200;
const IDC_CC: i32 = 201;
const IDC_SUBJECT: i32 = 202;
const IDC_BODY: i32 = 203;
const IDC_ATTACH_LIST: i32 = 204;
const IDC_BTN_ATTACH: i32 = 205;
const IDC_BTN_REMOVE: i32 = 206;
const IDC_BTN_SEND: i32 = 207;
const IDC_BTN_CANCEL: i32 = 208;
const IDC_STATUS: i32 = 209;
const IDC_BTN_DRAFT: i32 = 210;

/// Автосохранение черновика (порт startDraftAutosave: 20 с, TASK-022).
const IDT_DRAFT_AUTOSAVE: usize = 3;

/// Предзаполнение редактора (ответ/пересылка/черновик — TASK-N08).
pub struct ComposePrefill {
    pub to: String,
    pub cc: String,
    pub subject: String,
    pub body: String,
    /// Вложения редактируемого черновика (продолжение с того же места).
    pub atts: Vec<OutgoingAttachment>,
    /// Идентификатор существующего черновика: пересохраняем ту же запись,
    /// отправка уберет её с сервера. None — новое письмо/ответ/пересылка.
    pub draft_id: Option<String>,
}

/// Состояние окна редактора (GWLP_USERDATA).
struct ComposeState {
    parent: HWND,
    backend: Sender<Job>,
    account_id: String,
    atts: Vec<OutgoingAttachment>,
    hwnd_to: HWND,
    hwnd_cc: HWND,
    hwnd_subject: HWND,
    hwnd_body: HWND,
    hwnd_attach_list: HWND,
    hwnd_btn_send: HWND,
    hwnd_btn_cancel: HWND,
    hwnd_btn_draft: HWND,
    hwnd_status: HWND,
    sending: bool,
    /// Идентификатор черновика этого окна (TASK-N08): одна запись на всё
    /// время редактирования, пересохранения заменяют серверную копию.
    draft_id: String,
    /// Черновик уже сохранён (кнопкой/автосохранением/он был открыт).
    draft_saved: bool,
    /// Отпечаток последнего сохранённого содержимого (не изменилось —
    /// не пересохраняем: каждое сохранение это IMAP APPEND + удаление).
    last_print: String,
    /// Окно закрывается и ждёт результата сохранения черновика.
    closing_saving: bool,
}

/// Открывает редактор (один на приложение). Возвращает HWND нового окна.
pub fn open(hwnd_main: HWND, st: &AppState, prefill: Option<ComposePrefill>) -> Option<HWND> {
    if let Some(h) = st.hwnd_compose {
        let _ = unsafe { SetFocus(Some(h)) };
        return None;
    }
    let Some(backend) = &st.backend else {
        return None;
    };
    let backend_tx = backend.sender();
    let account_id = st
        .current_account
        .clone()
        .or_else(|| st.accounts.first().map(|a| a.id.clone()))
        .unwrap_or_default();
    // Черновик: открыт существующий — продолжаем ту же запись; новое
    // письмо — свежий идентификатор (порт newDraftId, TASK-N08).
    let draft_id = prefill
        .as_ref()
        .and_then(|p| p.draft_id.clone())
        .unwrap_or_else(crate::draft::new_draft_id);
    let settled = prefill
        .as_ref()
        .and_then(|p| p.draft_id.as_deref())
        .is_some();

    let hmod = unsafe { GetModuleHandleW(None) }.expect("GetModuleHandleW");
    let wc = WNDCLASSW {
        lpfnWndProc: Some(wndproc),
        hInstance: windows::Win32::Foundation::HINSTANCE(hmod.0),
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW) }.expect("LoadCursorW"),
        lpszClassName: WINDOW_CLASS,
        ..Default::default()
    };
    unsafe { RegisterClassW(&wc) };

    let state = Box::new(ComposeState {
        parent: hwnd_main,
        backend: backend_tx,
        account_id,
        atts: Vec::new(),
        hwnd_to: HWND::default(),
        hwnd_cc: HWND::default(),
        hwnd_subject: HWND::default(),
        hwnd_body: HWND::default(),
        hwnd_attach_list: HWND::default(),
        hwnd_btn_send: HWND::default(),
        hwnd_btn_cancel: HWND::default(),
        hwnd_btn_draft: HWND::default(),
        hwnd_status: HWND::default(),
        sending: false,
        draft_id,
        draft_saved: settled,
        last_print: String::new(),
        closing_saving: false,
    });
    let raw = Box::into_raw(state);

    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            WINDOW_CLASS,
            w!("Написать письмо — POS Mail"),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            780,
            620,
            Some(hwnd_main),
            None,
            Some(windows::Win32::Foundation::HINSTANCE(hmod.0)),
            Some(raw as *const std::ffi::c_void),
        )
    }
    .expect("создать окно редактора");

    if let Some(p) = prefill {
        unsafe {
            let st = &mut *raw;
            let _ = set_text(st.hwnd_to, &p.to);
            let _ = set_text(st.hwnd_cc, &p.cc);
            let _ = set_text(st.hwnd_subject, &p.subject);
            let _ = set_text(st.hwnd_body, &p.body);
            st.atts = p.atts;
            // Открыт существующий черновик: содержимое уже сохранено —
            // автосохранение не должно пересохранять его тут же.
            if settled {
                let att_meta: Vec<(&str, u64)> = st
                    .atts
                    .iter()
                    .map(|a| (a.name.as_str(), a.data.len() as u64))
                    .collect();
                st.last_print =
                    crate::draft::fingerprint(&p.to, &p.cc, &p.subject, &p.body, &att_meta);
            }
        }
    }

    unsafe {
        // Геометрия из layout.json (TASK-N06): нормальный прямоугольник до
        // показа; развёрнутость — в ShowWindow.
        let maximized = crate::layout_store::load()
            .and_then(|l| l.compose)
            .map(|p| crate::ui::restore_placement(hwnd, &p))
            .unwrap_or(false);
        let _ = windows::Win32::UI::WindowsAndMessaging::ShowWindow(
            hwnd,
            if maximized {
                windows::Win32::UI::WindowsAndMessaging::SW_SHOWMAXIMIZED
            } else {
                windows::Win32::UI::WindowsAndMessaging::SW_SHOW
            },
        );
        let _ = SetFocus(Some((*raw).hwnd_subject));
    }
    Some(hwnd)
}

fn set_text(hwnd: HWND, text: &str) -> windows::core::Result<()> {
    set_window_text(hwnd, text)
}

unsafe fn st_of(hwnd: HWND) -> Option<&'static mut ComposeState> {
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut ComposeState;
    if ptr.is_null() {
        None
    } else {
        Some(unsafe { &mut *ptr })
    }
}

/// Создание дочернего контрола (координаты уточнит layout()).
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
            // и разбирает нажатия (IDC_BTN_ATTACH, IDC_TO и т.д.).
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

/// Раскладка: поля сверху, список вложений, тело, кнопки снизу.
unsafe fn layout(hwnd: HWND, st: &ComposeState) {
    let mut rc = RECT::default();
    if unsafe { GetClientRect(hwnd, &mut rc) }.is_err() {
        return;
    }
    let w = rc.right;
    let h = rc.bottom;
    let m = 12;
    let field_x = m + 90;
    let field_w = (w - field_x - m).max(140);
    let list_y = 110;
    let list_h = 64;
    let btn_y = list_y + list_h + 8;
    let body_y = btn_y + 38;
    let bottom_h = 48;

    unsafe {
        move_window(st.hwnd_to, field_x, 14, field_w, 24);
        move_window(st.hwnd_cc, field_x, 46, field_w, 24);
        move_window(st.hwnd_subject, field_x, 78, field_w, 24);
        move_window(st.hwnd_attach_list, m, list_y, w - 2 * m, list_h);
        move_window(
            st.hwnd_body,
            m,
            body_y,
            w - 2 * m,
            (h - body_y - bottom_h).max(60),
        );
        move_window(st.hwnd_btn_send, w - m - 130, h - 38, 130, 30);
        move_window(st.hwnd_btn_cancel, w - m - 250, h - 38, 110, 30);
        move_window(st.hwnd_btn_draft, w - m - 370, h - 38, 110, 30);
        move_window(st.hwnd_status, m, h - 30, (w - 2 * m - 390).max(120), 24);
    }
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
                // Подписи.
                for (text, y) in [("Кому:", 17), ("Копия:", 49), ("Тема:", 81)] {
                    let h = child(hwnd, w!("STATIC"), Some(text), 0, 12, y, 84, 20, -1);
                    set_font(h, hfont);
                }
                // Поля.
                let edit_style = WS_TABSTOP.0 | ES_AUTOHSCROLL as u32;
                st.hwnd_to = child(hwnd, w!("EDIT"), None, edit_style, 102, 14, 400, 24, IDC_TO);
                st.hwnd_cc = child(hwnd, w!("EDIT"), None, edit_style, 102, 46, 400, 24, IDC_CC);
                st.hwnd_subject = child(
                    hwnd,
                    w!("EDIT"),
                    None,
                    edit_style,
                    102,
                    78,
                    400,
                    24,
                    IDC_SUBJECT,
                );
                for h in [st.hwnd_to, st.hwnd_cc, st.hwnd_subject] {
                    set_font(h, hfont);
                }
                // Вложения.
                st.hwnd_attach_list = child(
                    hwnd,
                    w!("LISTBOX"),
                    None,
                    LBS_NOTIFY as u32 | WS_VSCROLL.0,
                    12,
                    110,
                    400,
                    64,
                    IDC_ATTACH_LIST,
                );
                set_font(st.hwnd_attach_list, hfont);
                let b_attach = child(
                    hwnd,
                    w!("BUTTON"),
                    Some("Прикрепить файлы..."),
                    WS_TABSTOP.0,
                    12,
                    182,
                    170,
                    28,
                    IDC_BTN_ATTACH,
                );
                let b_remove = child(
                    hwnd,
                    w!("BUTTON"),
                    Some("Убрать выбранное"),
                    WS_TABSTOP.0,
                    190,
                    182,
                    150,
                    28,
                    IDC_BTN_REMOVE,
                );
                set_font(b_attach, hfont);
                set_font(b_remove, hfont);
                // «В черновики» (TASK-N08): сохранить не отправляя.
                let b_draft = child(
                    hwnd,
                    w!("BUTTON"),
                    Some("В черновики"),
                    WS_TABSTOP.0,
                    12,
                    214,
                    130,
                    28,
                    IDC_BTN_DRAFT,
                );
                set_font(b_draft, hfont);
                // Тело письма.
                st.hwnd_body = child(
                    hwnd,
                    w!("EDIT"),
                    None,
                    WS_TABSTOP.0
                        | ES_MULTILINE as u32
                        | ES_WANTRETURN as u32
                        | ES_AUTOVSCROLL as u32
                        | WS_VSCROLL.0,
                    12,
                    220,
                    400,
                    120,
                    IDC_BODY,
                );
                set_font(st.hwnd_body, hfont);
                // Кнопки отправки/отмены и статус.
                st.hwnd_btn_send = child(
                    hwnd,
                    w!("BUTTON"),
                    Some("Отправить"),
                    WS_TABSTOP.0,
                    12,
                    12,
                    130,
                    30,
                    IDC_BTN_SEND,
                );
                st.hwnd_btn_cancel = child(
                    hwnd,
                    w!("BUTTON"),
                    Some("Закрыть"),
                    WS_TABSTOP.0,
                    150,
                    12,
                    110,
                    30,
                    IDC_BTN_CANCEL,
                );
                set_font(st.hwnd_btn_send, hfont);
                set_font(st.hwnd_btn_cancel, hfont);
                st.hwnd_btn_draft = b_draft;
                st.hwnd_status = child(
                    hwnd,
                    w!("STATIC"),
                    Some(""),
                    0,
                    12,
                    240,
                    300,
                    20,
                    IDC_STATUS,
                );
                set_font(st.hwnd_status, hfont);
                layout(hwnd, st);
            }
            // Автосохранение черновика каждые 20 с, пока окно открыто
            // (порт startDraftAutosave TASK-022; таймер живёт в этом окне).
            let _ = unsafe { SetTimer(Some(hwnd), IDT_DRAFT_AUTOSAVE, 20_000, None) };
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
                    IDC_BTN_ATTACH => {
                        unsafe { on_attach(hwnd) };
                        return LRESULT(0);
                    }
                    IDC_BTN_REMOVE => {
                        unsafe { on_remove(hwnd) };
                        return LRESULT(0);
                    }
                    IDC_BTN_SEND => {
                        unsafe { on_send(hwnd) };
                        return LRESULT(0);
                    }
                    IDC_BTN_DRAFT => {
                        unsafe { save_draft_flow(hwnd, true) };
                        return LRESULT(0);
                    }
                    IDC_BTN_CANCEL => {
                        // «Закрыть» — через WM_CLOSE: черновик сохраняется
                        // (TASK-N08), а не выбрасывается.
                        let _ = unsafe {
                            windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                                Some(hwnd),
                                WM_CLOSE,
                                WPARAM(0),
                                LPARAM(0),
                            )
                        };
                        return LRESULT(0);
                    }
                    _ => {}
                }
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_CLOSE => {
            // Крестик/«Закрыть»/Alt+F4 (TASK-N08, порт closeCompose(true)):
            // изменившееся — сохраняем в черновики и закрываемся по ответу;
            // пустое и неотредактированное — просто закрываем. Отправка в
            // пути — закрываем сразу: черновик приберёт сама отправка.
            let sending = unsafe { st_of(hwnd) }.is_some_and(|st| st.sending);
            let posted = !sending && unsafe { save_draft_flow(hwnd, false) };
            if posted {
                if let Some(st) = unsafe { st_of(hwnd) } {
                    st.closing_saving = true;
                    let _ = set_text(st.hwnd_status, "сохраняю черновик...");
                }
            } else {
                let _ = unsafe { DestroyWindow(hwnd) };
            }
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == IDT_DRAFT_AUTOSAVE => {
            // Тик автосохранения (20 с): изменилось с прошлого раза —
            // ставим задачу, нет — ничего не делаем (порт saveDraftNow(true)).
            let _ = unsafe { save_draft_flow(hwnd, false) };
            LRESULT(0)
        }
        WM_APP_DRAFT_SAVED => {
            // Ответ рабочего потока на сохранение черновика (TASK-N08).
            let outcome = unsafe { Box::from_raw(lparam.0 as *mut DraftSaved) };
            let Some(st) = (unsafe { st_of(hwnd) }) else {
                return LRESULT(0);
            };
            if outcome.ok {
                st.draft_saved = true;
                st.last_print = outcome.print.clone();
                let text = if outcome.msg.is_empty() {
                    outcome.label.clone()
                } else {
                    format!("{} ({})", outcome.label, outcome.msg)
                };
                let _ = set_text(st.hwnd_status, &text);
                if st.closing_saving {
                    let _ = unsafe { DestroyWindow(hwnd) };
                }
            } else if st.closing_saving {
                // Не сохранилось и мы закрывались — не теряем текст молча:
                // спросить, закрывать ли без сохранения.
                st.closing_saving = false;
                let text = wide(&format!(
                    "Черновик не сохранился: {}\n\nЗакрыть окно без сохранения?",
                    outcome.msg
                ));
                let answer = unsafe {
                    MessageBoxW(
                        Some(hwnd),
                        PCWSTR(text.as_ptr()),
                        w!("POS Mail"),
                        MB_YESNO | MB_ICONERROR,
                    )
                };
                if answer == windows::Win32::UI::WindowsAndMessaging::IDYES {
                    let _ = unsafe { DestroyWindow(hwnd) };
                } else {
                    let _ = set_text(
                        st.hwnd_status,
                        &format!("черновик не сохранился: {}", outcome.msg),
                    );
                }
            } else {
                let _ = set_text(
                    st.hwnd_status,
                    &format!("черновик не сохранился: {}", outcome.msg),
                );
            }
            LRESULT(0)
        }
        WM_APP_SEND_RESULT => {
            let Some(st) = (unsafe { st_of(hwnd) }) else {
                return LRESULT(0);
            };
            let outcome = unsafe { Box::from_raw(lparam.0 as *mut SendOutcome) };
            if outcome.ok {
                unsafe {
                    MessageBoxW(
                        Some(hwnd),
                        w!("Письмо отправлено."),
                        w!("POS Mail"),
                        MB_OK | MB_ICONINFORMATION,
                    );
                    let _ = DestroyWindow(hwnd);
                }
            } else {
                st.sending = false;
                unsafe {
                    let _ = EnableWindow(st.hwnd_btn_send, true);
                    let _ = set_text(st.hwnd_status, &format!("не отправлено: {}", outcome.msg));
                }
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            // Таймер автосохранения гаснет вместе с окном (TASK-N08).
            let _ = unsafe { KillTimer(Some(hwnd), IDT_DRAFT_AUTOSAVE) };
            // Геометрия окна — в layout.json (TASK-N06).
            crate::layout_store::store_window(
                crate::layout_store::WinKind::Compose,
                crate::ui::snapshot_placement(hwnd),
            );
            if let Some(st) = unsafe { st_of(hwnd) } {
                unsafe {
                    let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                        Some(st.parent),
                        WM_APP_COMPOSE_CLOSED,
                        WPARAM(0),
                        LPARAM(0),
                    );
                    windows::Win32::UI::WindowsAndMessaging::SetWindowLongPtrW(
                        hwnd,
                        GWLP_USERDATA,
                        0,
                    );
                    drop(Box::from_raw(st as *mut ComposeState));
                }
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Диалог выбора вложений (мультивыбор) → список окна.
unsafe fn on_attach(hwnd: HWND) {
    let Some(st) = (unsafe { st_of(hwnd) }) else {
        return;
    };
    let mut buf = vec![0u16; 32768];
    let filter = wide("Все файлы\0*.*\0");
    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: hwnd,
        lpstrFilter: PCWSTR(filter.as_ptr()),
        lpstrFile: windows::core::PWSTR(buf.as_mut_ptr()),
        nMaxFile: buf.len() as u32,
        Flags: OFN_ALLOWMULTISELECT
            | OFN_EXPLORER
            | OFN_FILEMUSTEXIST
            | OFN_PATHMUSTEXIST
            | OFN_HIDEREADONLY,
        ..Default::default()
    };
    if !(unsafe { GetOpenFileNameW(&mut ofn) }).as_bool() {
        return;
    }
    // «каталог\\0файл1\\0файл2\\0\\0» либо одиночный «полный путь\\0».
    let mut parts: Vec<String> = Vec::new();
    let mut cur: Vec<u16> = Vec::new();
    for &u in &buf {
        if u == 0 {
            if cur.is_empty() {
                break;
            }
            parts.push(String::from_utf16_lossy(&cur));
            cur.clear();
        } else {
            cur.push(u);
        }
    }
    let paths: Vec<std::path::PathBuf> = if parts.len() == 1 {
        vec![std::path::PathBuf::from(&parts[0])]
    } else {
        let dir = std::path::PathBuf::from(&parts[0]);
        parts[1..].iter().map(|name| dir.join(name)).collect()
    };
    for path in paths {
        let Ok(data) = std::fs::read(&path) else {
            continue;
        };
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "файл".into());
        st.atts.push(OutgoingAttachment {
            mime: mime_of(&name),
            name,
            data,
        });
    }
    refresh_attach_list(st);
    let _ = set_text(
        st.hwnd_status,
        &format!("прикреплено файлов: {}", st.atts.len()),
    );
}

/// MIME-тип по расширению (для исходящих вложений).
fn mime_of(name: &str) -> String {
    let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
    match ext.as_str() {
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "zip" => "application/zip",
        "rar" => "application/vnd.rar",
        "7z" => "application/x-7z-compressed",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "txt" => "text/plain",
        "htm" | "html" => "text/html",
        "csv" => "text/csv",
        "json" => "application/json",
        "eml" => "message/rfc822",
        _ => "application/octet-stream",
    }
    .to_string()
}

/// Убрать выделенное вложение.
unsafe fn on_remove(hwnd: HWND) {
    let Some(st) = (unsafe { st_of(hwnd) }) else {
        return;
    };
    let sel = unsafe {
        windows::Win32::UI::WindowsAndMessaging::SendMessageW(
            st.hwnd_attach_list,
            LB_GETCURSEL,
            Some(WPARAM(0)),
            Some(LPARAM(0)),
        )
    }
    .0 as i32;
    if sel < 0 || sel as usize >= st.atts.len() {
        return;
    }
    st.atts.remove(sel as usize);
    refresh_attach_list(st);
}

/// Перерисовать список вложений.
fn refresh_attach_list(st: &ComposeState) {
    unsafe {
        windows::Win32::UI::WindowsAndMessaging::SendMessageW(
            st.hwnd_attach_list,
            LB_RESETCONTENT,
            Some(WPARAM(0)),
            Some(LPARAM(0)),
        );
    }
    for a in &st.atts {
        let mut label = wide(&format!("{} — {} КБ", a.name, a.data.len() / 1024));
        unsafe {
            windows::Win32::UI::WindowsAndMessaging::SendMessageW(
                st.hwnd_attach_list,
                LB_ADDSTRING,
                Some(WPARAM(0)),
                Some(LPARAM(label.as_mut_ptr() as isize)),
            );
        }
    }
}

/// «Отправить»: собрать поля → Job::SendMail в рабочий поток.
unsafe fn on_send(hwnd: HWND) {
    let Some(st) = (unsafe { st_of(hwnd) }) else {
        return;
    };
    if st.sending {
        return;
    }
    let to = read_text(st.hwnd_to);
    let cc = read_text(st.hwnd_cc);
    let subject = read_text(st.hwnd_subject);
    let body = read_text(st.hwnd_body);
    if to.trim().is_empty() {
        unsafe {
            MessageBoxW(
                Some(hwnd),
                w!("Укажите получателя."),
                w!("POS Mail"),
                MB_OK | MB_ICONERROR,
            );
        }
        let _ = unsafe { SetFocus(Some(st.hwnd_to)) };
        return;
    }
    st.sending = true;
    unsafe {
        let _ = EnableWindow(st.hwnd_btn_send, false);
        let _ = set_text(st.hwnd_status, "отправка...");
    }
    // Черновик этого окна (если сохранялся) отправка уберёт сама
    // (draft_id у задачи; TASK-N08).
    let draft_id = st.draft_saved.then(|| st.draft_id.clone());
    st.backend
        .send(Job::SendMail {
            hwnd_reply: hwnd,
            account_id: st.account_id.clone(),
            to,
            cc,
            subject,
            body,
            atts: st.atts.clone(),
            draft_id,
        })
        .ok();
}

/// Сохранить черновик, если есть что и с последнего раза изменилось (порт
/// saveDraftNow, TASK-022/N08): кнопка «В черновики», автотаймер 20 с,
/// закрытие окна. Возвращает true, если в рабочий поток ушла новая задача
/// сохранения (закрывающееся окно дождётся ответа перед DestroyWindow).
unsafe fn save_draft_flow(hwnd: HWND, manual: bool) -> bool {
    let Some(st) = (unsafe { st_of(hwnd) }) else {
        return false;
    };
    if st.sending {
        return false;
    }
    let to = read_text(st.hwnd_to);
    let cc = read_text(st.hwnd_cc);
    let subject = read_text(st.hwnd_subject);
    let body = read_text(st.hwnd_body);
    // Пустой черновик не сохраняем: нечего (прежняя серверная копия,
    // если была, остаётся в папке — удаляется оттуда, как обычное письмо).
    if crate::draft::is_empty(&to, &cc, &subject, &body, st.atts.len()) {
        if manual && st.draft_saved {
            let _ = set_text(
                st.hwnd_status,
                "черновик пуст — прежняя копия остаётся в папке",
            );
        }
        return false;
    }
    let att_meta: Vec<(&str, u64)> = st
        .atts
        .iter()
        .map(|a| (a.name.as_str(), a.data.len() as u64))
        .collect();
    let print = crate::draft::fingerprint(&to, &cc, &subject, &body, &att_meta);
    if st.draft_saved && print == st.last_print {
        if manual {
            let _ = set_text(st.hwnd_status, "черновик уже сохранён");
        }
        return false;
    }
    st.backend
        .send(Job::SaveDraft {
            hwnd_reply: hwnd,
            account_id: st.account_id.clone(),
            draft_id: st.draft_id.clone(),
            to,
            cc,
            subject,
            body,
            atts: st.atts.clone(),
            print,
        })
        .ok();
    let _ = set_text(st.hwnd_status, "сохраняю черновик...");
    true
}
