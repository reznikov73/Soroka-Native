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
mod autostart;
mod backend;
mod compose;
mod dates;
// Черновики (TASK-N08): чистая логика (id, отпечаток изменений, заголовок
// X-Soroka-Draft, копия/вложения в записи drafts) — тестируется на Linux.
mod draft;
mod ico;
mod layout_store;
// Настройки — дословный порт settings.rs Soroka (TASK-020/030/040/068...):
// большая часть API (категории, промпты, подписи) понадобится в Фазе 3
// (N13/N14), сейчас оболочка использует чтение и свои ключи — TASK-N09.
#[allow(dead_code)]
mod settings;
mod settings_dlg;
// Трей и автопроверка (TASK-N07): notify/ico — чистая логика текстов и
// разбора .ico (тестируется на Linux), tray — Win32-обвязка Shell_NotifyIconW.
mod notify;
mod tray;
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
    BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, FillRect, FrameRect, GetWindowDC, HDC,
    HGDIOBJ, PAINTSTRUCT, ReleaseDC, SetBkColor, SetTextColor,
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
use windows::Win32::UI::Shell::NIN_BALLOONUSERCLICK;
use windows::Win32::UI::WindowsAndMessaging::{
    BeginDeferWindowPos, CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CallWindowProcW,
    CheckMenuItem, CreateWindowExW, DefWindowProcW, DeferWindowPos, DestroyIcon, DestroyWindow,
    DispatchMessageW, ES_AUTOHSCROLL, EndDeferWindowPos, GA_ROOT, GWLP_USERDATA, GWLP_WNDPROC,
    GetAncestor, GetClientRect, GetCursorPos, GetForegroundWindow, GetMenu, GetMessageW, GetParent,
    GetWindowLongPtrW, GetWindowRect, IDC_ARROW, IsIconic, IsWindowVisible, KillTimer, LoadCursorW,
    MB_ICONINFORMATION, MB_OK, MB_YESNO, MF_BYCOMMAND, MF_CHECKED, MF_UNCHECKED, MINMAXINFO, MSG,
    MessageBoxW, PostMessageW, PostQuitMessage, RegisterClassW, RegisterWindowMessageW,
    SIZE_MINIMIZED, SW_HIDE, SW_RESTORE, SW_SHOW, SW_SHOWMAXIMIZED, SWP_NOZORDER, SendMessageW,
    SetForegroundWindow, SetTimer, SetWindowLongPtrW, SetWindowPos, ShowWindow,
    TranslateAcceleratorW, TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CHAR, WM_CLOSE,
    WM_COMMAND, WM_CONTEXTMENU, WM_CTLCOLOREDIT, WM_DESTROY, WM_ERASEBKGND, WM_GETMINMAXINFO,
    WM_KEYDOWN, WM_KILLFOCUS, WM_LBUTTONDBLCLK, WM_LBUTTONUP, WM_NCCREATE, WM_NCPAINT, WM_NOTIFY,
    WM_PAINT, WM_RBUTTONUP, WM_SETFOCUS, WM_SETFONT, WM_SIZE, WM_TIMER, WNDCLASSW, WNDPROC,
    WS_CHILD, WS_CLIPCHILDREN, WS_EX_CLIENTEDGE, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
};
use windows::core::{Error, HSTRING, PCWSTR, Result, w};

use account_dlg::{WM_APP_ACCOUNT_CLOSED, WM_APP_ACCOUNT_SAVED};
use backend::{
    Backend, BodyData, DraftSaved, Job, MailEvent, WM_APP_DRAFT_SAVED, WM_APP_MAIL,
    WM_APP_SEND_RESULT,
};
use compose::{ComposePrefill, WM_APP_COMPOSE_CLOSED};
use mail::store::Store;
use settings_dlg::{WM_APP_SETTINGS_CLOSED, WM_APP_SETTINGS_SAVED};
use state::AppState;
use tray::WM_APP_TRAY;
use ui::{
    EN_CHANGE, IDM_ACCOUNT_ADD, IDM_COMPOSE, IDM_DELETE, IDM_EXIT, IDM_FORWARD, IDM_REPLY,
    IDM_REPLY_ALL, IDM_SEARCH_CLOSE, IDM_SEARCH_FOCUS, IDM_SEARCH_GO, IDM_SETTINGS, IDM_SYNC,
    IDM_THEME, IDM_TOGGLE_SEEN, IDM_TRAY_OPEN, IDT_AUTOSYNC, IDT_SEARCH, PANE_MIN, SEARCH_H,
    SPLIT_W, STATUS_CLASS, STATUS_H, SplitCtx, apply_mail_event, apply_theme, create_fonts,
    create_menu, empty_pane_html, enter_search, exit_search, fire_search, letter_html,
    on_get_disp_info, on_list_custom_draw, on_list_item_changed, on_tree_select, rebuild_tree,
    register_splitter_class, register_status_class, restore_placement, setup_list_columns,
    snapshot_placement, status_text, track_list_menu, track_tree_menu,
};

