//! POS Mail Native — Фаза 1: почтовый каркас (TASK-N02).
//!
//! Нативная оболочка Win32 (Фаза 0, TASK-N01) подключена к почтовому ядру
//! Soroka: ящики из accounts.json, папки из кэша SQLite, реальные письма
//! в виртуальном списке, тело письма в WebView2-панели, написание/ответ
//! отдельным окном, отправка SMTP, вложения, очередь исходящих.
//!
//! Архитектура ресайза не изменилась: раскладку ведёт WM_SIZE через
//! SetWindowPos (микросекунды), WebView2 обновляется bounds'ом — DOM в
//! кадре ресайза по-прежнему нет.
//!
//! Код под windows 0.62 / webview2-com 0.39 (версии wry 0.57). Если при
//! сборке сигнатуры не совпадут — править минимально (docs/WORKFLOW.md).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod account_dlg;
mod ai;
mod backend;
mod compose;
// Ядро Soroka перенесено дословно (правило WORKFLOW: src/mail/ не редактировать).
// Часть его API (контакты, IDLE, пресеты провайдеров) подключается в Фазах 2–3 —
// до тех пор dead_code на модуле неинформативен. Снять allow вместе с подключением
// соответствующих API (TASK-N02, v0.2.2).
// needless_borrow — то же самое: два места в imap.rs (726–727), ядро не правим.
#[allow(dead_code, clippy::needless_borrow)]
mod mail;
mod state;
mod ui;
mod wide;

use std::sync::mpsc;

use webview2_com::{
    CoreWebView2EnvironmentOptions, CreateCoreWebView2ControllerCompletedHandler,
    CreateCoreWebView2EnvironmentCompletedHandler, WebMessageReceivedEventHandler,
    Microsoft::Web::WebView2::Win32::*, wait_with_pump,
};
use windows::core::{Error, HSTRING, PCWSTR, Result, w};
use windows::Win32::Foundation::{
    E_POINTER, E_UNEXPECTED, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, PAINTSTRUCT};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{
    ICC_BAR_CLASSES, ICC_LISTVIEW_CLASSES, ICC_TREEVIEW_CLASSES, INITCOMMONCONTROLSEX,
    InitCommonControlsEx,
    LVN_GETDISPINFOW, LVS_OWNERDATA, LVS_REPORT, LVS_SHOWSELALWAYS, NMHDR, NMLVDISPINFOW,
    NMLISTVIEW, NMTREEVIEWW, NM_CUSTOMDRAW, TVN_SELCHANGEDW, WC_LISTVIEW, WC_TREEVIEW,
};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetFocus, SetFocus, VK_ESCAPE, VK_RETURN};
use windows::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CreateWindowExW,
    DefWindowProcW, DestroyWindow, DispatchMessageW, ES_AUTOHSCROLL, GA_ROOT, GetAncestor,
    GetClientRect, GetMessageW, GetParent, GWLP_USERDATA, GWLP_WNDPROC, GetWindowLongPtrW,
    IDC_ARROW, LoadCursorW, MB_ICONINFORMATION, MB_OK, MB_YESNO, MINMAXINFO, MSG, MessageBoxW,
    PostMessageW, PostQuitMessage, RegisterClassW, SWP_NOZORDER, SW_SHOW, SendMessageW,
    SetTimer, SetWindowLongPtrW, SetWindowPos, ShowWindow, TranslateAcceleratorW, TranslateMessage,
    WINDOW_EX_STYLE, WINDOW_STYLE, WM_CHAR, WM_COMMAND, WM_DESTROY, WM_GETMINMAXINFO,
    WM_KEYDOWN, WM_NCCREATE, WM_NOTIFY, WM_PAINT, WM_SETFONT, WM_SIZE, WM_TIMER, WNDCLASSW,
    WNDPROC, WS_CHILD, WS_EX_CLIENTEDGE, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
};

