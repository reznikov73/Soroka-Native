//! Календарь (TASK-N11): сетка месяца + события выбранного дня.
//!
//! Данные — ядро Soroka (`Store::{load_calendar_events, add_calendar_event,
//! delete_calendar_event}`), экспорт события — перенесённый `ics.rs`.
//! `src/mail/` не правится.
//!
//! Сетка рисуется GDI (WM_PAINT): 7 колонок × 6 строк, дни другого месяца —
//! приглушённым цветом, сегодняшний — акцентом, выбранный — фоном выделения.
//! Рядом панель событий дня: список + «Добавить…» / «Удалить» / «В файл .ics».

use windows::core::{PCWSTR, w};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DEFAULT_GUI_FONT, DT_CENTER, DT_END_ELLIPSIS, DT_NOPREFIX,
    DT_SINGLELINE, DT_VCENTER, DeleteObject, DrawTextW, EndPaint, FillRect, GetStockObject,
    HBRUSH, HDC, HGDIOBJ, InvalidateRect, PAINTSTRUCT, SetBkColor, SetBkMode, SetTextColor,
    TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::{
    BN_CLICKED, BeginDeferWindowPos, CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT,
    CreateWindowExW, DefWindowProcW, DeferWindowPos, DestroyWindow, EndDeferWindowPos,
    ES_AUTOHSCROLL, ES_AUTOVSCROLL, ES_MULTILINE, GWLP_USERDATA, GetClientRect, GetWindowLongPtrW,
    IDC_ARROW, LB_ADDSTRING, LB_GETCURSEL, LB_RESETCONTENT, LBS_NOTIFY, LoadCursorW, MB_ICONERROR,
    MB_ICONINFORMATION, MB_OK, MINMAXINFO, MessageBoxW, PostMessageW, RegisterClassW, SWP_NOZORDER,
    SW_SHOW, SendMessageW, SetWindowLongPtrW, SetWindowPos, ShowWindow, WINDOW_EX_STYLE,
    WINDOW_STYLE, WM_CLOSE, WM_COMMAND, WM_CREATE, WM_CTLCOLOREDIT, WM_CTLCOLORLISTBOX,
    WM_CTLCOLORSTATIC, WM_DESTROY,
    WM_ERASEBKGND, WM_GETMINMAXINFO, WM_LBUTTONDOWN, WM_NCCREATE, WM_PAINT, WM_SETFONT, WM_SIZE,
    WNDCLASSW, WS_BORDER, WS_CHILD, WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_OVERLAPPEDWINDOW,
    WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};

use crate::dates;
use crate::ics;
use crate::mail::store::{CalendarEvent, Store};
use crate::state::AppState;
use crate::theme;
use crate::wide::{escape_html, read_text, set_window_text, wide};

/// Главное окно узнаёт о закрытии календаря.
pub const WM_APP_CALENDAR_CLOSED: u32 = crate::contacts_ui::WM_APP_CONTACTS_CLOSED + 3;
/// Диалог события сохранил запись.
const WM_APP_EVENT_SAVED: u32 = WM_APP_CALENDAR_CLOSED + 1;
/// Диалог события закрылся.
const WM_APP_EVENT_CLOSED: u32 = WM_APP_EVENT_SAVED + 1;

const WINDOW_CLASS: PCWSTR = w!("SorokaCalendarWnd");
const EVENT_CLASS: PCWSTR = w!("SorokaEventWnd");

const IDC_PREV: i32 = 600;
const IDC_NEXT: i32 = 601;
const IDC_TODAY: i32 = 602;
const IDC_LIST: i32 = 603;
const IDC_BTN_ADD: i32 = 604;
const IDC_BTN_DEL: i32 = 605;
const IDC_BTN_ICS: i32 = 606;
const IDC_STATUS: i32 = 607;

const IDC_EV_TITLE: i32 = 620;
const IDC_EV_DATE: i32 = 621;
const IDC_EV_TIME: i32 = 622;
const IDC_EV_DESC: i32 = 623;
const IDC_EV_SAVE: i32 = 624;
const IDC_EV_CANCEL: i32 = 625;

/// Геометрия календаря, px.
const HEADER_H: i32 = 48;
const WEEKDAY_H: i32 = 22;
const PANEL_W: i32 = 280;
const MARGIN: i32 = 12;

struct CalState {
    parent: HWND,
    dark: bool,
    year: i64,
    month: u32,
    /// Выбранный день «YYYY-MM-DD».
    selected: String,
    events: Vec<CalendarEvent>,
    /// События выбранного дня (строки списка).
    day_events: Vec<CalendarEvent>,
    hwnd_list: HWND,
    hwnd_status: HWND,
    hwnd_event_dlg: HWND,
    hwnd_btn_add: HWND,
    hwnd_btn_del: HWND,
    hwnd_btn_ics: HWND,
    hbr_bg: HBRUSH,
    hbr_surface: HBRUSH,
    hbr_sel: HBRUSH,
}

struct EventState {
    parent: HWND,
    dark: bool,
    hwnd_title: HWND,
    hwnd_date: HWND,
    hwnd_time: HWND,
    hwnd_desc: HWND,
    hwnd_save: HWND,
    hwnd_cancel: HWND,
    hbr_bg: HBRUSH,
    hbr_surface: HBRUSH,
}