const WINDOW_CLASS: PCWSTR = w!("SorokaNativeWnd");
/// EM_SETCUEBANNER (0x1501): подсказка в пустом EDIT; wparam=1 — показывать и в фокусе.
const EM_SETCUEBANNER: u32 = 0x1501;

/// Клик по вложению в панели письма (WebView2 → оболочка): lparam = индекс.
const WM_APP_ATTACH_CLICK: u32 = WM_APP_SETTINGS_CLOSED + 1;

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

    // Батч позиций всех детей одним DeferWindowPos: одна перерисовка вместо
    // шести отдельных — иначе при drag сплиттера окно мерцает (TASK-N04).
    let mut entries: Vec<(HWND, i32, i32, i32, i32)> = Vec::with_capacity(6);
    entries.push((st.hwnd_tree, 0, 0, tree_px, h_main));
    if search_open && !st.hwnd_search.0.is_null() {
        entries.push((st.hwnd_search, x_list, 2, list_px, SEARCH_H - 4));
    }
    entries.push((
        st.hwnd_list,
        x_list,
        list_top,
        list_px,
        (h_main - list_top).max(0),
    ));
    entries.push((st.hwnd_split_tree, x_split_tree, 0, SPLIT_W, h_main));
    entries.push((st.hwnd_split_pane, x_split_pane, 0, SPLIT_W, h_main));
    if !st.hwnd_status.0.is_null() && h > STATUS_H {
        entries.push((st.hwnd_status, 0, h - STATUS_H, w, STATUS_H));
    }
    unsafe {
        match BeginDeferWindowPos(entries.len() as i32) {
            Ok(mut wpos) => {
                for (h2, x, y, cx, cy) in &entries {
                    wpos = DeferWindowPos(wpos, *h2, None, *x, *y, *cx, *cy, SWP_NOZORDER)
                        .unwrap_or(wpos);
                }
                let _ = EndDeferWindowPos(wpos);
            }
            Err(_) => {
                // Резервный путь (практически недостижим): по одному.
                for (h2, x, y, cx, cy) in &entries {
                    let _ = SetWindowPos(*h2, None, *x, *y, *cx, *cy, SWP_NOZORDER);
                }
            }
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
        atts: Vec::new(),
        draft_id: None,
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
        atts: Vec::new(),
        draft_id: None,
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

/// Ящик, чья папка черновиков — этот ключ (по кэшу папок, как find_drafts
/// при сохранении черновика TASK-N08). None — ключ не папка черновиков.
fn drafts_account_of(key: &str) -> Option<String> {
    let acc = mail::accounts::split_key(key).0?;
    let found = Store::open_default()
        .ok()
        .and_then(|s| s.load_folders(&format!("{acc}::%")).ok())
        .and_then(|fs| mail::imap::find_drafts(&fs))?;
    (found == key).then_some(acc)
}

/// Двойной клик по строке списка (TASK-N08): в папке черновиков — продолжить
/// редактирование в окне написания (порт TASK-022; получатели/копия/вложения
/// берутся из локальной записи — серверная копия их не хранит).
fn open_draft_if_any(hwnd: HWND, st: &mut AppState, i: usize) {
    let Some(uid) = st.rows.get(i).map(|r| r.hdr.uid) else {
        return;
    };
    let Some(key) = st.current_folder.clone() else {
        return;
    };
    let Some(acc) = drafts_account_of(&key) else {
        return;
    };
    let Some(rec) = Store::open_default()
        .ok()
        .and_then(|s| s.draft_by_uid(&acc, uid).ok().flatten())
    else {
        return;
    };
    let (cc, atts) = draft::decode_extra(rec.attachments.as_deref());
    let p = ComposePrefill {
        to: rec.to_addr,
        cc,
        subject: rec.subject,
        body: rec.body,
        atts: atts
            .into_iter()
            .map(|(name, mime, data)| mail::smtp::OutgoingAttachment { name, mime, data })
            .collect(),
        draft_id: Some(rec.id),
    };
    if let Some(h) = compose::open(hwnd, st, Some(p)) {
        st.hwnd_compose = Some(h);
    }
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
        WM_NCPAINT => {
            // Рамка поля поиска — палитра вместо системного 3D-контура
            // (TASK-N10); место под рамку оставляет WS_EX_CLIENTEDGE.
            unsafe { paint_search_frame(hwnd) };
            return LRESULT(0);
        }
        WM_SETFOCUS | WM_KILLFOCUS => {
            // Фокус меняет цвет рамки (штрих ↔ акцент) — перекрасить,
            // дальше обычная обработка фокуса.
            unsafe { paint_search_frame(hwnd) };
        }
        _ => {}
    }
    let orig = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) };
    let proc: WNDPROC = unsafe { std::mem::transmute::<isize, WNDPROC>(orig) };
    unsafe { CallWindowProcW(proc, hwnd, msg, wparam, lparam) }
}