use account_dlg::{WM_APP_ACCOUNT_CLOSED, WM_APP_ACCOUNT_SAVED};
use backend::{Backend, Job, WM_APP_MAIL, WM_APP_SEND_RESULT};
use compose::{ComposePrefill, WM_APP_COMPOSE_CLOSED};
use state::AppState;
use ui::{
    EN_CHANGE, IDT_SEARCH, PANE_W, SEARCH_H, STATUS_H, TREE_W, apply_mail_event, create_fonts,
    create_menu, empty_pane_html, enter_search, exit_search, fire_search, on_get_disp_info,
    on_list_custom_draw, on_list_item_changed, on_tree_select, rebuild_tree, setup_list_columns,
    status_text, IDM_ACCOUNT_ADD, IDM_COMPOSE, IDM_DELETE, IDM_EXIT, IDM_FORWARD, IDM_REPLY,
    IDM_REPLY_ALL, IDM_SEARCH_CLOSE, IDM_SEARCH_FOCUS, IDM_SEARCH_GO, IDM_SYNC, IDM_TOGGLE_SEEN,
};

const WINDOW_CLASS: PCWSTR = w!("SorokaNativeWnd");
const STATUSBAR_CLASS: PCWSTR = w!("msctls_statusbar32");
/// EM_SETCUEBANNER (0x1501): подсказка в пустом EDIT; wparam=1 — показывать и в фокусе.
const EM_SETCUEBANNER: u32 = 0x1501;

/// Клик по вложению в панели письма (WebView2 → оболочка): lparam = индекс.
const WM_APP_ATTACH_CLICK: u32 = WM_APP_ACCOUNT_CLOSED + 1;

/// Раскладка панелей (как в Фазе 0, но с учетом статуса, поиска и новой ширины панели).
unsafe fn layout(hwnd: HWND, st: &AppState) {
    let mut rc = RECT::default();
    if unsafe { GetClientRect(hwnd, &mut rc) }.is_err() {
        return;
    }
    let w = rc.right;
    let h = rc.bottom;
    let h_main = (h - STATUS_H).max(0);
    // Поле поиска (TASK-N03) занимает полосу над списком.
    let search_open = st.search_query.is_some();
    let list_top = if search_open { SEARCH_H } else { 0 };
    let list_w = (w - TREE_W - PANE_W).max(160);
    unsafe {
        if !st.hwnd_tree.0.is_null() {
            let _ = SetWindowPos(st.hwnd_tree, None, 0, 0, TREE_W, h_main, SWP_NOZORDER);
        }
        if search_open && !st.hwnd_search.0.is_null() {
            let _ = SetWindowPos(
                st.hwnd_search,
                None,
                TREE_W,
                2,
                list_w,
                SEARCH_H - 4,
                SWP_NOZORDER,
            );
        }
        if !st.hwnd_list.0.is_null() {
            let _ = SetWindowPos(
                st.hwnd_list,
                None,
                TREE_W,
                list_top,
                list_w,
                (h_main - list_top).max(0),
                SWP_NOZORDER,
            );
        }
        if !st.hwnd_status.0.is_null() && h > STATUS_H {
            let _ =
                SetWindowPos(st.hwnd_status, None, 0, h - STATUS_H, w, STATUS_H, SWP_NOZORDER);
        }
        if let Some(ctrl) = &st.controller {
            let left = (w - PANE_W).max(TREE_W + 160);
            let _ = ctrl.SetBounds(RECT {
                left,
                top: 0,
                right: left + PANE_W,
                bottom: h_main,
            });
        }
    }
}

/// `webview2-com` 0.39 отдаёт собственный `Error`; приводим вручную.
fn wv_err(err: webview2_com::Error) -> Error {
    match err {
        webview2_com::Error::WindowsError(e) => e,
        other => Error::new(E_UNEXPECTED, other.to_string()),
    }
}

