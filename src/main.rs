//! POS Mail Native — Фаза 0, шип скорости (TASK-N01).
//!
//! Цель: доказать, что нативная оболочка Win32 решает проблему «рамка летит,
//! содержимое тормозит», из-за которой POS Mail на WebView2 переписывался
//! четырежды (TASK-099…108).
//!
//! Что в окне:
//! - дерево папок (TreeView, слева);
//! - список писем — ВИРТУАЛЬНЫЙ ListView на 50 000 строк (LVS_OWNERDATA):
//!   данные отдаются по запросу LVN_GETDISPINFO, элементы не хранятся
//!   контролом — переложение при ресайзе не зависит от объёма;
//! - панель письма — WebView2 (webview2-com), только для HTML-тела письма;
//! - статус-бар.
//!
//! Ресайз ведёт системная рамка Windows; раскладку панелей делает WM_SIZE
//! через SetWindowPos (микросекунды, GDI), WebView2 обновляется bounds'ом.
//! DOM в кадре ресайза нет — тормозить нечему.
//!
//! Код написан под windows 0.62 / webview2-com 0.39 (версии wry 0.57).
//! Если при сборке сигнатуры не совпадут — править минимально, по подсказкам
//! cargo (см. docs/WORKFLOW.md).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::mpsc;

use webview2_com::{
    CreateCoreWebView2ControllerCompletedHandler, CreateCoreWebView2EnvironmentCompletedHandler,
    CoreWebView2EnvironmentOptions, Microsoft::Web::WebView2::Win32::*, wait_with_pump,
};
use windows::core::{Error, HSTRING, PCWSTR, Result, w};
use windows::Win32::Foundation::{
    E_POINTER, E_UNEXPECTED, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, GetStockObject, PAINTSTRUCT, DEFAULT_GUI_FONT,
};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{
    HTREEITEM, ICC_BAR_CLASSES, ICC_LISTVIEW_CLASSES, ICC_TREEVIEW_CLASSES, INITCOMMONCONTROLSEX,
    InitCommonControlsEx, LVCOLUMNW, LVIF_TEXT, LVM_INSERTCOLUMNW, LVM_SETITEMCOUNT,
    LVM_SETEXTENDEDLISTVIEWSTYLE, LVN_GETDISPINFOW, LVS_EX_DOUBLEBUFFER, LVS_EX_FULLROWSELECT,
    LVS_OWNERDATA, LVS_REPORT, LVS_SHOWSELALWAYS, NM_DBLCLK, NMHDR, NMLVDISPINFOW,
    NMITEMACTIVATE, TVIF_TEXT, TVINSERTSTRUCTW, TVINSERTSTRUCTW_0, TVI_LAST, TVI_ROOT, TVITEMW,
    TVM_INSERTITEMW, WC_LISTVIEW, WC_TREEVIEW,
};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW,
    DispatchMessageW, GetClientRect, GetMessageW, GWLP_USERDATA, GetWindowLongPtrW, IDC_ARROW,
    LoadCursorW, MINMAXINFO, MSG, PostQuitMessage, RegisterClassW, SWP_NOZORDER, SW_SHOW,
    SendMessageW, SetWindowLongPtrW, SetWindowPos, ShowWindow, TranslateMessage, WINDOW_EX_STYLE,
    WINDOW_STYLE, WM_DESTROY, WM_GETMINMAXINFO, WM_NCCREATE, WM_NOTIFY, WM_PAINT, WM_SETFONT,
    WM_SIZE, WNDCLASSW, WS_CHILD, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
};

/// Сообщение статус-бара (SB_SETTEXTW из commctrl: WM_USER + 11).
const SB_SET_TEXT: u32 = 0x040B;

const WINDOW_CLASS: PCWSTR = w!("SorokaNativeWnd");
const STATUSBAR_CLASS: PCWSTR = w!("msctls_statusbar32");