/// Рамка поля поиска из палитры (TASK-N10): на месте системного 3D-контура
/// WS_EX_CLIENTEDGE — плоская линия, обычная (--stroke) или акцентная в
/// фокусе (--accent), как .tb-search в Soroka. Вызывается из WM_NCPAINT
/// сабкласса и при смене темы (apply_theme шлёт WM_NCPAINT полю).
unsafe fn paint_search_frame(hwnd: HWND) {
    unsafe {
        let parent = GetParent(hwnd).unwrap_or_default();
        if parent.is_invalid() {
            return;
        }
        let st = &*(GetWindowLongPtrW(parent, GWLP_USERDATA) as *const AppState);
        let p = theme::palette(st.dark);
        let color = if GetFocus() == hwnd {
            p.accent
        } else {
            p.stroke
        };
        let hdc = GetWindowDC(Some(hwnd));
        let mut wr = RECT::default();
        if GetWindowRect(hwnd, &mut wr).is_ok() {
            let w = wr.right - wr.left;
            let h = wr.bottom - wr.top;
            let br = CreateSolidBrush(color);
            // CLIENTEDGE оставляет 2px — закрашиваем обе линии плоско.
            let _ = FrameRect(
                hdc,
                &RECT {
                    left: 0,
                    top: 0,
                    right: w,
                    bottom: h,
                },
                br,
            );
            let _ = FrameRect(
                hdc,
                &RECT {
                    left: 1,
                    top: 1,
                    right: w - 1,
                    bottom: h - 1,
                },
                br,
            );
            let _ = DeleteObject(HGDIOBJ(br.0));
        }
        let _ = ReleaseDC(Some(hwnd), hdc);
    }
}

/// Применить тему (TASK-N05; с N09 хранится в settings.json — native_theme).
/// Оболочка красится применением палитры, панель письма — пере-навигацией
/// (данные письма уже в pane_body — бэкенд не трогаем). Вызывается при
/// переключении «Вид → Тёмная тема» и из диалога настроек.
unsafe fn set_theme(hwnd: HWND, st: &mut AppState, dark: bool) {
    st.dark = dark;
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
    // Тема — в общем settings.json (TASK-N09): только свой ключ,
    // остальное содержимое файла не трогаем.
    settings::store_theme(dark);
}

/// Показать главное окно из трея (клик/меню/balloon): восстановить из
/// свёрнутого, поднять на передний план (TASK-N07).
unsafe fn show_main(hwnd: HWND) {
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        } else {
            let _ = ShowWindow(hwnd, SW_SHOW);
        }
        let _ = SetForegroundWindow(hwnd);
    }
}