/// Создание WebView2 + приём кликов по вложениям (паттерн wry 0.57).
unsafe fn create_webview(parent: HWND) -> Result<(ICoreWebView2Controller, ICoreWebView2)> {
    let (tx, rx) = mpsc::channel();
    let env_handler = CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(
        move |error_code: Result<()>, environment: Option<ICoreWebView2Environment>| -> Result<()> {
            let env: Result<ICoreWebView2Environment> = (|| {
                error_code?;
                environment.ok_or_else(|| Error::from(E_POINTER))
            })();
            tx.send(env).map_err(|_| Error::from(E_UNEXPECTED))
        },
    ));
    let options = CoreWebView2EnvironmentOptions::default();
    unsafe {
        CreateCoreWebView2EnvironmentWithOptions(
            PCWSTR::null(), // браузер Evergreen, установленный в системе
            PCWSTR::null(), // каталог данных по умолчанию
            &ICoreWebView2EnvironmentOptions::from(options),
            &env_handler,
        )?;
    }
    let env = wait_with_pump(rx).map_err(wv_err)??;

    let (tx2, rx2) = mpsc::channel();
    let ctrl_handler = CreateCoreWebView2ControllerCompletedHandler::create(Box::new(
        move |error_code: Result<()>, controller: Option<ICoreWebView2Controller>| -> Result<()> {
            let ctrl: Result<ICoreWebView2Controller> = (|| {
                error_code?;
                controller.ok_or_else(|| Error::from(E_POINTER))
            })();
            tx2.send(ctrl).map_err(|_| Error::from(E_UNEXPECTED))
        },
    ));
    unsafe { env.CreateCoreWebView2Controller(parent, &ctrl_handler)? };
    let controller = wait_with_pump(rx2).map_err(wv_err)??;
    let webview = unsafe { controller.CoreWebView2()? };
    // Контроллер приходит скрытым — без явного показа панель останется чёрной
    // (проверено на 157.0.4341.46 в Фазе 0).
    unsafe { controller.SetIsVisible(true)? };

    // Клик по вложению в HTML → postMessage("attach:N") → задача бэкенду.
    // Замыкание живёт в COM-обработчике; держим клон канала и HWND родителя.
    // Токен — обычный i64 (паттерн wry 0.57 под webview2-com 0.39).
    let mut token: i64 = 0;
    unsafe {
        webview.add_WebMessageReceived(
            &WebMessageReceivedEventHandler::create(Box::new(
                move |_sender, args| -> Result<()> {
                    let Some(args) = args else {
                        return Ok(());
                    };
                    let mut text = windows::core::PWSTR::null();
                    args.TryGetWebMessageAsString(&mut text)?;
                    if text.is_null() {
                        return Ok(());
                    }
                    // PWSTR → String вручную: не зависим от вспомогательных API.
                    let mut len = 0usize;
                    while *text.0.add(len) != 0 {
                        len += 1;
                    }
                    let units = std::slice::from_raw_parts(text.0, len);
                    let msg = String::from_utf16_lossy(units);
                    if let Some(idx) = msg
                        .strip_prefix("attach:")
                        .and_then(|n| n.trim().parse::<u32>().ok())
                    {
                        let _ = PostMessageW(
                            Some(parent),
                            WM_APP_ATTACH_CLICK,
                            WPARAM(0),
                            LPARAM(idx as isize),
                        );
                    }
                    Ok(())
                },
            )),
            &mut token,
        )?;
    }

    Ok((controller, webview))
}

/// Цитата исходного письма для ответа/пересылки.
fn quote_body(hdr: &mail::imap::MessageHeader, pane: Option<&state::PaneBody>) -> String {
    let plain = match pane {
        Some(p) => p
            .text
            .clone()
            .unwrap_or_else(|| {
                p.html.as_deref().map(mail::body::html_to_text).unwrap_or_default()
            }),
        None => String::new(),
    };
    let mut cut = plain;
    if cut.len() > 20_000 {
        // Усечение по границе символа — String::truncate паникует посреди UTF-8.
        let mut end = 20_000;
        while end > 0 && !cut.is_char_boundary(end) {
            end -= 1;
        }
        cut.truncate(end);
        cut.push_str("\r\n…(обрезано)");
    }
    // Многострочный EDIT в Win32 разделяет строки только по CRLF: с одиночным
    // \n вся цитата ложится одной длинной строкой.
    let quoted: String = cut.lines().map(|l| format!("> {l}\r\n")).collect();
    format!(
        "\r\n\r\n----- Исходное письмо -----\r\nОт: {}\r\nКому: {}\r\nТема: {}\r\nДата: {}\r\n\r\n{}",
        hdr.from, hdr.to, hdr.subject, hdr.date, quoted
    )
}

