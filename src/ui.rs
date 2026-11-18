//! Оболочка главного окна: меню, дерево папок, виртуальный список писем,
//! панель письма (HTML-шаблон), статус-бар, применение событий бэкенда.
//!
//! Все функции вызываются только из UI-потока. Блокирующие операции уходят
//! в `backend` (рабочий поток), сюда возвращаются события через WM_APP_MAIL.

use windows::Win32::Foundation::{
    E_UNEXPECTED, HINSTANCE, HMODULE, HWND, LPARAM, LRESULT, RECT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontIndirectW, CreateSolidBrush, DT_END_ELLIPSIS, DT_NOPREFIX, DT_SINGLELINE,
    DT_VCENTER, DeleteObject, DrawTextW, EndPaint, FillRect, HFONT, HGDIOBJ, InvalidateRect,
    LOGFONTW, PAINTSTRUCT, SelectObject, SetBkColor, SetTextColor,
};
use windows::Win32::UI::Controls::{
    CDDS_ITEMPREPAINT, CDDS_PREPAINT, CDIS_SELECTED, CDRF_DODEFAULT, CDRF_NOTIFYITEMDRAW,
    CDRF_NOTIFYSUBITEMDRAW, HTREEITEM, LVCF_TEXT, LVCF_WIDTH, LVCOLUMNW, LVIF_TEXT,
    LVM_DELETECOLUMN, LVM_INSERTCOLUMNW, LVM_SETBKCOLOR, LVM_SETEXTENDEDLISTVIEWSTYLE,
    LVM_SETITEMCOUNT, LVM_SETTEXTBKCOLOR, LVM_SETTEXTCOLOR, LVS_EX_DOUBLEBUFFER,
    LVS_EX_FULLROWSELECT, NMLISTVIEW, NMLVCUSTOMDRAW, NMLVDISPINFOW, NMTREEVIEWW, TVE_EXPAND,
    TVGN_CARET, TVI_LAST, TVI_ROOT, TVIF_PARAM, TVIF_TEXT, TVINSERTSTRUCTW, TVINSERTSTRUCTW_0,
    TVITEMW, TVM_DELETEITEM, TVM_EXPAND, TVM_INSERTITEMW, TVM_SELECTITEM, TVM_SETBKCOLOR,
    TVM_SETLINECOLOR, TVM_SETTEXTCOLOR, WM_MOUSELEAVE,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, SetFocus, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
};
use windows::Win32::UI::WindowsAndMessaging::{
    ACCEL, AppendMenuW, CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, CreateAcceleratorTableW, CreateMenu,
    CreatePopupMenu, DefWindowProcW, FCONTROL, FNOINVERT, FSHIFT, FVIRTKEY, GWLP_USERDATA,
    GetClientRect, GetParent, GetSystemMetrics, GetWindowLongPtrW, GetWindowPlacement,
    GetWindowTextLengthW, GetWindowTextW, HACCEL, IDC_ARROW, IDC_SIZEWE, KillTimer, LoadCursorW,
    MENU_ITEM_FLAGS, MF_POPUP, MF_SEPARATOR, MF_STRING, NONCLIENTMETRICSW, RegisterClassW,
    SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SW_HIDE, SW_SHOW,
    SW_SHOWMAXIMIZED, SendMessageW, SetMenu, SetWindowLongPtrW, SetWindowPlacement, ShowWindow,
    WINDOWPLACEMENT, WM_CAPTURECHANGED, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MOUSEMOVE, WM_NCCREATE, WM_PAINT, WM_SIZE, WNDCLASSW,
};
use windows::core::{Error, HSTRING, PCWSTR, w};

use crate::backend::{BodyData, MailEvent};
use crate::dates;
use crate::layout_store;
use crate::mail::accounts::{load_accounts, split_key};
use crate::mail::imap::MessageHeader;
use crate::mail::store::Store;
use crate::state::{AppState, ListRow, PaneBody, TreeSlot};
use crate::theme;
use crate::wide::{escape_html, set_window_text, wide};

/// Раскладка, px (TASK-N04): дерево | сплиттер | список | сплиттер | тело.
pub const STATUS_H: i32 = 26;
/// Высота полосы поиска над списком (TASK-N03).
pub const SEARCH_H: i32 = 30;
/// Ширина полосы сплиттера — как .splitter-v в Soroka (6px).
pub const SPLIT_W: i32 = 6;
/// Минимальная ширина панели при перетаскивании — clamp из dragSplit Soroka (110px).
pub const PANE_MIN: i32 = 110;

// Веса панелей по умолчанию переехали в layout_store (TASK-N06: это их
// дом — layout.json); переиспользуем для state.rs и сплиттеров.
pub use crate::layout_store::{WF_DEF, WL_DEF, WR_DEF};

/// Идентификаторы команд меню.
pub const IDM_EXIT: u16 = 100;
pub const IDM_ACCOUNT_ADD: u16 = 101;
pub const IDM_SYNC: u16 = 110;
pub const IDM_COMPOSE: u16 = 120;
pub const IDM_REPLY: u16 = 121;
pub const IDM_REPLY_ALL: u16 = 122;
pub const IDM_FORWARD: u16 = 123;
pub const IDM_DELETE: u16 = 124;
pub const IDM_TOGGLE_SEEN: u16 = 125;
pub const IDM_SEARCH_FOCUS: u16 = 130;
pub const IDM_SEARCH_GO: u16 = 131;
pub const IDM_SEARCH_CLOSE: u16 = 132;
pub const IDM_THEME: u16 = 133;
pub const IDM_SETTINGS: u16 = 134;

/// EN_CHANGE — уведомление EDIT о вводе (приходит через WM_COMMAND).
/// В windows-rs константа лежит в WindowsAndMessaging — локальный псевдоним надёжен.
pub const EN_CHANGE: u32 = 0x0300;
/// Идентификатор таймера живого поиска (debounce 300 мс, как в Soroka).
pub const IDT_SEARCH: usize = 1;
/// EM_SETSEL (0x00B1) — выделение текста в EDIT.
const EM_SETSEL: u32 = 0x00B1;