/// «YYYY-MM-DD» из года/месяца/дня.
fn ymd(y: i64, m: u32, d: u32) -> String {
    format!("{y:04}-{m:02}-{d:02}")
}

/// Сегодня по локальному времени: (год, месяц, день).
fn today() -> (i64, u32, u32) {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    // Смещение локального времени к UTC — без tz-базы: берём у системы.
    let off = local_offset_secs();
    dates::civil_from_days((ts + off).div_euclid(86_400))
}

/// Смещение локального времени, секунды (Win32: разница между local и UTC).
fn local_offset_secs() -> i64 {
    use windows::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};
    unsafe {
        let mut tz = TIME_ZONE_INFORMATION::default();
        let r = GetTimeZoneInformation(&mut tz);
        // TIME_ZONE_ID_INVALID = 0xFFFFFFFF: не смогли — считаем UTC.
        if r == windows::Win32::System::Time::TIME_ZONE_ID_INVALID {
            return 0;
        }
        -(tz.Bias as i64 * 60) - (tz.DaylightBias as i64 * 60)
    }
}

/// Сколько дней в месяце — оставлено для будущих проверок ввода даты.
///
/// Сетка считается через `dates::civil_from_days` (полные даты), поэтому прямо
/// сейчас помощник не вызывается; жалоба clippy `dead_code` ожидаема.
#[allow(dead_code)]
fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => {
            if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 {
                29
            } else {
                28
            }
        }
    }
}

/// Сдвиг месяца на delta (например, -1/1).
fn shift_month(y: i64, m: u32, delta: i32) -> (i64, u32) {
    let total = (y * 12 + (m as i64 - 1)) + delta as i64;
    (total.div_euclid(12), (total.rem_euclid(12) + 1) as u32)
}

/// Понедельник-первый индекс дня недели для даты.
fn weekday_monday(y: i64, m: u32, d: u32) -> i64 {
    (dates::days_from_civil(y, m, d) + 3).rem_euclid(7)
}

/// Открывает календарь (один на приложение).
pub fn open(hwnd_main: HWND, st: &AppState) -> Option<HWND> {
    if let Some(h) = st.hwnd_calendar {
        let _ = unsafe { SetFocus(Some(h)) };
        return None;
    }
    let hmod = unsafe { GetModuleHandleW(None) }.expect("GetModuleHandleW");
    let wc = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(wndproc),
        hInstance: windows::Win32::Foundation::HINSTANCE(hmod.0),
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW) }.expect("LoadCursorW"),
        lpszClassName: WINDOW_CLASS,
        ..Default::default()
    };
    unsafe { RegisterClassW(&wc) };

    let (y, m, d) = today();
    let state = Box::new(CalState {
        parent: hwnd_main,
        dark: st.dark,
        year: y,
        month: m,
        selected: ymd(y, m, d),
        events: Vec::new(),
        day_events: Vec::new(),
        hwnd_list: HWND::default(),
        hwnd_status: HWND::default(),
        hwnd_event_dlg: HWND::default(),
        hwnd_btn_add: HWND::default(),
        hwnd_btn_del: HWND::default(),
        hwnd_btn_ics: HWND::default(),
        hbr_bg: HBRUSH::default(),
        hbr_surface: HBRUSH::default(),
        hbr_sel: HBRUSH::default(),
    });
    let raw = Box::into_raw(state);
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            WINDOW_CLASS,
            w!("Календарь — POS Mail"),
            WINDOW_STYLE(WS_OVERLAPPEDWINDOW.0 | WS_CLIPCHILDREN.0),
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            940,
            620,
            Some(hwnd_main),
            None,
            Some(windows::Win32::Foundation::HINSTANCE(hmod.0)),
            Some(raw as *const std::ffi::c_void),
        )
        .expect("создать окно календаря")
    };
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetFocus(Some((*raw).hwnd_list));
    }
    Some(hwnd)
}

unsafe fn st_of(hwnd: HWND) -> Option<&'static mut CalState> {
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut CalState;
    if ptr.is_null() {
        None
    } else {
        Some(unsafe { &mut *ptr })
    }
}

unsafe fn ev_of(hwnd: HWND) -> Option<&'static mut EventState> {
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut EventState;
    if ptr.is_null() {
        None
    } else {
        Some(unsafe { &mut *ptr })
    }
}

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
            WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | WS_CLIPSIBLINGS.0 | style),
            x,
            y,
            w,
            h,
            Some(parent),
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

/// Прямоугольник сетки: (x, y, ширина ячейки, высота ячейки).
fn grid_geom(w: i32, h: i32) -> (i32, i32, i32, i32, i32) {
    let grid_w = (w - MARGIN * 3 - PANEL_W).max(300);
    let top = HEADER_H + WEEKDAY_H;
    let grid_h = (h - top - MARGIN).max(120);
    let cw = grid_w / 7;
    let ch = grid_h / 6;
    (MARGIN, top, cw, ch, grid_w)
}

