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
mod dates;
mod layout_store;
// Ядро Soroka перенесено дословно (правило WORKFLOW: src/mail/ не редактировать).
// Часть его API (контакты, IDLE, пресеты провайдеров) подключается в Фазах 2–3 —
// до тех пор dead_code на модуле неинформативен. Снять allow вместе с подключением
// соответствующих API (TASK-N02, v0.2.2).
// needless_borrow — то же самое: два места в imap.rs (726–727), ядро не правим.
#[allow(dead_code, clippy::needless_borrow)]
mod mail;
mod state;
mod theme;
mod ui;
mod wide;

use std::sync::mpsc;

use webview2_com::{
    CoreWebView2EnvironmentOptions, CreateCoreWebView2ControllerCompletedHandler,
    CreateCoreWebView2EnvironmentCompletedHandler, Microsoft::Web::WebView2::Win32::*,
    WebMessageReceivedEventHandler, wait_with_pump,
};
use windows::Win32::Foundation::{
    E_POINTER, E_UNEXPECTED, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, FillRect, HDC, HGDIOBJ, PAINTSTRUCT,
    SetBkColor, SetTextColor,
};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{
    ICC_BAR_CLASSES, ICC_LISTVIEW_CLASSES, ICC_TREEVIEW_CLASSES, INITCOMMONCONTROLSEX,
    InitCommonControlsEx, LVN_GETDISPINFOW, LVS_OWNERDATA, LVS_REPORT, LVS_SHOWSELALWAYS,
    NM_CUSTOMDRAW, NMHDR, NMLISTVIEW, NMLVDISPINFOW, NMTREEVIEWW, TVN_SELCHANGEDW, WC_LISTVIEW,
    WC_TREEVIEW,
};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetFocus, SetFocus, VK_ESCAPE, VK_RETURN};
use windows::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CallWindowProcW, CheckMenuItem,
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, ES_AUTOHSCROLL, GA_ROOT,
    GWLP_USERDATA, GWLP_WNDPROC, GetAncestor, GetClientRect, GetMenu, GetMessageW, GetParent,
    GetWindowLongPtrW, IDC_ARROW, LoadCursorW, MB_ICONINFORMATION, MB_OK, MB_YESNO, MF_BYCOMMAND,
    MF_CHECKED, MF_UNCHECKED, MINMAXINFO, MSG, MessageBoxW, PostMessageW, PostQuitMessage,
    RegisterClassW, SW_SHOW, SW_SHOWMAXIMIZED, SWP_NOZORDER, SendMessageW, SetTimer,
    SetWindowLongPtrW, SetWindowPos, ShowWindow, TranslateAcceleratorW, TranslateMessage,
    WINDOW_EX_STYLE, WINDOW_STYLE, WM_CHAR, WM_COMMAND, WM_CTLCOLOREDIT, WM_DESTROY, WM_ERASEBKGND,
    WM_GETMINMAXINFO, WM_KEYDOWN, WM_NCCREATE, WM_NOTIFY, WM_PAINT, WM_SETFONT, WM_SIZE, WM_TIMER,
    WNDCLASSW, WNDPROC, WS_CHILD, WS_EX_CLIENTEDGE, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
};
use windows::core::{Error, HSTRING, PCWSTR, Result, w};

use account_dlg::{WM_APP_ACCOUNT_CLOSED, WM_APP_ACCOUNT_SAVED};
use backend::{Backend, BodyData, Job, WM_APP_MAIL, WM_APP_SEND_RESULT};
use compose::{ComposePrefill, WM_APP_COMPOSE_CLOSED};
use state::AppState;
use ui::{
    EN_CHANGE, IDM_ACCOUNT_ADD, IDM_COMPOSE, IDM_DELETE, IDM_EXIT, IDM_FORWARD, IDM_REPLY,
    IDM_REPLY_ALL, IDM_SEARCH_CLOSE, IDM_SEARCH_FOCUS, IDM_SEARCH_GO, IDM_SYNC, IDM_THEME,
    IDM_TOGGLE_SEEN, IDT_SEARCH, PANE_MIN, SEARCH_H, SPLIT_W, STATUS_CLASS, STATUS_H, SplitCtx,
    apply_mail_event, apply_theme, create_fonts, create_menu, empty_pane_html, enter_search,
    exit_search, fire_search, letter_html, on_get_disp_info, on_list_custom_draw,
    on_list_item_changed, on_tree_select, rebuild_tree, register_splitter_class,
    register_status_class, restore_placement, setup_list_columns, snapshot_placement, status_text,
};