/// Копирует UTF-16 строку в буфер элемента списка (с нулём на конце).
pub fn copy_utf16(dst: windows::core::PWSTR, cap: i32, text: &str) {
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

/// Текст в статус-бар (с TASK-N05 — собственное окно SorokaNativeStatus,
/// красится палитрой темы; текст хранится в заголовке окна через
/// SetWindowTextW/GetWindowTextW).
pub fn status_text(st: &AppState, text: &str) {
    let _ = set_window_text(st.hwnd_status, text);
}

/// Системный шрифт интерфейса + его полужирная версия (для непрочитанных).
/// NONCLIENTMETRICS даёт правильный шрифт под DPI и тему системы.
pub fn create_fonts() -> (HFONT, HFONT) {
    unsafe {
        let mut metrics = NONCLIENTMETRICSW {
            cbSize: std::mem::size_of::<NONCLIENTMETRICSW>() as u32,
            ..Default::default()
        };
        let ok = windows::Win32::UI::WindowsAndMessaging::SystemParametersInfoW(
            windows::Win32::UI::WindowsAndMessaging::SPI_GETNONCLIENTMETRICS,
            metrics.cbSize,
            Some(&mut metrics as *mut NONCLIENTMETRICSW as *mut std::ffi::c_void),
            windows::Win32::UI::WindowsAndMessaging::SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
        .is_ok();
        let lf: LOGFONTW = if ok {
            metrics.lfMessageFont
        } else {
            LOGFONTW::default()
        };
        let normal = CreateFontIndirectW(&lf);
        let mut bold_lf = lf;
        bold_lf.lfWeight = 600; // FW_SEMIBOLD
        let bold = CreateFontIndirectW(&bold_lf);
        (normal, bold)
    }
}

/// Меню + таблица ускорителей. Возвращает HACCEL для цикла сообщений.
/// CreateAcceleratorTableW на x64 требует выравнивания буфера >= 4 байт
/// (golang/go#64729); [ACCEL; _] выровнен на 2 — вызов эпизодически падал
/// с ERROR_NOACCESS (0x800703E6) и приложение молча закрывалось.
/// Оборачиваем таблицу в align(8).
#[repr(C, align(8))]
struct AccelTable([ACCEL; 9]);

pub fn create_menu(hwnd: HWND) -> windows::core::Result<HACCEL> {
    unsafe {
        let bar = CreateMenu()?;
        let mail = CreatePopupMenu()?;
        let folder = CreatePopupMenu()?;
        let view = CreatePopupMenu()?;
        let accounts = CreatePopupMenu()?;
        let file = CreatePopupMenu()?;

        let item = |menu, flags: MENU_ITEM_FLAGS, id: usize, text: PCWSTR| {
            AppendMenuW(menu, flags, id, text).ok();
        };

        item(
            mail,
            MF_STRING,
            IDM_COMPOSE as usize,
            w!("Написать письмо...\tCtrl+N"),
        );
        item(mail, MF_STRING, IDM_REPLY as usize, w!("Ответить\tCtrl+R"));
        item(
            mail,
            MF_STRING,
            IDM_REPLY_ALL as usize,
            w!("Ответить всем\tCtrl+Shift+R"),
        );
        item(
            mail,
            MF_STRING,
            IDM_FORWARD as usize,
            w!("Переслать...\tCtrl+F"),
        );
        item(mail, MF_SEPARATOR, 0, PCWSTR::null());
        item(mail, MF_STRING, IDM_DELETE as usize, w!("Удалить\tDel"));
        item(
            mail,
            MF_STRING,
            IDM_TOGGLE_SEEN as usize,
            w!("Прочитано / не прочитано\tCtrl+U"),
        );

        item(
            folder,
            MF_STRING,
            IDM_SEARCH_FOCUS as usize,
            w!("Найти в ящике...\t/"),
        );
        item(
            folder,
            MF_STRING,
            IDM_SYNC as usize,
            w!("Синхронизировать\tF5"),
        );

        // TASK-N05: переключение темы. Галочка ставится/снимается при старте
        // и переключении (CheckMenuItem в main.rs); с N09 тема хранится в
        // общем settings.json (native_theme), за диалогом — TASK-N09.
        item(view, MF_STRING, IDM_THEME as usize, w!("Тёмная тема"));

        item(
            accounts,
            MF_STRING,
            IDM_ACCOUNT_ADD as usize,
            w!("Добавить ящик..."),
        );

        item(file, MF_STRING, IDM_SETTINGS as usize, w!("Настройки..."));
        item(file, MF_SEPARATOR, 0, PCWSTR::null());
        item(file, MF_STRING, IDM_EXIT as usize, w!("Выход"));

        item(bar, MF_POPUP, mail.0 as usize, w!("&Почта"));
        item(bar, MF_POPUP, folder.0 as usize, w!("&Папка"));
        item(bar, MF_POPUP, view.0 as usize, w!("&Вид"));
        item(bar, MF_POPUP, accounts.0 as usize, w!("&Ящики"));
        item(bar, MF_POPUP, file.0 as usize, w!("&Файл"));

        SetMenu(hwnd, Some(bar))?;

        let accels = AccelTable([
            ACCEL {
                fVirt: FVIRTKEY | FCONTROL | FNOINVERT,
                key: 0x4E, // N
                cmd: IDM_COMPOSE,
            },
            ACCEL {
                fVirt: FVIRTKEY | FCONTROL | FNOINVERT,
                key: 0x52, // R
                cmd: IDM_REPLY,
            },
            ACCEL {
                fVirt: FVIRTKEY | FCONTROL | FSHIFT | FNOINVERT,
                key: 0x52, // R
                cmd: IDM_REPLY_ALL,
            },
            ACCEL {
                fVirt: FVIRTKEY | FCONTROL | FNOINVERT,
                key: 0x46, // F
                cmd: IDM_FORWARD,
            },
            ACCEL {
                fVirt: FVIRTKEY | FNOINVERT,
                key: 0x2E, // Delete
                cmd: IDM_DELETE,
            },
            ACCEL {
                fVirt: FVIRTKEY | FCONTROL | FNOINVERT,
                key: 0x55, // U
                cmd: IDM_TOGGLE_SEEN,
            },
            ACCEL {
                fVirt: FVIRTKEY | FNOINVERT,
                key: 0x74, // F5
                cmd: IDM_SYNC,
            },
            ACCEL {
                // «/» — как в Soroka (TASK-080); VK_OEM_2 не зависит от раскладки.
                fVirt: FVIRTKEY | FNOINVERT,
                key: 0xBF,
                cmd: IDM_SEARCH_FOCUS,
            },
            ACCEL {
                fVirt: FVIRTKEY | FNOINVERT,
                key: 0x1B, // Esc — закрыть поиск (вне поля — игнорируется)
                cmd: IDM_SEARCH_CLOSE,
            },
        ]);
        CreateAcceleratorTableW(&accels.0)
    }
}

// ===== Сплиттеры панелей (TASK-N04) =====

/// Контекст сплиттера (в GWLP_USERDATA, живёт до конца процесса).
pub struct SplitCtx {
    /// Главное окно (его AppState меняем, ему шлём WM_SIZE).
    pub main: HWND,
    /// true — граница «список|тело» (панель письма прячется на время жеста),
    /// false — «дерево|список».
    pub pane: bool,
    /// Идёт перетаскивание (мышь захвачена).
    pub captured: bool,
    /// Курсор над сплиттером (подсветка полосы-хвата).
    pub hover: bool,
    /// Запрошен ли WM_MOUSELEAVE (TrackMouseEvent).
    pub tracked: bool,
    /// Текущий жест: стартовый x (клиентские координаты полосы), ширина
    /// левой панели и сумма двух соседних панелей (px и веса).
    pub start_x: i32,
    pub start_a_px: i32,
    pub sum_px: i32,
    pub sum_w: f64,
}

const SPLITTER_CLASS: PCWSTR = w!("SorokaNativeSplitter");

/// Регистрация класса сплиттера (один раз из run(), до создания детей).
pub fn register_splitter_class(hmod: HMODULE) -> windows::core::Result<()> {
    let wc = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(splitter_proc),
        hInstance: HINSTANCE(hmod.0),
        hCursor: unsafe { LoadCursorW(None, IDC_SIZEWE)? },
        lpszClassName: SPLITTER_CLASS,
        ..Default::default()
    };
    if unsafe { RegisterClassW(&wc) } == 0 {
        return Err(Error::from(E_UNEXPECTED));
    }
    Ok(())
}

/// Оконная процедура сплиттера: перетаскивание меняет веса соседних панелей
/// (математика dragSplit из Soroka — сумма весов пары сохраняется, минимум
/// панели 110px) и шлёт главному окну WM_SIZE: дерево и список переложиваются
/// сразу. Bounds панели письма обновляются после жеста (layout() при
/// st.dragging == false).
unsafe extern "system" fn splitter_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let ctx = unsafe { (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut SplitCtx).as_mut() };
    match msg {
        WM_NCCREATE => {
            let cs = unsafe { &*(lparam.0 as *const CREATESTRUCTW) };
            unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize) };
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_PAINT => {
            let Some(ctx) = ctx else {
                return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
            };
            let mut ps = PAINTSTRUCT::default();
            let hdc = unsafe { BeginPaint(hwnd, &mut ps) };
            let mut rc = RECT::default();
            let _ = unsafe { GetClientRect(hwnd, &mut rc) };
            // Палитра темы (TASK-N05): фон — как у панелей, хват — штрих,
            // при наведении/жесте — акцент Soroka (вместо системных цветов,
            // долг из TASK-N04).
            let st = unsafe { &*(GetWindowLongPtrW(ctx.main, GWLP_USERDATA) as *const AppState) };
            let p = theme::palette(st.dark);
            let bg = unsafe { CreateSolidBrush(p.bg) };
            unsafe { FillRect(hdc, &rc, bg) };
            // Полоса-хват 3×36px по центру — как .splitter-v::after в Soroka.
            let cx = (rc.left + rc.right) / 2;
            let cy = (rc.top + rc.bottom) / 2;
            let line = RECT {
                left: cx - 1,
                top: cy - 18,
                right: cx + 2,
                bottom: cy + 18,
            };
            let grip_color = if ctx.hover || ctx.captured {
                p.accent
            } else {
                p.stroke
            };
            let grip = unsafe { CreateSolidBrush(grip_color) };
            unsafe { FillRect(hdc, &line, grip) };
            unsafe {
                let _ = DeleteObject(HGDIOBJ(bg.0));
                let _ = DeleteObject(HGDIOBJ(grip.0));
                let _ = EndPaint(hwnd, &ps);
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let Some(ctx) = ctx else {
                return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
            };
            if !ctx.tracked {
                ctx.tracked = true;
                let mut tme = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                let _ = unsafe { TrackMouseEvent(&mut tme) };
            }
            if !ctx.hover {
                ctx.hover = true;
                let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
            }
            if ctx.captured {
                // dx — клиентские координаты самой полосы: capture доставляет
                // WM_MOUSEMOVE и за пределами окна, координаты остаются корректными.
                let x = ((lparam.0 as usize) & 0xFFFF) as u16 as i16 as i32;
                let st =
                    unsafe { &mut *(GetWindowLongPtrW(ctx.main, GWLP_USERDATA) as *mut AppState) };
                let pa =
                    (ctx.start_a_px + (x - ctx.start_x)).clamp(PANE_MIN, ctx.sum_px - PANE_MIN);
                let wa = ctx.sum_w * pa as f64 / ctx.sum_px as f64;
                if ctx.pane {
                    st.wl = wa;
                    st.wr = ctx.sum_w - wa;
                } else {
                    st.wf = wa;
                    st.wl = ctx.sum_w - wa;
                }
                st.dragging = true;
                let _ =
                    unsafe { SendMessageW(ctx.main, WM_SIZE, Some(WPARAM(0)), Some(LPARAM(0))) };
            }
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            if let Some(ctx) = ctx {
                ctx.hover = false;
                ctx.tracked = false;
                let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let Some(ctx) = ctx else {
                return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
            };
            unsafe { SetCapture(hwnd) };
            ctx.captured = true;
            ctx.start_x = ((lparam.0 as usize) & 0xFFFF) as u16 as i16 as i32;
            let st = unsafe { &mut *(GetWindowLongPtrW(ctx.main, GWLP_USERDATA) as *mut AppState) };
            if ctx.pane {
                ctx.start_a_px = st.list_px;
                ctx.sum_px = st.list_px + st.pane_px;
                ctx.sum_w = st.wl + st.wr;
                // Панель письма не участвует в кадре жеста: прячем до отпускания.
                if let Some(ctrl) = &st.controller {
                    let _ = unsafe { ctrl.SetIsVisible(false) };
                }
            } else {
                ctx.start_a_px = st.tree_px;
                ctx.sum_px = st.tree_px + st.list_px;
                ctx.sum_w = st.wf + st.wl;
            }
            let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
            LRESULT(0)
        }
        WM_LBUTTONUP | WM_CAPTURECHANGED => {
            let Some(ctx) = ctx else {
                return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
            };
            if ctx.captured {
                ctx.captured = false;
                if msg == WM_LBUTTONUP {
                    let _ = unsafe { ReleaseCapture() };
                }
                let st =
                    unsafe { &mut *(GetWindowLongPtrW(ctx.main, GWLP_USERDATA) as *mut AppState) };
                st.dragging = false;
                if ctx.pane
                    && let Some(ctrl) = &st.controller
                {
                    let _ = unsafe { ctrl.SetIsVisible(true) };
                }
                // Финальная раскладка: панель письма получает новые bounds.
                let _ =
                    unsafe { SendMessageW(ctx.main, WM_SIZE, Some(WPARAM(0)), Some(LPARAM(0))) };
                layout_store::save(st.wf, st.wl, st.wr);
                let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
            }
            LRESULT(0)
        }
        WM_LBUTTONDBLCLK => {
            let Some(ctx) = ctx else {
                return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
            };
            // Сброс весов соседней пары к дефолту — как dblclick в Soroka.
            let st = unsafe { &mut *(GetWindowLongPtrW(ctx.main, GWLP_USERDATA) as *mut AppState) };
            if ctx.pane {
                st.wl = WL_DEF;
                st.wr = WR_DEF;
            } else {
                st.wf = WF_DEF;
                st.wl = WL_DEF;
            }
            let _ = unsafe { SendMessageW(ctx.main, WM_SIZE, Some(WPARAM(0)), Some(LPARAM(0))) };
            layout_store::save(st.wf, st.wl, st.wr);
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

// ===== Статус-бар (TASK-N05) =====

/// Класс собственного статус-бара: msctls_statusbar32 не даёт задать цвет
/// текста, тёмная тема на нём не собирается — красим сами (KISS-исключение
/// из «системные контролы как есть»: красится там, где система не умеет).
pub const STATUS_CLASS: PCWSTR = w!("SorokaNativeStatus");

/// Регистрация класса статус-бара (один раз из run(), до создания детей).
pub fn register_status_class(hmod: HMODULE) -> windows::core::Result<()> {
    let wc = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(status_proc),
        hInstance: HINSTANCE(hmod.0),
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW)? },
        lpszClassName: STATUS_CLASS,
        ..Default::default()
    };
    if unsafe { RegisterClassW(&wc) } == 0 {
        return Err(Error::from(E_UNEXPECTED));
    }
    Ok(())
}

/// Оконная процедура статус-бара: фон палитры, верхняя линия, текст
/// (хранится в заголовке окна — SetWindowTextW из status_text).
unsafe extern "system" fn status_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = unsafe { BeginPaint(hwnd, &mut ps) };
            let mut rc = RECT::default();
            let _ = unsafe { GetClientRect(hwnd, &mut rc) };
            // Палитра и шрифт — из AppState родителя (главного окна).
            let parent = unsafe { GetParent(hwnd) }.unwrap_or_default();
            let st = unsafe { &*(GetWindowLongPtrW(parent, GWLP_USERDATA) as *const AppState) };
            let p = theme::palette(st.dark);
            let bg = unsafe { CreateSolidBrush(p.bg) };
            let stroke = unsafe { CreateSolidBrush(p.stroke) };
            unsafe {
                FillRect(hdc, &rc, bg);
                // Верхняя граница — линия штриха палитры.
                FillRect(hdc, &RECT { bottom: 1, ..rc }, stroke);
                // Текст: вторичным цветом (статус — не главный контент).
                let len = GetWindowTextLengthW(hwnd).max(0) as usize;
                let mut buf = vec![0u16; len + 1];
                let got = GetWindowTextW(hwnd, &mut buf).max(0) as usize;
                if got > 0 {
                    let old = SelectObject(hdc, HGDIOBJ(st.font.0));
                    SetTextColor(hdc, p.text_dim);
                    SetBkColor(hdc, p.bg);
                    let mut trc = rc;
                    trc.left += 8;
                    trc.top += 1;
                    let _ = DrawTextW(
                        hdc,
                        &mut buf[..got],
                        &mut trc,
                        DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS,
                    );
                    SelectObject(hdc, old);
                }
                let _ = DeleteObject(HGDIOBJ(bg.0));
                let _ = DeleteObject(HGDIOBJ(stroke.0));
                let _ = EndPaint(hwnd, &ps);
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Применить тему к оболочке (TASK-N05): дерево, список, сплиттеры,
/// статус-бар, поле поиска. Вызывается при старте (до показа окна —
/// без вспышки системных цветов) и при переключении «Вид → Тёмная тема».
/// Панель письма пере-навигируется отдельно (main.rs: там нужен WebView2).
pub fn apply_theme(st: &mut AppState) {
    let p = theme::palette(st.dark);
    unsafe {
        // Дерево: фон, текст, линии — TVM-сообщения (выделение остаётся
        // системным, как и у списка: контрол красит его сам).
        SendMessageW(
            st.hwnd_tree,
            TVM_SETBKCOLOR,
            Some(WPARAM(0)),
            Some(LPARAM(p.bg.0 as isize)),
        );
        SendMessageW(
            st.hwnd_tree,
            TVM_SETTEXTCOLOR,
            Some(WPARAM(0)),
            Some(LPARAM(p.text.0 as isize)),
        );
        SendMessageW(
            st.hwnd_tree,
            TVM_SETLINECOLOR,
            Some(WPARAM(0)),
            Some(LPARAM(p.stroke.0 as isize)),
        );
        // Список: фон и текст; колонка даты затемняется в custom draw.
        SendMessageW(
            st.hwnd_list,
            LVM_SETBKCOLOR,
            Some(WPARAM(0)),
            Some(LPARAM(p.bg.0 as isize)),
        );
        SendMessageW(
            st.hwnd_list,
            LVM_SETTEXTCOLOR,
            Some(WPARAM(0)),
            Some(LPARAM(p.text.0 as isize)),
        );
        SendMessageW(
            st.hwnd_list,
            LVM_SETTEXTBKCOLOR,
            Some(WPARAM(0)),
            Some(LPARAM(p.bg.0 as isize)),
        );
    }
    // Кисть фона поля поиска (WM_CTLCOLOREDIT): прежнюю удалить.
    if !st.hbr_edit.is_invalid() {
        let _ = unsafe { DeleteObject(HGDIOBJ(st.hbr_edit.0)) };
    }
    st.hbr_edit = unsafe { CreateSolidBrush(p.surface) };
    for h in [
        st.hwnd_tree,
        st.hwnd_list,
        st.hwnd_status,
        st.hwnd_split_tree,
        st.hwnd_split_pane,
        st.hwnd_search,
    ] {
        unsafe {
            let _ = InvalidateRect(Some(h), None, true);
        }
    }
}

// ===== Геометрия окон (TASK-N06) =====

/// Снять геометрию окна для layout.json: нормальная позиция/размер
/// (WINDOWPLACEMENT корректен и для развёрнутого окна — отдаёт прямоугольник
/// НЕразвёрнутого состояния) и флаг «развёрнуто».
pub fn snapshot_placement(hwnd: HWND) -> layout_store::WinPlacement {
    let mut wp = WINDOWPLACEMENT {
        length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
        ..Default::default()
    };
    let r = if unsafe { GetWindowPlacement(hwnd, &mut wp) }.is_ok() {
        wp.rcNormalPosition
    } else {
        RECT::default()
    };
    layout_store::WinPlacement {
        x: r.left,
        y: r.top,
        w: r.right - r.left,
        h: r.bottom - r.top,
        maximized: wp.showCmd == SW_SHOWMAXIMIZED.0 as u32,
    }
}

/// Восстановить геометрию окна (TASK-N06): нормальный прямоугольник —
/// через SetWindowPlacement (рабочие координаты, ровно как снято), окно
/// пока скрыто. Возвращает true, если окно было развёрнуто — показывать
/// тогда SW_SHOWMAXIMIZED.
pub fn restore_placement(hwnd: HWND, p: &layout_store::WinPlacement) -> bool {
    let mut rc = RECT {
        left: p.x,
        top: p.y,
        right: p.x + p.w.max(200),
        bottom: p.y + p.h.max(120),
    };
    clamp_to_virtual_screen(&mut rc);
    let wp = WINDOWPLACEMENT {
        length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
        rcNormalPosition: rc,
        showCmd: SW_HIDE.0 as u32,
        ..Default::default()
    };
    let _ = unsafe { SetWindowPlacement(hwnd, &wp) };
    p.maximized
}

/// Не дать окну улететь за экран (мониторы меняются между запусками):
/// на виртуальном рабочем столе должна остаться полоса не меньше 150×60.
fn clamp_to_virtual_screen(r: &mut RECT) {
    let (vx, vy, vw, vh) = unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    };
    if vw <= 0 || vh <= 0 {
        return;
    }
    let w = r.right - r.left;
    let h = r.bottom - r.top;
    r.left = r.left.clamp(vx - w + 150, vx + vw - 150);
    r.top = r.top.clamp(vy - h + 60, vy + vh - 60);
    r.right = r.left + w;
    r.bottom = r.top + h;
}

/// Колонки виртуального списка писем.
pub fn setup_list_columns(hwnd_list: HWND) {
    unsafe {
        lv_column(hwnd_list, 0, "Вл.", 44);
        lv_column(hwnd_list, 1, "От", 240);
        lv_column(hwnd_list, 2, "Тема", 460);
        lv_column(hwnd_list, 3, "Дата", 150);
        SendMessageW(
            hwnd_list,
            LVM_SETEXTENDEDLISTVIEWSTYLE,
            Some(WPARAM(0)),
            Some(LPARAM(
                (LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER) as isize,
            )),
        );
    }
}

unsafe fn lv_column(lv: HWND, index: usize, title: &str, cx: i32) {
    let mut buf = wide(title);
    let col = LVCOLUMNW {
        mask: LVCF_TEXT | LVCF_WIDTH,
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

/// Переключение колонок списка: режим папки (4 колонки) ⇄ режим поиска
/// (5, с колонкой «Папка»). Расширенные стили списка при этом не теряются.
pub fn set_list_columns(hwnd_list: HWND, search: bool) {
    let old = if search { 4 } else { 5 };
    unsafe {
        for _ in 0..old {
            SendMessageW(hwnd_list, LVM_DELETECOLUMN, Some(WPARAM(0)), None);
        }
    }
    if search {
        unsafe {
            lv_column(hwnd_list, 0, "Вл.", 44);
            lv_column(hwnd_list, 1, "От", 200);
            lv_column(hwnd_list, 2, "Тема", 350);
            lv_column(hwnd_list, 3, "Папка", 150);
            lv_column(hwnd_list, 4, "Дата", 140);
        }
    } else {
        unsafe {
            lv_column(hwnd_list, 0, "Вл.", 44);
            lv_column(hwnd_list, 1, "От", 240);
            lv_column(hwnd_list, 2, "Тема", 460);
            lv_column(hwnd_list, 3, "Дата", 150);
        }
    }
}

/// Вставка узла дерева с lParam = индекс слота; возвращает хэндл узла.
unsafe fn tv_insert(tv: HWND, parent: HTREEITEM, text: &str, slot: usize) -> HTREEITEM {
    let mut buf = wide(text);
    let ins = TVINSERTSTRUCTW {
        hParent: parent,
        hInsertAfter: TVI_LAST,
        Anonymous: TVINSERTSTRUCTW_0 {
            item: TVITEMW {
                mask: TVIF_TEXT | TVIF_PARAM,
                pszText: windows::core::PWSTR(buf.as_mut_ptr()),
                cchTextMax: buf.len() as i32,
                lParam: LPARAM(slot as isize),
                ..Default::default()
            },
        },
    };
    let res = unsafe {
        SendMessageW(
            tv,
            TVM_INSERTITEMW,
            None,
            Some(LPARAM(&ins as *const TVINSERTSTRUCTW as isize)),
        )
    };
    HTREEITEM(res.0)
}

/// Человеческое имя папки для дерева.
fn folder_display(name: &str) -> String {
    let n = name.to_lowercase();
    if n == "inbox" {
        return "Входящие".into();
    }
    if n.contains("sent") || n.contains("отправ") {
        return "Отправленные".into();
    }
    if n.contains("draft") || n.contains("чернов") {
        return "Черновики".into();
    }
    if n.contains("trash") || n.contains("корзин") || n.contains("deleted") {
        return "Корзина".into();
    }
    if n.contains("spam") || n.contains("junk") {
        return "Спам".into();
    }
    name.to_string()
}

/// Перестраивает дерево: ящики → папки из кэша. Выбирает первый INBOX.
pub fn rebuild_tree(hwnd_tv: HWND, st: &mut AppState) {
    st.accounts = load_accounts();
    st.tree_slots.clear();
    unsafe {
        SendMessageW(
            hwnd_tv,
            TVM_DELETEITEM,
            Some(WPARAM(0)),
            Some(LPARAM(TVI_ROOT.0)),
        );
    }
    let mut first_inbox: Option<HTREEITEM> = None;
    for acc in st.accounts.clone() {
        let acc_slot = st.tree_slots.len();
        st.tree_slots.push(TreeSlot::Account {
            id: acc.id.clone(),
            email: acc.email.clone(),
        });
        let h_acc = unsafe { tv_insert(hwnd_tv, TVI_ROOT, &acc.email, acc_slot) };

        let like = format!("{}::%", acc.id);
        let mut folders: Vec<String> = Store::open_default()
            .map(|s| {
                s.load_folders(&like)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|f| f.name)
                    .collect()
            })
            .unwrap_or_default();
        // INBOX первым, дальше по алфавиту — как refresh_folders в Soroka.
        folders.sort_by(|a, b| {
            let inbox = |n: &str| n.ends_with("::INBOX");
            inbox(b).cmp(&inbox(a)).then_with(|| a.cmp(b))
        });
        let inbox_key = format!("{}::INBOX", acc.id);
        for key in folders {
            let (_, name) = split_key(&key);
            let slot = st.tree_slots.len();
            st.tree_slots.push(TreeSlot::Folder {
                key: key.clone(),
                name: name.clone(),
            });
            let display = if key == inbox_key {
                "Входящие".to_string()
            } else {
                folder_display(&name)
            };
            let h = unsafe { tv_insert(hwnd_tv, h_acc, &display, slot) };
            if key == inbox_key && first_inbox.is_none() {
                first_inbox = Some(h);
            }
        }
        unsafe {
            SendMessageW(
                hwnd_tv,
                TVM_EXPAND,
                Some(WPARAM(TVE_EXPAND.0 as usize)),
                Some(LPARAM(h_acc.0)),
            );
        }
    }
    if let Some(h) = first_inbox {
        unsafe {
            SendMessageW(
                hwnd_tv,
                TVM_SELECTITEM,
                Some(WPARAM(TVGN_CARET as usize)),
                Some(LPARAM(h.0)),
            );
        }
    }
}

/// Обработка TVN_SELCHANGEDW: выбор папки в дереве.
pub fn on_tree_select(hwnd_list: HWND, st: &mut AppState, nm: &NMTREEVIEWW) {
    let slot = nm.itemNew.lParam.0 as usize;
    let Some(TreeSlot::Folder { key, .. }) = st.tree_slots.get(slot).cloned() else {
        return;
    };
    // Выбор папки закрывает поиск (устаревший таймер отбросится
    // в WM_TIMER по search_query == None).
    if st.search_query.take().is_some() {
        close_search_ui(hwnd_list, st);
    }
    show_folder(hwnd_list, st, &key);
}

/// Показывает папку: письма из кэша в виртуальный список (мгновенно),
/// статус с числом непрочитанных.
pub fn show_folder(hwnd_list: HWND, st: &mut AppState, key: &str) {
    st.current_folder = Some(key.to_string());
    let (aid, _) = split_key(key);
    st.current_account = aid;
    st.selected_uid = None;
    st.pane_body = None;
    st.rows = Store::open_default()
        .map(|s| s.load_folder(key).unwrap_or_default())
        .unwrap_or_default()
        .into_iter()
        .map(|hdr| ListRow { hdr, folder: None })
        .collect();
    unsafe {
        SendMessageW(
            hwnd_list,
            LVM_SETITEMCOUNT,
            Some(WPARAM(st.rows.len())),
            None,
        );
        let _ = InvalidateRect(Some(hwnd_list), None, false);
    }
    let unread = st.rows.iter().filter(|m| !m.hdr.seen).count();
    let (_, disp) = split_key(key);
    status_text(
        st,
        &format!(
            "{}: {} писем, {} непрочитанных",
            folder_display(&disp),
            st.rows.len(),
            unread
        ),
    );
}

/// Открыть поиск: показать поле над списком, колонки с «Папкой», фокус в поле.
/// Повторный вызов (например, «/» при открытом поиске) — просто фокус.
pub fn enter_search(hwnd_list: HWND, st: &mut AppState) {
    if st.search_query.is_none() {
        st.search_query = Some(String::new());
        set_list_columns(hwnd_list, true);
        st.rows.clear();
        st.selected_uid = None;
        st.pane_body = None;
        unsafe {
            SendMessageW(hwnd_list, LVM_SETITEMCOUNT, Some(WPARAM(0)), None);
            let _ = ShowWindow(st.hwnd_search, SW_SHOW);
        }
    }
    unsafe {
        let _ = SetFocus(Some(st.hwnd_search));
        // Выделить прежний запрос — новый ввод начнётся с чистого поля.
        SendMessageW(
            st.hwnd_search,
            EM_SETSEL,
            Some(WPARAM(0)),
            Some(LPARAM(-1isize)),
        );
    }
    status_text(
        st,
        "поиск по всем папкам ящика: Enter или пауза — искать, Esc — к папке",
    );
}

/// Закрыть поиск и вернуться к текущей папке (Esc, пустой запрос, выбор папки).
pub fn exit_search(hwnd: HWND, hwnd_list: HWND, st: &mut AppState) {
    if st.search_query.is_none() {
        return; // поиск и так закрыт — ничего не дёргаем
    }
    st.search_query = None;
    let _ = unsafe { KillTimer(Some(hwnd), IDT_SEARCH) };
    close_search_ui(hwnd_list, st);
    if let Some(key) = st.current_folder.clone() {
        show_folder(hwnd_list, st, &key);
    } else {
        st.rows.clear();
        unsafe {
            SendMessageW(hwnd_list, LVM_SETITEMCOUNT, Some(WPARAM(0)), None);
        }
        status_text(st, "выберите папку");
    }
}

/// Внешняя часть закрытия: спрятать поле и вернуть колонки папки.
/// Вызывается и из on_tree_select (без таймера — устаревший WM_TIMER
/// отбрасывается по search_query == None).
fn close_search_ui(hwnd_list: HWND, st: &mut AppState) {
    let _ = unsafe { ShowWindow(st.hwnd_search, SW_HIDE) };
    set_list_columns(hwnd_list, false);
}

/// Запустить поиск по текущему тексту поля (Enter или сработавший debounce).
pub fn fire_search(hwnd: HWND, hwnd_list: HWND, st: &mut AppState) {
    let _ = unsafe { KillTimer(Some(hwnd), IDT_SEARCH) };
    let q = crate::wide::read_text(st.hwnd_search).trim().to_string();
    if q.is_empty() {
        // Пустой запрос = выхода из поиска: предсказуемо и как в Soroka.
        exit_search(hwnd, hwnd_list, st);
        return;
    }
    let Some(account_id) = st
        .current_account
        .clone()
        .or_else(|| st.accounts.first().map(|a| a.id.clone()))
    else {
        status_text(st, "ящики не настроены — искать не по чему");
        return;
    };
    st.search_query = Some(q.clone());
    st.rows.clear();
    st.selected_uid = None;
    st.pane_body = None;
    unsafe {
        SendMessageW(hwnd_list, LVM_SETITEMCOUNT, Some(WPARAM(0)), None);
        let _ = InvalidateRect(Some(hwnd_list), None, false);
    }
    status_text(st, &format!("ищу «{q}»…"));
    if let Some(b) = &st.backend {
        b.post(crate::backend::Job::Search {
            account_id,
            query: q,
        });
    }
}

/// Ответ LVN_GETDISPINFOW: текст ячейки виртуального списка.
pub fn on_get_disp_info(st: &mut AppState, nm: &mut NMLVDISPINFOW) {
    let i = nm.item.iItem as usize;
    if !nm.item.mask.contains(LVIF_TEXT) || i >= st.rows.len() {
        return;
    }
    let row = st.rows[i].clone();
    let text = match nm.item.iSubItem {
        0 => {
            if row.hdr.has_attachments {
                "+".to_string()
            } else {
                String::new()
            }
        }
        1 => row.hdr.from.clone(),
        2 => row.hdr.subject.clone(),
        // В режиме поиска колонка 3 — папка письма; в режиме папки — дата.
        3 => match &row.folder {
            Some(key) => folder_display(&split_key(key).1),
            None => list_date(&row.hdr.date),
        },
        _ => list_date(&row.hdr.date),
    };
    copy_utf16(nm.item.pszText, nm.item.cchTextMax, &text);
}

/// Локальное «сейчас»: (Unix-время UTC, смещение зоны в секундах).
/// Смещение — из пары GetLocalTime/GetSystemTime (DST-переходы дают
/// небольшую неточность на старых письмах — для «сегодня/вчера» неважно).
fn local_now_off() -> (i64, i64) {
    use windows::Win32::Foundation::SYSTEMTIME;
    use windows::Win32::System::SystemInformation::{GetLocalTime, GetSystemTime};
    let st_unix = |st: &SYSTEMTIME| {
        dates::from_ymdhms(
            st.wYear as i64,
            st.wMonth as u32,
            st.wDay as u32,
            st.wHour as u32,
            st.wMinute as u32,
            st.wSecond as u32,
        )
    };
    unsafe {
        let lt = GetLocalTime();
        let utc = GetSystemTime();
        let l = st_unix(&lt);
        let u = st_unix(&utc);
        (u, l - u)
    }
}

/// Дата строки списка: «сегодня, 14:32» / «вчера» / «12 ноября» (TASK-N05).
/// Что не разобралось — показываем сырую строку заголовка.
fn list_date(raw: &str) -> String {
    let (now, off) = local_now_off();
    match dates::parse_rfc5322(raw) {
        Some(ts) => dates::fmt_list(ts, now, off),
        None => raw.trim().to_string(),
    }
}

/// Дата шапки письма: полный формат «18 ноября 2026, 14:32».
fn full_date(raw: &str) -> String {
    let (now, off) = local_now_off();
    match dates::parse_rfc5322(raw) {
        Some(ts) => dates::fmt_full(ts, now, off),
        None => raw.trim().to_string(),
    }
}

/// Ответ NM_CUSTOMDRAW списка: непрочитанные — полужирным, цвета —
/// палитра темы (TASK-N05). На каждый ITEMPREPAINT (включая сабайтемы)
/// выбираем нужный шрифт и цвет заново.
///
/// lv — сырой указатель уведомления: список присылает NMLVCUSTOMDRAW
/// (расширение NMCUSTOMDRAW), цвета меняются в нём же.
///
/// Выделенная строка красится системой (синий + белый, как у дерева —
/// контрол делает это сам): палитру в выделение не вмешиваем; owner-draw
/// строки — отдельная задача N10, если понадобится.
pub fn on_list_custom_draw(st: &AppState, lv: *mut NMLVCUSTOMDRAW) -> LRESULT {
    let (stage, item, state, hdc) = unsafe {
        let nm = &(*lv).nmcd;
        (nm.dwDrawStage, nm.dwItemSpec, nm.uItemState, nm.hdc)
    };
    match stage {
        CDDS_PREPAINT => LRESULT(CDRF_NOTIFYITEMDRAW as isize),
        stage if stage.0 & CDDS_ITEMPREPAINT.0 != 0 => {
            let bold = st.rows.get(item).is_some_and(|r| !r.hdr.seen);
            let hfont = if bold { st.font_bold } else { st.font };
            unsafe {
                SelectObject(hdc, HGDIOBJ(hfont.0));
            }
            if (state & CDIS_SELECTED).0 == 0 {
                // Свободная строка: колонка даты — вторичным цветом,
                // остальное — основным (цвета темы).
                let lv = unsafe { &mut *lv };
                let p = theme::palette(st.dark);
                let date_col: i32 = if st.search_query.is_some() { 4 } else { 3 };
                lv.clrText = if lv.iSubItem == date_col {
                    p.text_dim
                } else {
                    p.text
                };
            }
            LRESULT(CDRF_NOTIFYSUBITEMDRAW as isize)
        }
        _ => LRESULT(CDRF_DODEFAULT as isize),
    }
}

/// Ответ LVN_ITEMCHANGED: фокус перешёл на строку — открыть письмо.
pub fn on_list_item_changed(st: &mut AppState, nm: &NMLISTVIEW) {
    const LVIS_FOCUSED: u32 = 0x0001;
    let was = nm.uOldState & LVIS_FOCUSED;
    let now = nm.uNewState & LVIS_FOCUSED;
    if now == 0 || was != 0 {
        return;
    }
    let i = nm.iItem as usize;
    let Some(row) = st.rows.get(i) else {
        return;
    };
    // Строка из результатов поиска: контекст (папка/ящик) берём из неё —
    // ответ, удаление и флаги дальше работают с этой папкой.
    if let Some(f) = &row.folder {
        st.current_folder = Some(f.clone());
        st.current_account = split_key(f).0;
    }
    let key = st.current_folder.clone().unwrap_or_default();
    if key.is_empty() {
        return;
    }
    st.selected_uid = Some(row.hdr.uid);
    if st.pane_body.as_ref().is_some_and(|b| b.uid == row.hdr.uid) {
        return; // уже показано
    }
    // Плейсхолдер, пока бэкенд тащит тело (кэш — мгновенно).
    if let Some(wv) = &st.webview {
        let _ =
            unsafe { wv.NavigateToString(&HSTRING::from(loading_html(&row.hdr.subject, st.dark))) };
    }
    status_text(st, &format!("загрузка письма {}…", row.hdr.subject));
    if let Some(b) = &st.backend {
        b.post(crate::backend::Job::FetchBody {
            key,
            uid: row.hdr.uid,
        });
    }
}

/// Читаемый размер вложения.
fn fmt_size(bytes: u32) -> String {
    if bytes < 1024 {
        format!("{bytes} Б")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} КБ", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} МБ", bytes as f64 / (1024.0 * 1024.0))
    }
}