/// Ячейка под точкой (x, y) → дата или None.
fn cell_at(st: &CalState, x: i32, y: i32) -> Option<(i64, u32, u32)> {
    let mut rc = RECT::default();
    if unsafe { GetClientRect(st.hwnd_list, &mut rc) }.is_err() {
        // не используем — только для подавления предупреждения о неиспользуемом
    }
    let (w, h) = unsafe {
        let mut cr = RECT::default();
        let _ = windows::Win32::UI::WindowsAndMessaging::GetClientRect(
            windows::Win32::UI::WindowsAndMessaging::GetParent(st.hwnd_list)
                .unwrap_or_default(),
            &mut cr,
        );
        (cr.right, cr.bottom)
    };
    let (gx, gy, cw, ch, _) = grid_geom(w, h);
    if x < gx || y < gy || cw <= 0 || ch <= 0 {
        return None;
    }
    let col = (x - gx) / cw;
    let row = (y - gy) / ch;
    if !(0..7).contains(&col) || !(0..6).contains(&row) {
        return None;
    }
    let first = dates::days_from_civil(st.year, st.month, 1);
    let start = first - weekday_monday(st.year, st.month, 1);
    let days = start + (row * 7 + col) as i64;
    let (y2, m2, d2) = dates::civil_from_days(days);
    Some((y2, m2, d2))
}

/// Перечитывает события из кэша и обновляет список выбранного дня.
unsafe fn reload(hwnd: HWND, st: &mut CalState) {
    st.events = Store::open_default()
        .and_then(|s| s.load_calendar_events())
        .unwrap_or_default();
    unsafe { refresh_day(hwnd, st) };
}

/// Заполняет список событий выбранного дня.
unsafe fn refresh_day(hwnd: HWND, st: &mut CalState) {
    st.day_events = st
        .events
        .iter()
        .filter(|e| e.start_date == st.selected)
        .cloned()
        .collect();
    unsafe {
        SendMessageW(st.hwnd_list, LB_RESETCONTENT, None, None);
        for e in &st.day_events {
            let line = match &e.start_time {
                Some(t) if !t.is_empty() => format!("{t}  {}", e.title),
                _ => format!("весь день  {}", e.title),
            };
            let mut buf = wide(&line);
            SendMessageW(
                st.hwnd_list,
                LB_ADDSTRING,
                Some(WPARAM(0)),
                Some(LPARAM(buf.as_mut_ptr() as isize)),
            );
        }
    }
    let in_month = st
        .events
        .iter()
        .filter(|e| e.start_date.starts_with(&format!("{:04}-{:02}", st.year, st.month)))
        .count();
    let text = format!(
        "{} {} — событий в месяце: {}, в дне: {}",
        dates::month_nom_ru(st.month),
        st.year,
        in_month,
        st.day_events.len()
    );
    let _ = set_window_text(st.hwnd_status, &text);
    let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
}

/// Раскладка: кнопки навигации в шапке, панель событий справа.
unsafe fn layout(hwnd: HWND, st: &CalState) {
    let mut rc = RECT::default();
    if unsafe { GetClientRect(hwnd, &mut rc) }.is_err() {
        return;
    }
    let w = rc.right;
    let h = rc.bottom;
    let px = w - PANEL_W - MARGIN;
    let entries: [(HWND, i32, i32, i32, i32); 6] = [
        (st.hwnd_status, 200, 12, (px - 200 - MARGIN).max(150), 22),
        (st.hwnd_list, px, HEADER_H + WEEKDAY_H, PANEL_W, (h - HEADER_H - WEEKDAY_H - 100).max(80)),
        (st.hwnd_btn_add, px, h - 92, PANEL_W, 28),
        (st.hwnd_btn_del, px, h - 60, 132, 28),
        (st.hwnd_btn_ics, px + 140, h - 60, 140, 28),
        (HWND::default(), 0, 0, 0, 0),
    ];
    unsafe {
        match BeginDeferWindowPos(entries.len() as i32) {
            Ok(mut wpos) => {
                for (h2, x, y, cx, cy) in &entries {
                    if !h2.0.is_null() {
                        wpos = DeferWindowPos(wpos, *h2, None, *x, *y, *cx, *cy, SWP_NOZORDER)
                            .unwrap_or(wpos);
                    }
                }
                let _ = EndDeferWindowPos(wpos);
            }
            Err(_) => {
                for (h2, x, y, cx, cy) in &entries {
                    if !h2.0.is_null() {
                        let _ = SetWindowPos(*h2, None, *x, *y, *cx, *cy, SWP_NOZORDER);
                    }
                }
            }
        }
    }
}

/// Кисти палитры.
unsafe fn apply_brushes(hwnd: HWND, st: &mut CalState) {
    let p = theme::palette(st.dark);
    unsafe {
        for b in [st.hbr_bg, st.hbr_surface, st.hbr_sel] {
            if !b.is_invalid() {
                let _ = DeleteObject(HGDIOBJ(b.0));
            }
        }
        st.hbr_bg = CreateSolidBrush(p.bg);
        st.hbr_surface = CreateSolidBrush(p.surface);
        st.hbr_sel = CreateSolidBrush(p.selected);
        let _ = InvalidateRect(Some(hwnd), None, true);
    }
}