/// Ширины панелей раскладки, px.
const TREE_W: i32 = 240;
const PANE_W: i32 = 420;
const STATUS_H: i32 = 26;
const ROWS: usize = 50_000;

/// Строка «письма» для виртуального списка.
struct Row {
    from: String,
    subject: String,
    date: String,
}

/// Состояние приложения: одно окно, один поток.
struct AppState {
    rows: Vec<Row>,
    hwnd_tree: HWND,
    hwnd_list: HWND,
    hwnd_status: HWND,
    controller: Option<ICoreWebView2Controller>,
    webview: Option<ICoreWebView2>,
}

impl AppState {
    fn new(rows: Vec<Row>) -> Self {
        Self {
            rows,
            hwnd_tree: HWND::default(),
            hwnd_list: HWND::default(),
            hwnd_status: HWND::default(),
            controller: None,
            webview: None,
        }
    }
}

fn make_rows(n: usize) -> Vec<Row> {
    (0..n)
        .map(|i| Row {
            from: format!("Отправитель {} <user{}@example.com>", i % 997, i),
            subject: format!(
                "Тема письма №{} — виртуальный список не боится объёма",
                i
            ),
            date: format!("18.11.2026 {:02}:{:02}", (i / 60) % 24, i % 60),
        })
        .collect()
}