/// Шаблон панели письма (WebView2) в палитре темы (TASK-N05: цвета —
/// переменные Soroka из styles.css, тёмная тема — дефолт).
pub fn letter_html(hdr: &MessageHeader, body: &BodyData, dark: bool) -> String {
    let p = theme::palette(dark);
    let atts = if body.attachments.is_empty() {
        String::new()
    } else {
        let items: Vec<String> = body
            .attachments
            .iter()
            .enumerate()
            .map(|(i, a)| {
                format!(
                    r##"<a href="#" onclick="window.chrome.webview.postMessage('attach:{i}');return false;">{}</a> ({})"##,
                    escape_html(&a.name),
                    fmt_size(a.size)
                )
            })
            .collect();
        format!(
            r#"<div class="atts">Вложения: {} — щёлкните, чтобы сохранить в «Загрузки»</div>"#,
            items.join(" · ")
        )
    };
    let truncated = if body.truncated {
        r#"<div class="warn">письмо показано не полностью (слишком большое)</div>"#
    } else {
        ""
    };
    let inner = if let Some(html) = &body.html {
        html.clone()
    } else {
        format!(
            "<pre>{}</pre>",
            escape_html(body.text.as_deref().unwrap_or("(письмо без текста)"))
        )
    };
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><style>
body {{ font-family: 'Segoe UI', sans-serif; background: {bg}; color: {text}; margin: 0; }}
.hdr {{ padding: 14px 20px 12px; border-bottom: 1px solid {border}; background: {hdr}; }}
h1 {{ font-size: 17px; font-weight: 600; margin: 0 0 6px; }}
.meta {{ color: {meta}; font-size: 13px; line-height: 1.5; }}
.atts {{ padding: 8px 20px; border-bottom: 1px solid {border}; font-size: 13px; color: {meta}; }}
.atts a {{ color: {link}; text-decoration: none; border-bottom: 1px dotted {link}; }}
.warn {{ padding: 8px 20px; color: {warn}; font-size: 12px; border-bottom: 1px solid {border}; }}
.body {{ padding: 16px 20px; line-height: 1.55; font-size: 14px; }}
pre {{ white-space: pre-wrap; word-wrap: break-word; font-family: 'Segoe UI', Consolas, monospace; }}
img {{ max-width: 100%; }}
</style></head><body>
<div class="hdr"><h1>{subject}</h1><div class="meta">От: {from}<br>Кому: {to}<br>{date}</div></div>
{atts}{truncated}
<div class="body">{inner}</div>
</body></html>"#,
        bg = p.pane_bg,
        text = p.pane_text,
        border = p.pane_border,
        hdr = p.pane_hdr,
        meta = p.pane_meta,
        link = p.pane_link,
        warn = p.pane_warn,
        subject = escape_html(&hdr.subject),
        from = escape_html(&hdr.from),
        to = escape_html(&hdr.to),
        date = escape_html(&full_date(&hdr.date)),
        atts = atts,
        truncated = truncated,
        inner = inner,
    )
}