/// Рисование сетки месяца.
unsafe fn paint(hwnd: HWND, st: &CalState) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = unsafe { BeginPaint(hwnd, &mut ps) };
    let p = theme::palette(st.dark);
    let mut rc = RECT::default();
    if unsafe { GetClientRect(hwnd, &mut rc) }.is_err() {
        let _ = unsafe { EndPaint(hwnd, &ps) };
        return;
    }
    let (w, h) = (rc.right, rc.bottom);
    unsafe {
        let _ = FillRect(hdc, &rc, st.hbr_bg);
        let _ = SetBkMode(hdc, TRANSPARENT);
        let font = GetStockObject(DEFAULT_GUI_FONT);
        let old = windows::Win32::Graphics::Gdi::SelectObject(hdc, font);
        // Заголовок: «Ноябрь 2026».
        let mut title = wide(&format!("{} {}", dates::month_nom_ru(st.month), st.year));
        let mut trc = RECT {
            left: MARGIN + 120,
            top: 8,
            right: w - 320,
            bottom: 40,
        };
        let _ = SetTextColor(hdc, p.text);
        DrawTextW(
            hdc,
            &mut title,
            &mut trc,
            DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX,
        );
        // Дни недели.
        let (gx, gy, cw, ch, _) = grid_geom(w, h);
        let names = ["Пн", "Вт", "Ср", "Чт", "Пт", "Сб", "Вс"];
        for (i, name) in names.iter().enumerate() {
            let mut buf = wide(name);
            let mut cell = RECT {
                left: gx + cw * i as i32,
                top: gy - WEEKDAY_H,
                right: gx + cw * (i as i32 + 1),
                bottom: gy,
            };
            let _ = SetTextColor(hdc, p.text_dim);
            DrawTextW(hdc, &mut buf, &mut cell, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
        }
        // Ячейки: 6 недель от понедельника первой недели месяца.
        let first = dates::days_from_civil(st.year, st.month, 1);
        let start = first - weekday_monday(st.year, st.month, 1);
        let today_key = {
            let (ty, tm, td) = today();
            ymd(ty, tm, td)
        };
        for row in 0..6i32 {
            for col in 0..7i32 {
                let days = start + (row * 7 + col) as i64;
                let (cy, cm, cd) = dates::civil_from_days(days);
                let key = ymd(cy, cm, cd);
                let cell = RECT {
                    left: gx + cw * col,
                    top: gy + ch * row,
                    right: gx + cw * (col + 1),
                    bottom: gy + ch * (row + 1),
                };
                // Фон: выбранный / поверхность месяца / фон другого месяца.
                let brush = if key == st.selected {
                    st.hbr_sel
                } else if cm == st.month {
                    st.hbr_surface
                } else {
                    st.hbr_bg
                };
                let mut fill = cell;
                fill.left += 2;
                fill.top += 2;
                fill.right -= 2;
                fill.bottom -= 2;
                let _ = FillRect(hdc, &fill, brush);
                // Номер дня.
                let mut num = wide(&cd.to_string());
                let mut nrc = RECT {
                    left: cell.left + 6,
                    top: cell.top + 4,
                    right: cell.right - 4,
                    bottom: cell.top + 24,
                };
                let _ = SetTextColor(
                    hdc,
                    if cm != st.month {
                        p.text_dim
                    } else if key == today_key {
                        p.accent
                    } else {
                        p.text
                    },
                );
                DrawTextW(
                    hdc,
                    &mut num,
                    &mut nrc,
                    DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS,
                );
                // До двух названий событий дня.
                let mut y = cell.top + 24;
                for e in st.events.iter().filter(|e| e.start_date == key).take(2) {
                    let mut buf = wide(&e.title);
                    let mut erc = RECT {
                        left: cell.left + 6,
                        top: y,
                        right: cell.right - 4,
                        bottom: y + 16,
                    };
                    let _ = SetTextColor(hdc, p.text_dim);
                    DrawTextW(
                        hdc,
                        &mut buf,
                        &mut erc,
                        DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS,
                    );
                    y += 16;
                    if y > cell.bottom - 16 {
                        break;
                    }
                }
            }
        }
        windows::Win32::Graphics::Gdi::SelectObject(hdc, old);
        let _ = EndPaint(hwnd, &ps);
    }
    let _ = escape_html; // символ используется в других модулях — здесь не нужен
    let _ = SendMessageW; // подавление неиспользуемого импорта в этой ветке
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
        WM_CREATE => {
            let Some(st) = (unsafe { st_of(hwnd) }) else {
                return LRESULT(-1);
            };
            let hfont = unsafe { GetStockObject(DEFAULT_GUI_FONT) };
            unsafe {
                let b_prev = child(hwnd, w!("BUTTON"), Some("‹"), WS_TABSTOP.0, 12, 10, 34, 28, IDC_PREV);
                let b_next = child(hwnd, w!("BUTTON"), Some("›"), WS_TABSTOP.0, 50, 10, 34, 28, IDC_NEXT);
                let b_today = child(hwnd, w!("BUTTON"), Some("Сегодня"), WS_TABSTOP.0, 90, 10, 100, 28, IDC_TODAY);
                for b in [b_prev, b_next, b_today] {
                    set_font(b, hfont);
                }
                st.hwnd_list = child(
                    hwnd,
                    w!("LISTBOX"),
                    None,
                    LBS_NOTIFY as u32 | WS_BORDER.0 | WS_VSCROLL.0,
                    0,
                    0,
                    200,
                    100,
                    IDC_LIST,
                );
                set_font(st.hwnd_list, hfont);
                st.hwnd_status = child(hwnd, w!("STATIC"), Some(""), 0, 12, 0, 300, 20, IDC_STATUS);
                set_font(st.hwnd_status, hfont);
                st.hwnd_btn_add = child(hwnd, w!("BUTTON"), Some("Добавить событие..."), WS_TABSTOP.0, 0, 0, 200, 28, IDC_BTN_ADD);
                st.hwnd_btn_del = child(hwnd, w!("BUTTON"), Some("Удалить"), WS_TABSTOP.0, 0, 0, 120, 28, IDC_BTN_DEL);
                st.hwnd_btn_ics = child(hwnd, w!("BUTTON"), Some("В файл .ics"), WS_TABSTOP.0, 0, 0, 130, 28, IDC_BTN_ICS);
                for b in [st.hwnd_btn_add, st.hwnd_btn_del, st.hwnd_btn_ics] {
                    set_font(b, hfont);
                }
                layout(hwnd, st);
                apply_brushes(hwnd, st);
                reload(hwnd, st);
            }
            LRESULT(0)
        }
        WM_GETMINMAXINFO => {
            let mmi = unsafe { &mut *(lparam.0 as *mut MINMAXINFO) };
            mmi.ptMinTrackSize = POINT { x: 760, y: 420 };
            LRESULT(0)
        }
        WM_ERASEBKGND => {
            if let Some(st) = unsafe { st_of(hwnd) } {
                let p = theme::palette(st.dark);
                let hdc = HDC(wparam.0 as *mut core::ffi::c_void);
                let mut rc = RECT::default();
                if unsafe { GetClientRect(hwnd, &mut rc) }.is_ok() {
                    let br = unsafe { CreateSolidBrush(p.bg) };
                    unsafe { FillRect(hdc, &rc, br) };
                    let _ = unsafe { DeleteObject(HGDIOBJ(br.0)) };
                }
            }
            LRESULT(1)
        }
        WM_PAINT => {
            if let Some(st) = unsafe { st_of(hwnd) } {
                unsafe { paint(hwnd, st) };
            } else {
                let mut ps = PAINTSTRUCT::default();
                unsafe {
                    BeginPaint(hwnd, &mut ps);
                    let _ = EndPaint(hwnd, &ps);
                }
            }
            LRESULT(0)
        }
        WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX => {
            // Статус и список в тёмной теме: сами контролы фон не красят.
            if let Some(st) = unsafe { st_of(hwnd) } {
                let p = theme::palette(st.dark);
                let hdc = HDC(wparam.0 as *mut core::ffi::c_void);
                unsafe {
                    let _ = SetTextColor(hdc, p.text);
                    let _ = SetBkColor(hdc, p.surface);
                }
                return LRESULT(st.hbr_surface.0 as isize);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_SIZE => {
            if let Some(st) = unsafe { st_of(hwnd) } {
                unsafe { layout(hwnd, st) };
                let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_LBUTTONDOWN => {            if let Some(st) = unsafe { st_of(hwnd) } {
                let x = (lparam.0 & 0xFFFF) as i16 as i32;
                let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
                if let Some((cy, cm, cd)) = cell_at(st, x, y) {
                    st.selected = ymd(cy, cm, cd);
                    // Клик по дню другого месяца — перелистнуть месяц.
                    if cm != st.month || cy != st.year {
                        st.year = cy;
                        st.month = cm;
                    }
                    unsafe { refresh_day(hwnd, st) };
                }
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            let id = (wparam.0 & 0xFFFF) as i32;
            let code = ((wparam.0 >> 16) & 0xFFFF) as u32;
            if lparam.0 != 0 && code == BN_CLICKED {
                let Some(st) = (unsafe { st_of(hwnd) }) else {
                    return LRESULT(0);
                };
                match id {
                    IDC_PREV => {
                        let (y, m) = shift_month(st.year, st.month, -1);
                        st.year = y;
                        st.month = m;
                        unsafe { refresh_day(hwnd, st) };
                    }
                    IDC_NEXT => {
                        let (y, m) = shift_month(st.year, st.month, 1);
                        st.year = y;
                        st.month = m;
                        unsafe { refresh_day(hwnd, st) };
                    }
                    IDC_TODAY => {
                        let (y, m, d) = today();
                        st.year = y;
                        st.month = m;
                        st.selected = ymd(y, m, d);
                        unsafe { refresh_day(hwnd, st) };
                    }
                    IDC_BTN_ADD => unsafe { open_event_dialog(hwnd, st) },
                    IDC_BTN_DEL => unsafe { delete_selected(hwnd, st) },
                    IDC_BTN_ICS => unsafe { export_selected(hwnd, st) },
                    _ => {}
                }
                return LRESULT(0);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_APP_EVENT_SAVED => {
            if let Some(st) = unsafe { st_of(hwnd) } {
                unsafe { reload(hwnd, st) };
            }
            LRESULT(0)
        }
        WM_APP_EVENT_CLOSED => {
            if let Some(st) = unsafe { st_of(hwnd) } {
                st.hwnd_event_dlg = HWND::default();
            }
            LRESULT(0)
        }
        crate::WM_APP_DLG_THEME => {
            if let Some(st) = unsafe { st_of(hwnd) } {
                st.dark = wparam.0 != 0;
                unsafe { apply_brushes(hwnd, st) };
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = unsafe { DestroyWindow(hwnd) };
            LRESULT(0)
        }
        WM_DESTROY => {
            if let Some(st) = unsafe { st_of(hwnd) } {
                unsafe {
                    for b in [st.hbr_bg, st.hbr_surface, st.hbr_sel] {
                        if !b.is_invalid() {
                            let _ = DeleteObject(HGDIOBJ(b.0));
                        }
                    }
                    let _ = PostMessageW(Some(st.parent), WM_APP_CALENDAR_CLOSED, WPARAM(0), LPARAM(0));
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                    drop(Box::from_raw(st as *mut CalState));
                }
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Выбранное в списке событие дня.
fn selected_event(st: &CalState) -> Option<CalendarEvent> {
    let idx = unsafe { SendMessageW(st.hwnd_list, LB_GETCURSEL, None, None) }.0;
    if idx < 0 {
        return None;
    }
    st.day_events.get(idx as usize).cloned()
}

/// Удаление события с подтверждением.
unsafe fn delete_selected(hwnd: HWND, st: &mut CalState) {
    let Some(ev) = selected_event(st) else {
        msgbox(hwnd, "Сначала выберите событие в списке.", false);
        return;
    };
    let text = wide(&format!("Удалить событие «{}»?", ev.title));
    let answer = unsafe {
        MessageBoxW(
            Some(hwnd),
            PCWSTR(text.as_ptr()),
            w!("POS Mail"),
            windows::Win32::UI::WindowsAndMessaging::MB_YESNO
                | windows::Win32::UI::WindowsAndMessaging::MB_ICONINFORMATION,
        )
    };
    if answer != windows::Win32::UI::WindowsAndMessaging::IDYES {
        return;
    }
    match Store::open_default().and_then(|s| s.delete_calendar_event(ev.id)) {
        Ok(()) => unsafe { reload(hwnd, st) },
        Err(e) => msgbox(hwnd, &format!("Удалить не удалось: {e}"), true),
    }
}

/// Экспорт выбранного события в .ics (порт Soroka: файл в «Загрузки»).
unsafe fn export_selected(hwnd: HWND, st: &CalState) {
    let Some(ev) = selected_event(st) else {
        msgbox(hwnd, "Сначала выберите событие в списке.", false);
        return;
    };
    let start = match &ev.start_time {
        Some(t) if !t.is_empty() => format!("{}T{}", ev.start_date, t),
        _ => ev.start_date.clone(),
    };
    let end = ev
        .end_date
        .clone()
        .unwrap_or_else(|| start.clone());
    let item = ics::IcsEvent {
        title: &ev.title,
        start: &start,
        end: &end,
        description: &ev.description,
    };
    match ics::build_ics(&item) {
        Ok(content) => match ics::save_to_downloads(&content, &ics::file_base_name(&start)) {
            Ok(path) => msgbox(hwnd, &format!("Сохранено: {path}"), false),
            Err(e) => msgbox(hwnd, &format!("Не сохранить файл: {e}"), true),
        },
        Err(e) => msgbox(hwnd, &format!(".ics не собрался: {e}"), true),
    }
}

fn msgbox(hwnd: HWND, text: &str, error: bool) {
    let flags = if error {
        MB_OK | MB_ICONERROR
    } else {
        MB_OK | MB_ICONINFORMATION
    };
    let text = wide(text);
    unsafe {
        MessageBoxW(Some(hwnd), PCWSTR(text.as_ptr()), w!("POS Mail"), flags);
    }
}

/// Диалог нового события: дата — выбранный день, время можно оставить пустым.
unsafe fn open_event_dialog(cal: HWND, st: &mut CalState) {
    if !st.hwnd_event_dlg.0.is_null() {
        let _ = unsafe { SetFocus(Some(st.hwnd_event_dlg)) };
        return;
    }
    let hmod = unsafe { GetModuleHandleW(None) }.expect("GetModuleHandleW");
    let wc = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(event_proc),
        hInstance: windows::Win32::Foundation::HINSTANCE(hmod.0),
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW) }.expect("LoadCursorW"),
        lpszClassName: EVENT_CLASS,
        ..Default::default()
    };
    unsafe { RegisterClassW(&wc) };
    let state = Box::new(EventState {
        parent: cal,
        dark: st.dark,
        hwnd_title: HWND::default(),
        hwnd_date: HWND::default(),
        hwnd_time: HWND::default(),
        hwnd_desc: HWND::default(),
        hwnd_save: HWND::default(),
        hwnd_cancel: HWND::default(),
        hbr_bg: HBRUSH::default(),
        hbr_surface: HBRUSH::default(),
    });
    let raw = Box::into_raw(state);
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            EVENT_CLASS,
            w!("Новое событие — POS Mail"),
            WINDOW_STYLE(WS_OVERLAPPEDWINDOW.0 | WS_CLIPCHILDREN.0),
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            520,
            400,
            Some(cal),
            None,
            Some(windows::Win32::Foundation::HINSTANCE(hmod.0)),
            Some(raw as *const std::ffi::c_void),
        )
        .expect("создать окно события")
    };
    st.hwnd_event_dlg = hwnd;
    unsafe {
        let _ = set_window_text((*raw).hwnd_date, &st.selected);
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetFocus(Some((*raw).hwnd_title));
    }
}

unsafe fn event_layout(hwnd: HWND, st: &EventState) {
    let mut rc = RECT::default();
    if unsafe { GetClientRect(hwnd, &mut rc) }.is_err() {
        return;
    }
    let w = rc.right;
    let h = rc.bottom;
    let lx = 90;
    let fw = (w - lx - 14).max(160);
    let entries: [(HWND, i32, i32, i32, i32); 6] = [
        (st.hwnd_title, lx, 14, fw, 24),
        (st.hwnd_date, lx, 46, 140, 24),
        (st.hwnd_time, lx + 150, 46, 90, 24),
        (st.hwnd_desc, lx, 78, fw, (h - 78 - 56).max(60)),
        (st.hwnd_save, (w - 260).max(lx + 120), h - 40, 120, 28),
        (st.hwnd_cancel, (w - 130).max(lx + 248), h - 40, 116, 28),
    ];
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
                for (h2, x, y, cx, cy) in &entries {
                    let _ = SetWindowPos(*h2, None, *x, *y, *cx, *cy, SWP_NOZORDER);
                }
            }
        }
    }
}

unsafe fn event_brushes(hwnd: HWND, st: &mut EventState) {
    let p = theme::palette(st.dark);
    unsafe {
        if !st.hbr_bg.is_invalid() {
            let _ = DeleteObject(HGDIOBJ(st.hbr_bg.0));
        }
        if !st.hbr_surface.is_invalid() {
            let _ = DeleteObject(HGDIOBJ(st.hbr_surface.0));
        }
        st.hbr_bg = CreateSolidBrush(p.bg);
        st.hbr_surface = CreateSolidBrush(p.surface);
        let _ = InvalidateRect(Some(hwnd), None, true);
    }
}

/// «YYYY-MM-DD» + необязательное «HH:MM» → проверка формата.
fn valid_date(s: &str) -> bool {
    let b = s.as_bytes();
    s.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && s[0..4].bytes().all(|c| c.is_ascii_digit())
        && s[5..7].bytes().all(|c| c.is_ascii_digit())
        && s[8..10].bytes().all(|c| c.is_ascii_digit())
}

fn valid_time(s: &str) -> bool {
    let b = s.as_bytes();
    s.len() == 5
        && b[2] == b':'
        && s[0..2].bytes().all(|c| c.is_ascii_digit())
        && s[3..5].bytes().all(|c| c.is_ascii_digit())
        && s[0..2].parse::<u32>().map(|h| h < 24).unwrap_or(false)
        && s[3..5].parse::<u32>().map(|m| m < 60).unwrap_or(false)
}

unsafe fn save_event(hwnd: HWND, st: &mut EventState) {
    let title = read_text(st.hwnd_title).trim().to_string();
    let date = read_text(st.hwnd_date).trim().to_string();
    let time = read_text(st.hwnd_time).trim().to_string();
    if title.is_empty() {
        msgbox(hwnd, "Название события обязательно.", true);
        let _ = unsafe { SetFocus(Some(st.hwnd_title)) };
        return;
    }
    if !valid_date(&date) {
        msgbox(hwnd, "Дата — в формате ГГГГ-ММ-ДД (например, 2026-11-22).", true);
        let _ = unsafe { SetFocus(Some(st.hwnd_date)) };
        return;
    }
    if !time.is_empty() && !valid_time(&time) {
        msgbox(hwnd, "Время — в формате ЧЧ:ММ (или пусто для события на весь день).", true);
        let _ = unsafe { SetFocus(Some(st.hwnd_time)) };
        return;
    }
    let ev = CalendarEvent {
        id: 0,
        title,
        start_date: date,
        end_date: None,
        start_time: if time.is_empty() { None } else { Some(time) },
        description: read_text(st.hwnd_desc).trim().to_string(),
        source_folder: None,
        source_uid: None,
    };
    match Store::open_default().and_then(|s| s.add_calendar_event(&ev)) {
        Ok(_) => unsafe {
            let _ = PostMessageW(Some(st.parent), WM_APP_EVENT_SAVED, WPARAM(0), LPARAM(0));
            let _ = DestroyWindow(hwnd);
        },
        Err(e) => msgbox(hwnd, &format!("Событие не сохранилось: {e}"), true),
    }
}

unsafe extern "system" fn event_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_NCCREATE => {
            let cs = unsafe { &*(lparam.0 as *const CREATESTRUCTW) };
            unsafe {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_CREATE => {
            let Some(st) = (unsafe { ev_of(hwnd) }) else {
                return LRESULT(-1);
            };
            let hfont = unsafe { GetStockObject(DEFAULT_GUI_FONT) };
            unsafe {
                for (text, y) in [
                    ("Название:", 17),
                    ("Дата:", 49),
                    ("Время:", 49),
                    ("Описание:", 81),
                ] {
                    let x = if text == "Время:" { 236 } else { 14 };
                    let h = child(hwnd, w!("STATIC"), Some(text), 0, x, y, 80, 20, -1);
                    set_font(h, hfont);
                }
                let style = WS_TABSTOP.0 | ES_AUTOHSCROLL as u32 | WS_BORDER.0;
                st.hwnd_title = child(hwnd, w!("EDIT"), None, style, 90, 14, 400, 24, IDC_EV_TITLE);
                st.hwnd_date = child(hwnd, w!("EDIT"), None, style, 90, 46, 140, 24, IDC_EV_DATE);
                st.hwnd_time = child(hwnd, w!("EDIT"), None, style, 310, 46, 90, 24, IDC_EV_TIME);
                st.hwnd_desc = child(
                    hwnd,
                    w!("EDIT"),
                    None,
                    WS_TABSTOP.0
                        | ES_MULTILINE as u32
                        | ES_AUTOVSCROLL as u32
                        | WS_VSCROLL.0
                        | WS_BORDER.0,
                    90,
                    78,
                    400,
                    200,
                    IDC_EV_DESC,
                );
                for h in [st.hwnd_title, st.hwnd_date, st.hwnd_time, st.hwnd_desc] {
                    set_font(h, hfont);
                }
                st.hwnd_save = child(hwnd, w!("BUTTON"), Some("Сохранить"), WS_TABSTOP.0, 250, 340, 120, 28, IDC_EV_SAVE);
                st.hwnd_cancel = child(hwnd, w!("BUTTON"), Some("Отмена"), WS_TABSTOP.0, 380, 340, 116, 28, IDC_EV_CANCEL);
                set_font(st.hwnd_save, hfont);
                set_font(st.hwnd_cancel, hfont);
                event_layout(hwnd, st);
                event_brushes(hwnd, st);
            }
            LRESULT(0)
        }
        WM_GETMINMAXINFO => {
            let mmi = unsafe { &mut *(lparam.0 as *mut MINMAXINFO) };
            mmi.ptMinTrackSize = POINT { x: 420, y: 300 };
            LRESULT(0)
        }
        WM_ERASEBKGND => {
            if let Some(st) = unsafe { ev_of(hwnd) } {
                let p = theme::palette(st.dark);
                let hdc = HDC(wparam.0 as *mut core::ffi::c_void);
                let mut rc = RECT::default();
                if unsafe { GetClientRect(hwnd, &mut rc) }.is_ok() {
                    let br = unsafe { CreateSolidBrush(p.bg) };
                    unsafe { FillRect(hdc, &rc, br) };
                    let _ = unsafe { DeleteObject(HGDIOBJ(br.0)) };
                }
            }
            LRESULT(1)
        }
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = unsafe { BeginPaint(hwnd, &mut ps) };
            if let Some(st) = unsafe { ev_of(hwnd) } {
                let p = theme::palette(st.dark);
                unsafe {
                    let _ = FillRect(hdc, &ps.rcPaint, st.hbr_bg);
                    let _ = SetTextColor(hdc, p.text);
                }
            }
            let _ = unsafe { EndPaint(hwnd, &ps) };
            LRESULT(0)
        }
        WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT => {
            if let Some(st) = unsafe { ev_of(hwnd) } {
                let p = theme::palette(st.dark);
                let hdc = HDC(wparam.0 as *mut core::ffi::c_void);
                unsafe {
                    let _ = SetTextColor(hdc, p.text);
                    let _ = SetBkColor(hdc, p.bg);
                }
                return LRESULT(st.hbr_bg.0 as isize);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_SIZE => {
            if let Some(st) = unsafe { ev_of(hwnd) } {
                unsafe { event_layout(hwnd, st) };
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_COMMAND => {
            let id = (wparam.0 & 0xFFFF) as i32;
            let code = ((wparam.0 >> 16) & 0xFFFF) as u32;
            if lparam.0 != 0 && code == BN_CLICKED {
                match id {
                    IDC_EV_SAVE => {
                        if let Some(st) = unsafe { ev_of(hwnd) } {
                            unsafe { save_event(hwnd, st) };
                        }
                        return LRESULT(0);
                    }
                    IDC_EV_CANCEL => {
                        let _ = unsafe { DestroyWindow(hwnd) };
                        return LRESULT(0);
                    }
                    _ => {}
                }
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        crate::WM_APP_DLG_THEME => {
            if let Some(st) = unsafe { ev_of(hwnd) } {
                st.dark = wparam.0 != 0;
                unsafe { event_brushes(hwnd, st) };
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = unsafe { DestroyWindow(hwnd) };
            LRESULT(0)
        }
        WM_DESTROY => {
            if let Some(st) = unsafe { ev_of(hwnd) } {
                unsafe {
                    if !st.hbr_bg.is_invalid() {
                        let _ = DeleteObject(HGDIOBJ(st.hbr_bg.0));
                    }
                    if !st.hbr_surface.is_invalid() {
                        let _ = DeleteObject(HGDIOBJ(st.hbr_surface.0));
                    }
                    let _ = PostMessageW(Some(st.parent), WM_APP_EVENT_CLOSED, WPARAM(0), LPARAM(0));
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                    drop(Box::from_raw(st as *mut EventState));
                }
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn month_math_is_stable() {
        assert_eq!(days_in_month(2026, 2), 28);
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(2000, 2), 29);
        assert_eq!(days_in_month(1900, 2), 28);
        assert_eq!(days_in_month(2026, 11), 30);
        assert_eq!(shift_month(2026, 1, -1), (2025, 12));
        assert_eq!(shift_month(2026, 12, 1), (2027, 1));
        assert_eq!(shift_month(2026, 11, 0), (2026, 11));
    }

    #[test]
    fn weekdays_start_on_monday() {
        // 2026-11-22 — воскресенье → индекс 6 при понедельнике = 0.
        assert_eq!(weekday_monday(2026, 11, 22), 6);
        // 2026-11-23 — понедельник.
        assert_eq!(weekday_monday(2026, 11, 23), 0);
    }

    #[test]
    fn date_and_time_validation() {
        assert!(valid_date("2026-11-22"));
        assert!(!valid_date("22.11.2026"));
        assert!(!valid_date("2026-1-22"));
        assert!(valid_time("09:05"));
        assert!(valid_time("23:59"));
        assert!(!valid_time("24:00"));
        assert!(!valid_time("9:05"));
    }
}