/// HTML «тела письма» для панели WebView2.
fn letter_html(row: &Row) -> String {
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><style>
body {{ font-family: 'Segoe UI', sans-serif; background: #1c1e24; color: #d6d9e0; margin: 24px; }}
h1 {{ font-size: 17px; font-weight: 600; margin: 0 0 6px; }}
.meta {{ color: #8b93a3; font-size: 13px; margin-bottom: 18px; }}
p {{ line-height: 1.55; max-width: 62ch; font-size: 14px; }}
.note {{ border-left: 3px solid #4c7ad0; padding-left: 12px; color: #a9b4c9; }}
</style></head><body>
<h1>{}</h1>
<div class="meta">{} · {}</div>
<p>Это панель письма — единственное место, где работает WebView2. Оболочка —
дерево, список на 50 000 строк, рамка, статус-бар — рисуется чистым Win32/GDI
и не зависит от этого движка.</p>
<p class="note">Тяни край окна: список переложится мгновенно при любом объёме.
Двойной клик по строке открывает «письмо» здесь.</p>
<p>С уважением, проверка скорости.</p>
</body></html>"#,
        row.subject, row.from, row.date
    )
}

/// Копирует UTF-16 строку в буфер элемента списка (с нулём на конце).
fn copy_utf16(dst: windows::core::PWSTR, cap: i32, text: &str) {
    let cap = cap.max(0) as usize;
    if cap == 0 {
        return;
    }
    let mut written = 0usize;
    for (i, unit) in text.encode_utf16().take(cap - 1).enumerate() {
        unsafe { *dst.0.add(i) = unit };
        written = i + 1;
    }
    unsafe { *dst.0.add(written) = 0 };
}

/// Раскладка панелей. Вызывается из WM_SIZE и после создания.
unsafe fn layout(hwnd: HWND, st: &AppState) {
    let mut rc = RECT::default();
    if unsafe { GetClientRect(hwnd, &mut rc) }.is_err() {
        return;
    }
    let w = rc.right;
    let h = rc.bottom;
    let h_main = (h - STATUS_H).max(0);
    if !st.hwnd_tree.0.is_null() {
        unsafe {
            let _ = SetWindowPos(st.hwnd_tree, None, 0, 0, TREE_W, h_main, SWP_NOZORDER);
        }
    }
    if !st.hwnd_list.0.is_null() {
        let list_w = (w - TREE_W - PANE_W).max(120);
        unsafe {
            let _ = SetWindowPos(
                st.hwnd_list,
                None,
                TREE_W,
                0,
                list_w,
                h_main,
                SWP_NOZORDER,
            );
        }
    }
    if !st.hwnd_status.0.is_null() && h > STATUS_H {
        unsafe {
            let _ = SetWindowPos(st.hwnd_status, None, 0, h - STATUS_H, w, STATUS_H, SWP_NOZORDER);
        }
    }
    if let Some(ctrl) = &st.controller {
        let left = (w - PANE_W).max(TREE_W + 120);
        let _ = unsafe {
            ctrl.SetBounds(RECT {
                left,
                top: 0,
                right: left + PANE_W,
                bottom: h_main,
            })
        };
    }
}

/// Текст в статус-бар.
unsafe fn status_text(st: &AppState, text: &str) {
    let mut buf: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        SendMessageW(
            st.hwnd_status,
            SB_SET_TEXT,
            Some(WPARAM(0)),
            Some(LPARAM(buf.as_mut_ptr() as isize)),
        );
    }
}

/// `webview2-com` 0.39 отдаёт собственный `Error`; `From` в `windows::core::Error`
/// нет — приводим вручную.
fn wv_err(err: webview2_com::Error) -> Error {
    match err {
        webview2_com::Error::WindowsError(e) => e,
        other => Error::new(E_UNEXPECTED, other.to_string()),
    }
}

/// Создание WebView2 (среда + контроллер) на родительском окне.
/// Паттерн — как в wry 0.57: замыкания-обработчики + wait_with_pump.
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
    // Контроллер приходит скрытым (IsVisible=false): без явного показа панель
    // остаётся чёрной — проверено на 157.0.4341.46.
    unsafe { controller.SetIsVisible(true)? };
    Ok((controller, webview))
}

unsafe fn lv_column(lv: HWND, index: usize, title: &str, cx: i32) {
    let mut buf: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    let col = LVCOLUMNW {
        mask: windows::Win32::UI::Controls::LVCF_TEXT
            | windows::Win32::UI::Controls::LVCF_WIDTH,
        pszText: windows::core::PWSTR(buf.as_mut_ptr()),
        cchTextMax: buf.len() as i32,
        cx,
        ..Default::default()
    };
    unsafe {
        SendMessageW(
            lv,
            LVM_INSERTCOLUMNW,
            Some(WPARAM(index)),
            Some(LPARAM(&col as *const LVCOLUMNW as isize)),
        );
    }
}

unsafe fn tv_insert(tv: HWND, parent: HTREEITEM, text: &str) {
    let mut buf: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let ins = TVINSERTSTRUCTW {
        hParent: parent,
        hInsertAfter: TVI_LAST,
        Anonymous: TVINSERTSTRUCTW_0 {
            item: TVITEMW {
                mask: TVIF_TEXT,
                pszText: windows::core::PWSTR(buf.as_mut_ptr()),
                cchTextMax: buf.len() as i32,
                ..Default::default()
            },
        },
    };
    unsafe {
        SendMessageW(
            tv,
            TVM_INSERTITEMW,
            None,
            Some(LPARAM(&ins as *const TVINSERTSTRUCTW as isize)),
        );
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
            let cs = unsafe { &*(lparam.0 as *const CREATESTRUCTW) };
            unsafe {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_GETMINMAXINFO => {
            let mmi = unsafe { &mut *(lparam.0 as *mut MINMAXINFO) };
            mmi.ptMinTrackSize = POINT { x: 960, y: 600 };
            LRESULT(0)
        }
        WM_SIZE => {
            let st = unsafe { &mut *(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppState) };
            unsafe { layout(hwnd, st) };
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_NOTIFY => {
            let hdr = unsafe { &*(lparam.0 as *const NMHDR) };
            let st = unsafe { &mut *(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppState) };
            if hdr.hwndFrom == st.hwnd_list && hdr.code == LVN_GETDISPINFOW {
                let nm = unsafe { &mut *(lparam.0 as *mut NMLVDISPINFOW) };
                let i = nm.item.iItem as usize;
                if nm.item.mask.contains(LVIF_TEXT) && i < st.rows.len() {
                    let row = &st.rows[i];
                    let text = match nm.item.iSubItem {
                        0 => row.from.as_str(),
                        1 => row.subject.as_str(),
                        _ => row.date.as_str(),
                    };
                    copy_utf16(nm.item.pszText, nm.item.cchTextMax, text);
                }
                return LRESULT(0);
            }
            if hdr.hwndFrom == st.hwnd_list && hdr.code == NM_DBLCLK {
                let nm = unsafe { &*(lparam.0 as *const NMITEMACTIVATE) };
                if nm.iItem >= 0 {
                    let i = nm.iItem as usize;
                    if let Some(row) = st.rows.get(i) {
                        if let Some(wv) = &st.webview {
                            let _ = unsafe {
                                wv.NavigateToString(&HSTRING::from(letter_html(row)))
                            };
                        }
                        unsafe { status_text(st, &format!("Открыто: {}", row.subject)) };
                    }
                }
                return LRESULT(0);
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
    // Владение сознательно передаётся окну: приложение однопоточное, живёт
    // до выхода — для шипа освобождение не критично.
    let state = Box::into_raw(Box::new(AppState::new(make_rows(ROWS))));
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            WINDOW_CLASS,
            w!("POS Mail — нативная оболочка (Фаза 0)"),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1280,
            800,
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
        WINDOW_STYLE(
            LVS_REPORT | LVS_OWNERDATA | LVS_SHOWSELALWAYS,
        ),
    )?;
    st.hwnd_status = child(STATUSBAR_CLASS, WINDOW_STYLE(0))?;

    // Шрифт по умолчанию всем детям.
    let hfont = unsafe { GetStockObject(DEFAULT_GUI_FONT) };
    for h in [st.hwnd_tree, st.hwnd_list, st.hwnd_status] {
        unsafe {
            SendMessageW(h, WM_SETFONT, Some(WPARAM(hfont.0 as usize)), Some(LPARAM(1)));
        }
    }

    // Колонки и режим виртуального списка.
    unsafe {
        lv_column(st.hwnd_list, 0, "От", 260);
        lv_column(st.hwnd_list, 1, "Тема", 480);
        lv_column(st.hwnd_list, 2, "Дата", 150);
        SendMessageW(
            st.hwnd_list,
            LVM_SETEXTENDEDLISTVIEWSTYLE,
            Some(WPARAM(0)),
            Some(LPARAM(
                (LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER) as isize
            )),
        );
        SendMessageW(
            st.hwnd_list,
            LVM_SETITEMCOUNT,
            Some(WPARAM(st.rows.len())),
            None,
        );
    }

    // Дерево папок.
    unsafe {
        tv_insert(st.hwnd_tree, TVI_ROOT, "Входящие");
        tv_insert(st.hwnd_tree, TVI_ROOT, "Отправленные");
        tv_insert(st.hwnd_tree, TVI_ROOT, "Черновики");
        tv_insert(st.hwnd_tree, TVI_ROOT, "Корзина");
        tv_insert(st.hwnd_tree, TVI_ROOT, "Архив");
        tv_insert(st.hwnd_tree, TVI_ROOT, "Спам");
    }

    // Панель письма: WebView2.
    let (controller, webview) = unsafe { create_webview(hwnd)? };
    st.controller = Some(controller);
    st.webview = Some(webview);
    if let Some(row) = st.rows.first() {
        unsafe {
            let _ = st
                .webview
                .as_ref()
                .unwrap()
                .NavigateToString(&HSTRING::from(letter_html(row)));
        }
    }

    unsafe {
        layout(hwnd, st);
        status_text(st, "50 000 писем в виртуальном списке — тяни край окна");
        let _ = ShowWindow(hwnd, SW_SHOW);
    }

    // Цикл сообщений.
    let mut msg = MSG::default();
    loop {
        let r = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if r.0 <= 0 {
            break;
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