/// Заглушка «загружается» в палитре темы.
fn loading_html(subject: &str, dark: bool) -> String {
    let p = theme::palette(dark);
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><style>
body {{ font-family: 'Segoe UI', sans-serif; background: {bg}; color: {meta}; margin: 24px; }}
</style></head><body>Загрузка «{subject}»…</body></html>"#,
        bg = p.pane_bg,
        meta = p.pane_meta,
        subject = escape_html(subject)
    )
}

/// Пустая панель: папка выбрана, письмо — нет. В палитре темы.
pub fn empty_pane_html(dark: bool) -> String {
    let p = theme::palette(dark);
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><style>
body {{ font-family: 'Segoe UI', sans-serif; background: {bg}; color: {meta}; margin: 24px; }}
</style></head><body>Выберите письмо в списке.</body></html>"#,
        bg = p.pane_bg,
        meta = p.pane_meta
    )
}

/// Показ тела письма в панели (вызывается на BodyReady).
pub fn show_body(st: &mut AppState, key: &str, uid: u32, from_cache: bool, body: BodyData) {
    let Some(i) = st.rows.iter().position(|m| m.hdr.uid == uid) else {
        return;
    };
    if st.current_folder.as_deref() != Some(key) || st.selected_uid != Some(uid) {
        return;
    }
    let hdr = st.rows[i].hdr.clone();
    let was_unread = !hdr.seen;
    st.pane_body = Some(PaneBody {
        uid,
        text: body.text.clone(),
        html: body.html.clone(),
        attachments: body.attachments.clone(),
        truncated: body.truncated,
    });
    if let Some(wv) = &st.webview {
        let _ = unsafe { wv.NavigateToString(&HSTRING::from(letter_html(&hdr, &body, st.dark))) };
    }
    status_text(
        st,
        &format!("{}{}", hdr.subject, if from_cache { " (кэш)" } else { "" }),
    );
    // Прочитано — на сервер и в кэш (как set_seen в Soroka).
    if was_unread && let Some(b) = &st.backend {
        b.post(crate::backend::Job::SetSeen {
            key: key.to_string(),
            uid,
            seen: true,
        });
    }
}