const WINDOW_CLASS: PCWSTR = w!("SorokaNativeWnd");
/// EM_SETCUEBANNER (0x1501): подсказка в пустом EDIT; wparam=1 — показывать и в фокусе.
const EM_SETCUEBANNER: u32 = 0x1501;

/// Клик по вложению в панели письма (WebView2 → оболочка): lparam = индекс.
const WM_APP_ATTACH_CLICK: u32 = WM_APP_ACCOUNT_CLOSED + 1;

/// Раскладка панелей (TASK-N04): дерево | сплиттер | список | сплиттер | тело.
/// Ширины считаются из весов (аналог flex-grow в Soroka), минимум панели —
/// 110px; на время жеста сплиттера (st.dragging) SetBounds панели письма
/// отложен — WebView2 не участвует в кадре жеста.
unsafe fn layout(hwnd: HWND, st: &mut AppState) {
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

    // Ширины из весов (dragSplit Soroka: сумма весов пары сохраняется).
    let content = (w - 2 * SPLIT_W).max(0);
    let total = st.wf + st.wl + st.wr;
    let mut tree_px = if total > 0.0 {
        ((content as f64) * st.wf / total).round() as i32
    } else {
        PANE_MIN
    };
    tree_px = tree_px.clamp(PANE_MIN, (content - 2 * PANE_MIN).max(PANE_MIN));
    let mut pane_px = if total > 0.0 {
        ((content as f64) * st.wr / total).round() as i32
    } else {
        PANE_MIN
    };
    pane_px = pane_px.clamp(PANE_MIN, (content - tree_px - PANE_MIN).max(PANE_MIN));
    let list_px = (content - tree_px - pane_px).max(PANE_MIN);
    st.tree_px = tree_px;
    st.list_px = list_px;
    st.pane_px = pane_px;

    let x_split_tree = tree_px;
    let x_list = tree_px + SPLIT_W;
    let x_split_pane = x_list + list_px;
    let x_pane = x_split_pane + SPLIT_W;

    unsafe {
        let _ = SetWindowPos(st.hwnd_tree, None, 0, 0, tree_px, h_main, SWP_NOZORDER);
        if search_open && !st.hwnd_search.0.is_null() {
            let _ = SetWindowPos(
                st.hwnd_search,
                None,
                x_list,
                2,
                list_px,
                SEARCH_H - 4,
                SWP_NOZORDER,
            );
        }
        let _ = SetWindowPos(
            st.hwnd_list,
            None,
            x_list,
            list_top,
            list_px,
            (h_main - list_top).max(0),
            SWP_NOZORDER,
        );
        let _ = SetWindowPos(
            st.hwnd_split_tree,
            None,
            x_split_tree,
            0,
            SPLIT_W,
            h_main,
            SWP_NOZORDER,
        );
        let _ = SetWindowPos(
            st.hwnd_split_pane,
            None,
            x_split_pane,
            0,
            SPLIT_W,
            h_main,
            SWP_NOZORDER,
        );
        if !st.hwnd_status.0.is_null() && h > STATUS_H {
            let _ = SetWindowPos(
                st.hwnd_status,
                None,
                0,
                h - STATUS_H,
                w,
                STATUS_H,
                SWP_NOZORDER,
            );
        }
        if !st.dragging
            && let Some(ctrl) = &st.controller
        {
            let _ = ctrl.SetBounds(RECT {
                left: x_pane,
                top: 0,
                right: x_pane + pane_px,
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
        move |error_code: Result<()>,
              environment: Option<ICoreWebView2Environment>|
              -> Result<()> {
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
            &WebMessageReceivedEventHandler::create(Box::new(move |_sender, args| -> Result<()> {
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
            })),
            &mut token,
        )?;
    }

    Ok((controller, webview))
}

/// Цитата исходного письма для ответа/пересылки.
fn quote_body(hdr: &mail::imap::MessageHeader, pane: Option<&state::PaneBody>) -> String {
    let plain = match pane {
        Some(p) => p.text.clone().unwrap_or_else(|| {
            p.html
                .as_deref()
                .map(mail::body::html_to_text)
                .unwrap_or_default()
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
    st.rows
        .iter()
        .find(|m| m.hdr.uid == uid)
        .map(|r| r.hdr.clone())
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
                let cmd = if key == VK_RETURN.0 {
                    IDM_SEARCH_GO
                } else {
                    IDM_SEARCH_CLOSE
                };
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

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
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
                IDM_THEME => {
                    // TASK-N05: переключение темы. Оболочка красится
                    // применением палитры, панель письма — пере-навигацией
                    // (данные письма уже в pane_body — бэкенд не трогаем).
                    st.dark = !st.dark;
                    let menu = unsafe { GetMenu(hwnd) };
                    if !menu.is_invalid() {
                        unsafe {
                            let _ = CheckMenuItem(
                                menu,
                                IDM_THEME as u32,
                                MF_BYCOMMAND.0
                                    | if st.dark {
                                        MF_CHECKED.0
                                    } else {
                                        MF_UNCHECKED.0
                                    },
                            );
                        }
                    }
                    apply_theme(st);
                    let html = if let Some(b) = st.pane_body.as_ref()
                        && let Some(row) = st.rows.iter().find(|r| r.hdr.uid == b.uid)
                    {
                        letter_html(
                            &row.hdr,
                            &BodyData {
                                text: b.text.clone(),
                                html: b.html.clone(),
                                attachments: b.attachments.clone(),
                                truncated: b.truncated,
                            },
                            st.dark,
                        )
                    } else {
                        empty_pane_html(st.dark)
                    };
                    if let Some(wv) = &st.webview {
                        let _ = unsafe { wv.NavigateToString(&HSTRING::from(html)) };
                    }
                    layout_store::save(st.wf, st.wl, st.wr, st.dark);
                    status_text(
                        st,
                        if st.dark {
                            "тема: тёмная (палитра Soroka)"
                        } else {
                            "тема: светлая"
                        },
                    );
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
                IDM_REPLY | IDM_REPLY_ALL => match reply_prefill(st, id == IDM_REPLY_ALL) {
                    Some(p) => {
                        if let Some(h) = compose::open(hwnd, st, Some(p)) {
                            st.hwnd_compose = Some(h);
                        }
                    }
                    None => status_text(st, "сначала выберите письмо"),
                },
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
                        if !key.is_empty()
                            && let Some(b) = &st.backend
                        {
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
                    if !key.is_empty()
                        && let Some(b) = &st.backend
                    {
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
                        // Список присылает NMLVCUSTOMDRAW (расширение NMCUSTOMDRAW).
                        return on_list_custom_draw(
                            st,
                            lparam.0 as *mut windows::Win32::UI::Controls::NMLVCUSTOMDRAW,
                        );
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
        WM_ERASEBKGND => {
            // Фон главного окна — палитра темы (TASK-N05): закрыть щели
            // вокруг поля поиска и на ресайзе до переложения детей.
            let st = unsafe { &mut *(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppState) };
            let p = theme::palette(st.dark);
            let hdc = HDC(wparam.0 as *mut core::ffi::c_void);
            let mut rc = RECT::default();
            if unsafe { GetClientRect(hwnd, &mut rc) }.is_ok() {
                let br = unsafe { CreateSolidBrush(p.bg) };
                unsafe { FillRect(hdc, &rc, br) };
                unsafe {
                    let _ = DeleteObject(HGDIOBJ(br.0));
                }
            }
            LRESULT(1)
        }
        WM_CTLCOLOREDIT => {
            // Поле поиска (TASK-N05): фон и текст палитрой, кисть — из
            // AppState (пересоздаётся в apply_theme).
            let st = unsafe { &mut *(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppState) };
            if HWND(lparam.0 as *mut core::ffi::c_void) == st.hwnd_search {
                let p = theme::palette(st.dark);
                let hdc = HDC(wparam.0 as *mut core::ffi::c_void);
                unsafe {
                    let _ = SetTextColor(hdc, p.text);
                    let _ = SetBkColor(hdc, p.surface);
                }
                return LRESULT(st.hbr_edit.0 as isize);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
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
            // Геометрия главного окна — в layout.json (TASK-N06), до
            // PostQuitMessage: дальше цикл сообщений завершится.
            layout_store::store_window(layout_store::WinKind::Main, snapshot_placement(hwnd));
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
    // Класс сплиттеров (TASK-N04).
    register_splitter_class(hmod)?;
    // Класс статус-бара (TASK-N05: собственный, палитра темы).
    register_status_class(hmod)?;

    // Состояние уходит в GWLP_USERDATA через CREATESTRUCTW (WM_NCCREATE).
    let state = Box::into_raw(Box::new(AppState::new(font, font_bold)));
    // Компоновка из %APPDATA%\Soroka\layout.json (TASK-N04/N05/N06):
    // веса, тема, геометрия главного окна; нет файла — дефолт Soroka
    // (веса LAYOUT_DEFAULTS, тёмная тема, системная позиция окна).
    let mut main_placement = None;
    if let Some(l) = layout_store::load() {
        unsafe {
            (*state).wf = l.wf;
            (*state).wl = l.wl;
            (*state).wr = l.wr;
            (*state).dark = l.dark;
        }
        main_placement = l.main;
    }
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
    st.hwnd_status = child(STATUS_CLASS, WINDOW_STYLE(0))?;

    // Поле поиска (TASK-N03): скрытая строка над списком, появляется по «/».
    st.hwnd_search = unsafe {
        CreateWindowExW(
            WS_EX_CLIENTEDGE,
            w!("EDIT"),
            None,
            WS_CHILD | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
            0,
            2,
            400,
            SEARCH_H - 4,
            Some(hwnd),
            None,
            Some(windows::Win32::Foundation::HINSTANCE(hmod.0)),
            None,
        )?
    };

    // Сплиттеры панелей (TASK-N04): дерево|список и список|тело. Контекст —
    // в GWLP_USERDATA через CREATESTRUCTW (как у главного окна).
    let ctx_split_tree = Box::into_raw(Box::new(SplitCtx {
        main: hwnd,
        pane: false,
        captured: false,
        hover: false,
        tracked: false,
        start_x: 0,
        start_a_px: 0,
        sum_px: 0,
        sum_w: 0.0,
    }));
    let ctx_split_pane = Box::into_raw(Box::new(SplitCtx {
        main: hwnd,
        pane: true,
        captured: false,
        hover: false,
        tracked: false,
        start_x: 0,
        start_a_px: 0,
        sum_px: 0,
        sum_w: 0.0,
    }));
    st.hwnd_split_tree = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("SorokaNativeSplitter"),
            None,
            WS_CHILD | WS_VISIBLE,
            0,
            0,
            SPLIT_W,
            100,
            Some(hwnd),
            None,
            Some(windows::Win32::Foundation::HINSTANCE(hmod.0)),
            Some(ctx_split_tree as *const _),
        )?
    };
    st.hwnd_split_pane = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("SorokaNativeSplitter"),
            None,
            WS_CHILD | WS_VISIBLE,
            0,
            0,
            SPLIT_W,
            100,
            Some(hwnd),
            None,
            Some(windows::Win32::Foundation::HINSTANCE(hmod.0)),
            Some(ctx_split_pane as *const _),
        )?
    };

    // Шрифт интерфейса всем детям (системный, подобранный под DPI).
    for h in [st.hwnd_tree, st.hwnd_list, st.hwnd_status, st.hwnd_search] {
        unsafe {
            SendMessageW(
                h,
                WM_SETFONT,
                Some(WPARAM(st.font.0 as usize)),
                Some(LPARAM(1)),
            );
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
    // Галочка темы (TASK-N05): тёмная — как в Soroka — отмечена по умолчанию.
    if st.dark {
        let menu = unsafe { GetMenu(hwnd) };
        if !menu.is_invalid() {
            unsafe {
                let _ = CheckMenuItem(menu, IDM_THEME as u32, MF_BYCOMMAND.0 | MF_CHECKED.0);
            }
        }
    }
    // Тема до показа окна — без вспышки системных цветов (TASK-N05).
    apply_theme(st);

    // Панель письма: WebView2 (единственный движок в оболочке).
    let backend = Backend::spawn(hwnd);
    let (controller, webview) = unsafe { create_webview(hwnd)? };
    st.controller = Some(controller);
    st.webview = Some(webview);
    if let Some(wv) = &st.webview {
        let _ = unsafe { wv.NavigateToString(&HSTRING::from(empty_pane_html(st.dark))) };
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
        // Геометрия главного окна из layout.json (TASK-N06) — до показа:
        // SetWindowPlacement ставит нормальный прямоугольник скрытому окну,
        // развёрнутость уходит в ShowWindow ниже.
        let maximized = main_placement
            .map(|p| restore_placement(hwnd, &p))
            .unwrap_or(false);
        layout(hwnd, st);
        let _ = ShowWindow(hwnd, if maximized { SW_SHOWMAXIMIZED } else { SW_SHOW });
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