/// Пересчитать общее число непрочитанных по всем ящикам из кэша и
/// обновить подпись/иконку трея (порт set_tray_unread; чтение кэша —
/// как в rebuild_tree, малый запрос).
fn refresh_unread(hwnd: HWND, st: &mut AppState) {
    let mut total = 0u32;
    if let Ok(store) = Store::open_default() {
        for acc in &st.accounts {
            let like = format!("{}::%", acc.id);
            if let Ok(rows) = store.unread_counts(&like) {
                total += rows.iter().map(|&(_, u)| u).sum::<u32>();
            }
        }
    }
    st.unread_total = total;
    tray::refresh(hwnd, st);
}

/// Перевооружить таймер автопроверки под st.autosync_min (0 — выключить).
fn arm_autosync(hwnd: HWND, st: &AppState) {
    unsafe {
        let _ = KillTimer(Some(hwnd), IDT_AUTOSYNC);
        if st.autosync_min > 0 {
            // Минимум 1 минута: 0 исключён выше, дробных минут не бывает.
            let _ = SetTimer(Some(hwnd), IDT_AUTOSYNC, st.autosync_min * 60_000, None);
        }
    }
}

/// Снимок UID кэша INBOX перед синхронизацией — для «новых писем»
/// (TASK-N07). Пустой кэш не снимаем: первая проверка ящика — базлайн,
/// без уведомления (иначе после добавления ящика балон показал бы всю
/// историю папки). Не-INBOX не снимаем: уведомления — только о входящих,
/// как sync_inbox в Soroka.
fn snapshot_inbox(st: &mut AppState, key: &str) {
    if !key.ends_with("::INBOX") {
        return;
    }
    let known: Vec<u32> = Store::open_default()
        .ok()
        .and_then(|s| s.load_folder(key).ok())
        .map(|v| v.iter().map(|m| m.uid).collect())
        .unwrap_or_default();
    if !known.is_empty() {
        st.autosync_known.insert(key.to_string(), known);
    }
}

/// Автопроверка по таймеру (TASK-N07): синхронизируем INBOX каждого
/// ящика (у Soroka — таймеры по каждому ящику, интервал у нас общий —
/// native_autosync_min). Задачи уходят в очередь рабочего потока;
/// снимки UID — для уведомлений о новых письмах.
fn trigger_autosync(st: &mut AppState) {
    if st.accounts.is_empty() {
        return;
    }
    // Ключи собираем заранее: дальше st меняется (снимки, статус).
    let keys: Vec<String> = st
        .accounts
        .iter()
        .map(|acc| format!("{}::INBOX", acc.id))
        .collect();
    for key in keys {
        snapshot_inbox(st, &key);
        if let Some(b) = &st.backend {
            b.post(Job::SyncFolder { key });
        }
    }
    status_text(st, "автопроверка почты...");
}