/// Применение события бэкенда (вызывается из wndproc на WM_APP_MAIL).
pub fn apply_mail_event(
    hwnd: HWND,
    hwnd_list: HWND,
    hwnd_tv: HWND,
    st: &mut AppState,
    ev: MailEvent,
) {
    match ev {
        MailEvent::SyncDone {
            key,
            account_id: _,
            total,
            fetched,
            error,
            folders_refreshed,
        } => {
            if folders_refreshed {
                let sel = st.current_folder.clone();
                rebuild_tree_keep(hwnd_tv, hwnd_list, st, sel);
            }
            // Во время поиска список не подменяем выдачей папки — только статус.
            if st.search_query.is_none() && st.current_folder.as_deref() == Some(key.as_str()) {
                show_folder(hwnd_list, st, &key);
            }
            match error {
                Some(err) => status_text(st, &format!("сеть: {err}")),
                None => status_text(st, &format!("{key}: всего {total}, загружено {fetched}")),
            }
        }
        MailEvent::BodyReady {
            key,
            uid,
            from_cache,
            body,
        } => {
            show_body(st, &key, uid, from_cache, body);
        }
        MailEvent::BodyFailed { key, uid, error } => {
            if st.current_folder.as_deref() == Some(key.as_str()) && st.selected_uid == Some(uid) {
                status_text(st, &format!("не удалось загрузить письмо: {error}"));
            }
        }
        MailEvent::Seen { key, uid } => {
            if st.current_folder.as_deref() == Some(key.as_str()) {
                if let Some(m) = st.rows.iter_mut().find(|m| m.hdr.uid == uid) {
                    m.hdr.seen = true;
                }
                let _ = unsafe { InvalidateRect(Some(hwnd_list), None, false) };
            }
        }
        MailEvent::Unseen { key, uid } => {
            if st.current_folder.as_deref() == Some(key.as_str()) {
                if let Some(m) = st.rows.iter_mut().find(|m| m.hdr.uid == uid) {
                    m.hdr.seen = false;
                }
                let _ = unsafe { InvalidateRect(Some(hwnd_list), None, false) };
            }
        }
        MailEvent::Deleted { key, uid } => {
            if st.current_folder.as_deref() == Some(key.as_str()) {
                st.rows.retain(|m| m.hdr.uid != uid);
                unsafe {
                    SendMessageW(
                        hwnd_list,
                        LVM_SETITEMCOUNT,
                        Some(WPARAM(st.rows.len())),
                        None,
                    );
                }
                if st.selected_uid == Some(uid) {
                    st.selected_uid = None;
                    st.pane_body = None;
                    if let Some(wv) = &st.webview {
                        let _ = unsafe {
                            wv.NavigateToString(&HSTRING::from(empty_pane_html(st.dark)))
                        };
                    }
                }
                let _ = unsafe { InvalidateRect(Some(hwnd_list), None, false) };
                status_text(st, "письмо удалено (в корзине сервера)");
            }
        }
        MailEvent::AttachSaved { path } => {
            status_text(st, &format!("вложение сохранено: {path}"));
            unsafe {
                windows::Win32::UI::WindowsAndMessaging::MessageBoxW(
                    Some(hwnd),
                    w!("Вложение сохранено в «Загрузки»."),
                    w!("POS Mail"),
                    windows::Win32::UI::WindowsAndMessaging::MB_OK
                        | windows::Win32::UI::WindowsAndMessaging::MB_ICONINFORMATION,
                );
            }
        }
        MailEvent::AttachFailed { error } => {
            status_text(st, &format!("вложение: {error}"));
        }
        MailEvent::OutboxFlushed { sent } => {
            status_text(
                st,
                &format!("очередь: отправлено писем, ждавших сети: {sent}"),
            );
        }
        MailEvent::SearchDone { query, hits } => {
            // Режим поиска уже закрыт или сменился запрос — выдача устарела.
            let Some(cur) = st.search_query.as_ref() else {
                return;
            };
            if cur.as_str() != query.as_str() {
                return;
            }
            let in_body = hits.iter().filter(|h| h.in_body).count();
            st.rows = hits
                .iter()
                .map(|h| ListRow {
                    hdr: MessageHeader {
                        uid: h.uid,
                        seq: 0,
                        from: h.from.clone(),
                        subject: h.subject.clone(),
                        date: h.date.clone(),
                        seen: h.seen,
                        flagged: h.flagged,
                        has_attachments: h.has_attachments,
                        to: String::new(),
                        cc: String::new(),
                        msg_id: String::new(),
                        thread_key: String::new(),
                    },
                    folder: Some(h.folder.clone()),
                })
                .collect();
            st.selected_uid = None;
            st.pane_body = None;
            unsafe {
                SendMessageW(
                    hwnd_list,
                    LVM_SETITEMCOUNT,
                    Some(WPARAM(st.rows.len())),
                    None,
                );
                let _ = InvalidateRect(Some(hwnd_list), None, false);
            }
            let n = st.rows.len();
            status_text(
                st,
                &format!("поиск «{query}»: {n} писем, {in_body} совпало в теле — Esc к папке"),
            );
        }
        MailEvent::Status(text) => status_text(st, &text),
    }
}

/// rebuild_tree с восстановлением выбранной папки.
fn rebuild_tree_keep(hwnd_tv: HWND, hwnd_list: HWND, st: &mut AppState, sel: Option<String>) {
    rebuild_tree(hwnd_tv, st);
    if let Some(key) = sel
        && st.search_query.is_none()
        && st
            .tree_slots
            .iter()
            .any(|s| matches!(s, TreeSlot::Folder { key: k, .. } if *k == key))
    {
        show_folder(hwnd_list, st, &key);
    }
}