/// Ответ/ответ всем: поля редактора из выбранного письма.
fn reply_prefill(st: &AppState, all: bool) -> Option<ComposePrefill> {
    let uid = st.selected_uid?;
    let row = st.rows.iter().find(|m| m.hdr.uid == uid)?;
    let hdr = &row.hdr;
    let cc = if all {
        let mut cc = hdr.to.clone();
        if !hdr.cc.is_empty() {
            if !cc.is_empty() {
                cc.push_str(", ");
            }
            cc.push_str(&hdr.cc);
        }
        cc
    } else {
        String::new()
    };
    let subject = if hdr.subject.to_lowercase().starts_with("re:") {
        hdr.subject.clone()
    } else {
        format!("Re: {}", hdr.subject)
    };
    Some(ComposePrefill {
        to: hdr.from.clone(),
        cc,
        subject,
        body: quote_body(hdr, st.pane_body.as_ref()),
    })
}

/// Пересылка.
fn forward_prefill(st: &AppState) -> Option<ComposePrefill> {
    let uid = st.selected_uid?;
    let row = st.rows.iter().find(|m| m.hdr.uid == uid)?;
    let hdr = &row.hdr;
    let subject = if hdr.subject.to_lowercase().starts_with("fwd:") {
        hdr.subject.clone()
    } else {
        format!("Fwd: {}", hdr.subject)
    };
    Some(ComposePrefill {
        to: String::new(),
        cc: String::new(),
        subject,
        body: quote_body(hdr, st.pane_body.as_ref()),
    })
}

/// Выбранное письмо (для удаления/переключения флага).
fn selected_header(st: &AppState) -> Option<mail::imap::MessageHeader> {
    let uid = st.selected_uid?;
    st.rows.iter().find(|m| m.hdr.uid == uid).map(|r| r.hdr.clone())
}

/// Сабкласс поля поиска (TASK-N03): Enter — искать, Esc — вернуться к папке.
/// Прочее (набор, стрелки, буфер обмена) — штатная процедура EDIT;
/// исходная процедура хранится в GWLP_USERDATA поля.
unsafe extern "system" fn search_edit_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_KEYDOWN => {
            let key = wparam.0 as u16;
            if key == VK_RETURN.0 || key == VK_ESCAPE.0 {
                let cmd = if key == VK_RETURN.0 { IDM_SEARCH_GO } else { IDM_SEARCH_CLOSE };
                // GetParent в 0.62 возвращает Result — при ошибке просто молчим.
                let parent = unsafe { GetParent(hwnd) }.unwrap_or_default();
                let _ = unsafe {
                    PostMessageW(Some(parent), WM_COMMAND, WPARAM(cmd as usize), LPARAM(0))
                };
                return LRESULT(0);
            }
        }
        WM_CHAR => {
            // Enter/Esc не должны пищать в однострочном EDIT.
            let ch = wparam.0 as u16;
            if ch == 0x0D || ch == 0x1B {
                return LRESULT(0);
            }
        }
        _ => {}
    }
    let orig = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) };
    let proc: WNDPROC = unsafe { std::mem::transmute::<isize, WNDPROC>(orig) };
    unsafe { CallWindowProcW(proc, hwnd, msg, wparam, lparam) }
}