/// Проверить «новые письма» после SyncDone (TASK-N07): сравниваем свежий
/// кэш со снимком до синхронизации. Уведомление — balloon трея — только
/// когда включены ui.notifications и окно не на виду (порт notify_new_mail,
/// TASK-087/099; вместо «загружено N» — честное число новых UID).
fn check_new_mail(hwnd: HWND, st: &mut AppState, key: &str) {
    let Some(known) = st.autosync_known.remove(key) else {
        return;
    };
    let fresh = Store::open_default()
        .ok()
        .and_then(|s| s.load_folder(key).ok())
        .unwrap_or_default();
    let fresh_uids: Vec<u32> = fresh.iter().map(|m| m.uid).collect();
    let n = notify::new_count(&known, &fresh_uids);
    if n == 0 || !st.notifications {
        return;
    }
    // Окно на виду — не шумим (как в Soroka: is_visible && is_focused).
    let at_hand =
        unsafe { IsWindowVisible(hwnd).as_bool() } && unsafe { GetForegroundWindow() } == hwnd;
    if at_hand {
        return;
    }
    let first = fresh
        .iter()
        .find(|m| !known.contains(&m.uid))
        .map(|m| (m.from.as_str(), m.subject.as_str()));
    tray::balloon(
        hwnd,
        &notify::new_mail_title(n),
        &notify::new_mail_body(first),
    );
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
            let res = unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
            // Кнопка «—» — в трей, если так велит ui.minimize_to_tray
            // (TASK-N07; в Soroka то же делает обработчик #win-min).
            if wparam.0 == SIZE_MINIMIZED as usize && st.minimize_to_tray {
                unsafe {
                    let _ = ShowWindow(hwnd, SW_HIDE);
                };
            }
            res
        }
        WM_CLOSE => {
            // Крестик — как в Soroka TASK-072: при включённом сворачивании
            // в трей прячет окно, а не завершает программу (выход — меню
            // трея); подсказку показываем один раз за запуск.
            let st = unsafe { &mut *(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppState) };
            if st.minimize_to_tray {
                unsafe {
                    let _ = ShowWindow(hwnd, SW_HIDE);
                };
                if !st.hide_hint_shown {
                    st.hide_hint_shown = true;
                    tray::balloon(
                        hwnd,
                        "POS Mail",
                        "POS Mail свёрнут в трей. Выход — в меню трея.",
                    );
                }
                LRESULT(0)
            } else {
                unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
            }
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
                    // TASK-N05: быстрое переключение темы меню «Вид».
                    unsafe { set_theme(hwnd, st, !st.dark) };
                    status_text(
                        st,
                        if st.dark {
                            "тема: тёмная (палитра Soroka)"
                        } else {
                            "тема: светлая"
                        },
                    );
                }
                IDM_SETTINGS => {
                    // TASK-N09: диалог настроек (settings.json общий с Soroka).
                    if let Some(h) = settings_dlg::open(hwnd, st) {
                        st.hwnd_settings = Some(h);
                    }
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
                        // Снимок UID до синхронизации: F5 из главного окна и
                        // «Проверить почту» из трея одинаково участвуют в
                        // уведомлениях о новых письмах (TASK-N07).
                        snapshot_inbox(st, &key);
                        status_text(st, "синхронизация...");
                        if let Some(b) = &st.backend {
                            b.post(Job::SyncFolder { key });
                        }
                    } else {
                        status_text(st, "сначала выберите папку");
                    }
                }
                IDM_TRAY_OPEN => {
                    // Меню трея: Открыть POS Mail (TASK-N07).
                    unsafe { show_main(hwnd) };
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
        WM_CONTEXTMENU => {
            // Контекстные меню (TASK-N10): список — действия над письмом
            // под курсором, дерево — «Проверить папку». lparam — экранные
            // координаты; -1 — меню с клавиатуры (Shift+F10), берём курсор.
            let target = HWND(wparam.0 as *mut core::ffi::c_void);
            let st = unsafe { &mut *(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppState) };
            let (x, y) = if lparam.0 == -1 {
                let mut pt = POINT::default();
                if unsafe { GetCursorPos(&mut pt) }.is_err() {
                    return LRESULT(0);
                }
                (pt.x, pt.y)
            } else {
                // Знаковые координаты (могут быть отрицательны на многомониторе).
                (
                    (lparam.0 & 0xFFFF) as u16 as i16 as i32,
                    ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32,
                )
            };
            if target == st.hwnd_list {
                track_list_menu(hwnd, st.hwnd_list, x, y, st);
                return LRESULT(0);
            }
            if target == st.hwnd_tree {
                track_tree_menu(hwnd, st.hwnd_tree, x, y, st);
                return LRESULT(0);
            }
            // Прочее (фон, заголовок окна) — системное поведение:
            // DefWindowProc покажет меню окна у заголовка.
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
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
                    windows::Win32::UI::Controls::LVN_ITEMACTIVATE => {
                        // Двойной клик по строке (TASK-N08): черновик —
                        // продолжить редактирование в окне написания.
                        let nm = unsafe {
                            &*(lparam.0 as *const windows::Win32::UI::Controls::NMITEMACTIVATE)
                        };
                        open_draft_if_any(hwnd, st, nm.iItem.max(0) as usize);
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
                let mut tray_dirty = false;
                for ev in b.drain() {
                    // События, меняющие непрочитанные: после них — счётчик
                    // трея (TASK-N07). Проверку «новых» делаем до apply:
                    // кэш бэкенд уже обновил, порядок UI-списка неважен.
                    if matches!(
                        ev,
                        MailEvent::SyncDone { .. }
                            | MailEvent::Seen { .. }
                            | MailEvent::Unseen { .. }
                            | MailEvent::Deleted { .. }
                    ) {
                        tray_dirty = true;
                    }
                    let sync_key = if let MailEvent::SyncDone {
                        key, error: None, ..
                    } = &ev
                    {
                        Some(key.clone())
                    } else {
                        None
                    };
                    apply_mail_event(hwnd, st.hwnd_list, st.hwnd_tree, st, ev);
                    if let Some(key) = sync_key {
                        check_new_mail(hwnd, st, &key);
                    }
                }
                if tray_dirty {
                    refresh_unread(hwnd, st);
                }
            }
            LRESULT(0)
        }
        WM_APP_TRAY => {
            // События иконки трея (TASK-N07): lparam — сообщение мыши или
            // клик по balloon. Левая кнопка и клик по уведомлению —
            // показать окно; правая/контекстное меню — меню трея.
            let ev = lparam.0 as u32;
            if ev == WM_LBUTTONUP || ev == WM_LBUTTONDBLCLK || ev == NIN_BALLOONUSERCLICK {
                unsafe { show_main(hwnd) };
            } else if ev == WM_RBUTTONUP || ev == WM_CONTEXTMENU {
                tray::track_menu(hwnd);
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
            // Черновик мог появиться/измениться (TASK-N08): если смотрим его
            // папку — обновляем список (как mail-changed в Soroka после
            // закрытия редактора).
            if let Some(key) = st.current_folder.clone()
                && drafts_account_of(&key).is_some()
                && let Some(b) = &st.backend
            {
                b.post(Job::SyncFolder { key });
            }
            LRESULT(0)
        }
        WM_APP_ACCOUNT_SAVED => {
            let st = unsafe { &mut *(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppState) };
            rebuild_tree(st.hwnd_tree, st);
            // Ящик добавился/удалился — счётчик непрочитанных трея тоже.
            refresh_unread(hwnd, st);
            LRESULT(0)
        }
        WM_APP_SETTINGS_SAVED => {
            // Диалог настроек сохранил файл (TASK-N09): перечитываем свои
            // ключи; тема могла измениться — применяем как «Вид → Тёмная тема».
            // N07: трейные ключи — в кэш стейта, таймер и автозапуск —
            // перевооружить.
            let st = unsafe { &mut *(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppState) };
            let view = settings::native_view();
            if view.dark != st.dark {
                unsafe { set_theme(hwnd, st, view.dark) };
            }
            st.minimize_to_tray = view.minimize_to_tray;
            st.notifications = view.notifications;
            st.autosync_min = view.autosync_min;
            arm_autosync(hwnd, st);
            autostart::apply(view.autostart);
            status_text(st, "настройки сохранены (settings.json общий с Soroka)");
            LRESULT(0)
        }
        WM_APP_SETTINGS_CLOSED => {
            let st = unsafe { &mut *(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppState) };
            st.hwnd_settings = None;
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
        WM_APP_DRAFT_SAVED => {
            // Ответ сохранения идёт окну редактора (TASK-N08); сюда не
            // должен попадать — но Box освободим, чтобы не течь.
            drop(unsafe { Box::from_raw(lparam.0 as *mut DraftSaved) });
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
        WM_TIMER if wparam.0 == IDT_AUTOSYNC => {
            // Автопроверка (TASK-N07): интервал — native_autosync_min из
            // общего settings.json; работает и со скрытым окном (таймер
            // окна живёт, пока жив цикл сообщений).
            let st = unsafe { &mut *(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppState) };
            trigger_autosync(st);
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
            // Трей: убрать иконку до уничтожения окна (TASK-N07), таймер
            // гасится вместе с окном, HICON — наши, освобождаем сами.
            {
                let st = unsafe { &mut *(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppState) };
                tray::remove(hwnd, st);
                unsafe {
                    let _ = KillTimer(Some(hwnd), IDT_AUTOSYNC);
                    if !st.hicon_tray.is_invalid() {
                        let _ = DestroyIcon(st.hicon_tray);
                    }
                    if !st.hicon_tray_alert.is_invalid() {
                        let _ = DestroyIcon(st.hicon_tray_alert);
                    }
                }
            }
            // Геометрия главного окна — в layout.json (TASK-N06), до
            // PostQuitMessage: дальше цикл сообщений завершится.
            layout_store::store_window(layout_store::WinKind::Main, snapshot_placement(hwnd));
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => {
            // TaskbarCreated (TASK-N07): Explorer перезапустился — иконку
            // трея нужно добавить заново (иначе она пропадает).
            let st = unsafe { &mut *(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppState) };
            if st.taskbar_created != 0 && msg == st.taskbar_created {
                tray::readd(hwnd, st);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
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
        // Иконка окна/панели задач — app.ico Сороки (TASK-N07; ранее
        // оставалась системная заглушка).
        hIcon: tray::load_app_icon().unwrap_or_default(),
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
    // Компоновка из %APPDATA%\Soroka\layout.json (TASK-N04/N06): веса и
    // геометрия главного окна; нет файла — дефолт Soroka (веса
    // LAYOUT_DEFAULTS, системная позиция окна).
    let mut main_placement = None;
    let layout_cfg = layout_store::load();
    if let Some(l) = &layout_cfg {
        unsafe {
            (*state).wf = l.wf;
            (*state).wl = l.wl;
            (*state).wr = l.wr;
        }
        main_placement = l.main;
    }
    // Тема — в общем settings.json (native_theme, TASK-N09). Из layout.json
    // (там её хранил N05) переносится одноразово; нет нигде — тёмная,
    // как в Soroka. Здесь же — кэш трейных ключей (TASK-N07): свернуть в
    // трей, уведомления, интервал автопроверки.
    let native_view = settings::native_view();
    unsafe {
        (*state).dark = settings::init_theme(layout_cfg.as_ref().and_then(|l| l.theme.as_deref()));
        (*state).minimize_to_tray = native_view.minimize_to_tray;
        (*state).notifications = native_view.notifications;
        (*state).autosync_min = native_view.autosync_min;
    }
    let hwnd = unsafe {
        CreateWindowExW(
            // WS_CLIPCHILDREN (TASK-N04.3): родитель никогда не рисует поверх
            // детей — его стирание фона не мелькает сквозь панели между их
            // перерисовками. WS_EX_COMPOSITED из v0.3.7 убран: с WebView2-
            // ребёнком (ускоренная отрисовка вне WM_PAINT) композиция сама
            // источник вспышек; от межоконных разрывов теперь WS_CLIPCHILDREN
            // + двойная буферизация контролов (манифест comctl32 v6).
            WINDOW_EX_STYLE(0),
            WINDOW_CLASS,
            w!("POS Mail — Фаза 1: почтовый каркас"),
            WINDOW_STYLE(WS_OVERLAPPEDWINDOW.0 | WS_CLIPCHILDREN.0),
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
        last_x: 0,
        timer_armed: false,
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
        last_x: 0,
        timer_armed: false,
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

    // Двойная буферизация дерева: без неё TreeView целиком перерисовывается
    // на каждый шаг drag сплиттеров и ресайза (TASK-N04).
    unsafe {
        SendMessageW(
            st.hwnd_tree,
            windows::Win32::UI::Controls::TVM_SETEXTENDEDSTYLE,
            Some(WPARAM(
                windows::Win32::UI::Controls::TVS_EX_DOUBLEBUFFER as usize,
            )),
            Some(LPARAM(
                windows::Win32::UI::Controls::TVS_EX_DOUBLEBUFFER as isize,
            )),
        );
    }

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

    // Трей (TASK-N07): иконки Сороки (конверт/конверт с точкой), добавить
    // до показа окна; TaskbarCreated — для возврата после рестарта Explorer.
    tray::load_tray_icons(st);
    tray::add(hwnd, st);
    st.taskbar_created = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };

    // Дерево ящиков/папок + выбор первого INBOX (загрузка из кэша — мгновенно).
    rebuild_tree(st.hwnd_tree, st);
    // Подпись/иконка трея по непрочитанным из кэша (TASK-N07).
    refresh_unread(hwnd, st);
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

    // Автопроверка (TASK-N07): таймер по native_autosync_min (0 — выкл.),
    // автозапуск — синхронизировать с ui.autostart (реестр, HKCU Run).
    arm_autosync(hwnd, st);
    autostart::apply(native_view.autostart);

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