unsafe extern "system" fn wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_NCCREATE => {
            let cs = unsafe { &*(lparam.0 as *const CREATESTRUCTW) };
            unsafe {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_GETMINMAXINFO => {
            let mmi = unsafe { &mut *(lparam.0 as *mut MINMAXINFO) };
            mmi.ptMinTrackSize = POINT { x: 1000, y: 620 };
            LRESULT(0)
        }
        WM_SIZE => {
            let st = unsafe { &mut *(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppState) };
            unsafe { layout(hwnd, st) };
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_COMMAND => {
            let st = unsafe { &mut *(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppState) };
            let id = (wparam.0 & 0xFFFF) as u16;
            let code = ((wparam.0 >> 16) & 0xFFFF) as u32;
            // Живой поиск: ввод в поле — отложенный запуск (debounce, как в Soroka).
            if lparam.0 == st.hwnd_search.0 as isize && code == EN_CHANGE {
                let _ = unsafe { SetTimer(Some(hwnd), IDT_SEARCH, 300, None) };
                return LRESULT(0);
            }
            // Команды меню (lparam == 0); от контролов — в их собственных wndproc.
            if lparam.0 != 0 {
                return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
            }
            match id {
                IDM_SEARCH_FOCUS => {
                    enter_search(st.hwnd_list, st);
                    unsafe { layout(hwnd, st) };
                }
                IDM_SEARCH_GO => {
                    fire_search(hwnd, st.hwnd_list, st);
                    // Пустой запрос мог закрыть поиск — полоса над списком исчезает.
                    unsafe { layout(hwnd, st) };
                }
                IDM_SEARCH_CLOSE => {
                    exit_search(hwnd, st.hwnd_list, st);
                    unsafe { layout(hwnd, st) };
                }
                IDM_EXIT => {
                    let _ = unsafe { DestroyWindow(hwnd) };
                }
                IDM_ACCOUNT_ADD => {
                    if let Some(h) = account_dlg::open(hwnd, st) {
                        st.hwnd_account = Some(h);
                    }
                }
                IDM_SYNC => {
                    if let Some(key) = st.current_folder.clone() {
                        status_text(st, "синхронизация...");
                        if let Some(b) = &st.backend {
                            b.post(Job::SyncFolder { key });
                        }
                    } else {
                        status_text(st, "сначала выберите папку");
                    }
                }
                IDM_COMPOSE => {
                    if st.accounts.is_empty() {
                        unsafe {
                            MessageBoxW(
                                Some(hwnd),
                                w!("Сначала добавьте почтовый ящик."),
                                w!("POS Mail"),
                                MB_OK | MB_ICONINFORMATION,
                            );
                        }
                        if let Some(h) = account_dlg::open(hwnd, st) {
                            st.hwnd_account = Some(h);
                        }
                    } else if let Some(h) = compose::open(hwnd, st, None) {
                        st.hwnd_compose = Some(h);
                    }
                }
                IDM_REPLY | IDM_REPLY_ALL => {
                    match reply_prefill(st, id == IDM_REPLY_ALL) {
                        Some(p) => {
                            if let Some(h) = compose::open(hwnd, st, Some(p)) {
                                st.hwnd_compose = Some(h);
                            }
                        }
                        None => status_text(st, "сначала выберите письмо"),
                    }
                }
                IDM_FORWARD => match forward_prefill(st) {
                    Some(p) => {
                        if let Some(h) = compose::open(hwnd, st, Some(p)) {
                            st.hwnd_compose = Some(h);
                        }
                    }
                    None => status_text(st, "сначала выберите письмо"),
                },
                IDM_DELETE => {
                    let Some(hdr) = selected_header(st) else {
                        status_text(st, "сначала выберите письмо");
                        return LRESULT(0);
                    };
                    let answer = unsafe {
                        MessageBoxW(
                            Some(hwnd),
                            w!("Удалить письмо (перенести в корзину сервера)?"),
                            w!("POS Mail"),
                            MB_YESNO | MB_ICONINFORMATION,
                        )
                    };
                    if answer == windows::Win32::UI::WindowsAndMessaging::IDYES {
                        let key = st.current_folder.clone().unwrap_or_default();
                        if !key.is_empty() && let Some(b) = &st.backend {
                            b.post(Job::DeleteMessage { key, uid: hdr.uid });
                        }
                    }
                }
                IDM_TOGGLE_SEEN => {
                    let Some(hdr) = selected_header(st) else {
                        status_text(st, "сначала выберите письмо");
                        return LRESULT(0);
                    };
                    let key = st.current_folder.clone().unwrap_or_default();
                    if !key.is_empty() && let Some(b) = &st.backend {
                        b.post(Job::SetSeen {
                            key,
                            uid: hdr.uid,
                            seen: !hdr.seen,
                        });
                    }
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_NOTIFY => {
            let hdr = unsafe { &*(lparam.0 as *const NMHDR) };
            let st = unsafe { &mut *(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppState) };
            if hdr.hwndFrom == st.hwnd_list {
                match hdr.code {
                    LVN_GETDISPINFOW => {
                        let nm = unsafe { &mut *(lparam.0 as *mut NMLVDISPINFOW) };
                        on_get_disp_info(st, nm);
                        return LRESULT(0);
                    }
                    NM_CUSTOMDRAW => {
                        let nm = unsafe { &*(lparam.0 as *const windows::Win32::UI::Controls::NMCUSTOMDRAW) };
                        return on_list_custom_draw(st, nm);
                    }
                    windows::Win32::UI::Controls::LVN_ITEMCHANGED => {
                        let nm = unsafe { &*(lparam.0 as *const NMLISTVIEW) };
                        on_list_item_changed(st, nm);
                        return LRESULT(0);
                    }
                    _ => {}
                }
            } else if hdr.hwndFrom == st.hwnd_tree && hdr.code == TVN_SELCHANGEDW {
                let nm = unsafe { &*(lparam.0 as *const NMTREEVIEWW) };
                // Выбор папки мог закрыть поиск — тогда список сдвигается вверх.
                let was_search = st.search_query.is_some();
                on_tree_select(st.hwnd_list, st, nm);
                if was_search && st.search_query.is_none() {
                    unsafe { layout(hwnd, st) };
                }
                return LRESULT(0);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_APP_MAIL => {
            let st = unsafe { &mut *(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppState) };
            if let Some(b) = &st.backend {
                for ev in b.drain() {
                    apply_mail_event(hwnd, st.hwnd_list, st.hwnd_tree, st, ev);
                }
            }
            LRESULT(0)
        }
        WM_APP_ATTACH_CLICK => {
            let st = unsafe { &mut *(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppState) };
            let index = lparam.0 as u32;
            let Some(uid) = st.selected_uid else {
                return LRESULT(0);
            };
            let Some(key) = st.current_folder.clone() else {
                return LRESULT(0);
            };
            if let Some(b) = &st.backend {
                b.post(Job::SaveAttachment { key, uid, index });
                status_text(st, "сохраняю вложение...");
            }
            LRESULT(0)
        }
        WM_APP_COMPOSE_CLOSED => {
            let st = unsafe { &mut *(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppState) };
            st.hwnd_compose = None;
            LRESULT(0)
        }
        WM_APP_ACCOUNT_SAVED => {
            let st = unsafe { &mut *(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppState) };
            rebuild_tree(st.hwnd_tree, st);
            LRESULT(0)
        }
        WM_APP_ACCOUNT_CLOSED => {
            let st = unsafe { &mut *(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppState) };
            st.hwnd_account = None;
            LRESULT(0)
        }
        WM_APP_SEND_RESULT => {
            // Приходит только окну редактора; сюда не должен попадать.
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == IDT_SEARCH => {
            // Сработал отложенный живой поиск (EN_CHANGE + 300 мс).
            let st = unsafe { &mut *(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppState) };
            if st.search_query.is_some() {
                fire_search(hwnd, st.hwnd_list, st);
                unsafe { layout(hwnd, st) };
            }
            LRESULT(0)
        }
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            unsafe {
                BeginPaint(hwnd, &mut ps);
                let _ = EndPaint(hwnd, &ps);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn run() -> Result<()> {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
    }
    unsafe {
        let _ = InitCommonControlsEx(&INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_LISTVIEW_CLASSES | ICC_TREEVIEW_CLASSES | ICC_BAR_CLASSES,
        });
    }

    let (font, font_bold) = create_fonts();

    let hmod = unsafe { GetModuleHandleW(None)? };
    let wc = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(wndproc),
        hInstance: windows::Win32::Foundation::HINSTANCE(hmod.0),
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW)? },
        lpszClassName: WINDOW_CLASS,
        ..Default::default()
    };
    if unsafe { RegisterClassW(&wc) } == 0 {
        return Err(Error::from(E_UNEXPECTED));
    }

    // Состояние уходит в GWLP_USERDATA через CREATESTRUCTW (WM_NCCREATE).
    let state = Box::into_raw(Box::new(AppState::new(font, font_bold)));
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            WINDOW_CLASS,
            w!("POS Mail — Фаза 1: почтовый каркас"),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1440,
            900,
            None,
            None,
            Some(windows::Win32::Foundation::HINSTANCE(hmod.0)),
            Some(state as *const _),
        )?
    };
    let st = unsafe { &mut *state };

    // --- дети: дерево, список, статус-бар ---
    let child = |class: PCWSTR, style_extra: WINDOW_STYLE| -> Result<HWND> {
        unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class,
                None,
                WS_CHILD | WS_VISIBLE | style_extra,
                0,
                0,
                100,
                100,
                Some(hwnd),
                None,
                Some(windows::Win32::Foundation::HINSTANCE(hmod.0)),
                None,
            )
        }
    };
    st.hwnd_tree = child(WC_TREEVIEW, WINDOW_STYLE(0))?;
    st.hwnd_list = child(
        WC_LISTVIEW,
        WINDOW_STYLE(LVS_REPORT | LVS_OWNERDATA | LVS_SHOWSELALWAYS),
    )?;
    st.hwnd_status = child(STATUSBAR_CLASS, WINDOW_STYLE(0))?;

    // Поле поиска (TASK-N03): скрытая строка над списком, появляется по «/».
    st.hwnd_search = unsafe {
        CreateWindowExW(
            WS_EX_CLIENTEDGE,
            w!("EDIT"),
            None,
            WS_CHILD | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
            TREE_W,
            2,
            400,
            SEARCH_H - 4,
            Some(hwnd),
            None,
            Some(windows::Win32::Foundation::HINSTANCE(hmod.0)),
            None,
        )?
    };

    // Шрифт интерфейса всем детям (системный, подобранный под DPI).
    for h in [st.hwnd_tree, st.hwnd_list, st.hwnd_status, st.hwnd_search] {
        unsafe {
            SendMessageW(h, WM_SETFONT, Some(WPARAM(st.font.0 as usize)), Some(LPARAM(1)));
        }
    }

    unsafe {
        // Подсказка в пустом поле (EM_SETCUEBANNER: показывать и в фокусе).
        let cue = wide::wide("Поиск по всем папкам ящика — Enter: искать, Esc: к папке");
        SendMessageW(
            st.hwnd_search,
            EM_SETCUEBANNER,
            Some(WPARAM(1)),
            Some(LPARAM(cue.as_ptr() as isize)),
        );
        // Сабкласс: Enter/Esc уходят командами главному окну; исходная
        // процедура EDIT хранится в GWLP_USERDATA поля.
        let orig = SetWindowLongPtrW(
            st.hwnd_search,
            GWLP_WNDPROC,
            search_edit_proc as *const () as isize,
        );
        SetWindowLongPtrW(st.hwnd_search, GWLP_USERDATA, orig);
    }

    setup_list_columns(st.hwnd_list);

    // Меню + ускорители.
    let haccel = create_menu(hwnd)?;

    // Панель письма: WebView2 (единственный движок в оболочке).
    let backend = Backend::spawn(hwnd);
    let (controller, webview) = unsafe { create_webview(hwnd)? };
    st.controller = Some(controller);
    st.webview = Some(webview);
    if let Some(wv) = &st.webview {
        let _ = unsafe { wv.NavigateToString(&HSTRING::from(empty_pane_html())) };
    }
    st.backend = Some(backend);

    // Дерево ящиков/папок + выбор первого INBOX (загрузка из кэша — мгновенно).
    rebuild_tree(st.hwnd_tree, st);
    if st.accounts.is_empty() {
        status_text(
            st,
            "ящики не настроены: меню «Ящики → Добавить ящик» (accounts.json общий с Soroka)",
        );
    } else if let Some(key) = st.current_folder.clone() {
        status_text(st, "синхронизация...");
        if let Some(b) = &st.backend {
            b.post(Job::SyncFolder { key });
        }
    }

    unsafe {
        layout(hwnd, st);
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetFocus(Some(st.hwnd_list));
    }

    // Цикл сообщений. Ускорители — только когда фокус в главном окне:
    // в поле поиска «/» и буквы должны печататься, в окнах редактора — свои
    // клавиши. Фокус внутри WebView2 живёт в другом потоке (GetFocus пуст) —
    // там ускорители работают, как и раньше.
    let mut msg = MSG::default();
    loop {
        let r = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if r.0 <= 0 {
            break;
        }
        let focused = unsafe { GetFocus() };
        let in_search_edit = focused == st.hwnd_search;
        // GetAncestor в 0.62 возвращает HWND (0 при неудаче) — без Result.
        let root = if focused.0.is_null() {
            HWND::default()
        } else {
            unsafe { GetAncestor(focused, GA_ROOT) }
        };
        let accels_ok = !in_search_edit && (focused.0.is_null() || root == hwnd);
        if accels_ok && unsafe { TranslateAcceleratorW(hwnd, haccel, &msg) } != 0 {
            continue;
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    unsafe { CoUninitialize() };
    Ok(())
}

fn main() {
    if let Err(err) = run() {
        eprintln!("ошибка: {err}");
        std::process::exit(1);
    }
}
