//! Книга контактов (TASK-N12, часть 2): окно со списком, карточка контакта,
//! импорт/экспорт CSV и vCard.
//!
//! Слой данных целиком в ядре Soroka (`mail::contacts` — модель, CSV/vCard,
//! нормализация; `Store::{load_contacts, save_contact, delete_contact,
//! import_contacts}`), поэтому здесь только Win32-оболочка: список, фильтр,
//! карточка и файловые диалоги. `src/mail/` не правится (правило WORKFLOW).

use std::path::PathBuf;

use windows::core::{PCWSTR, PWSTR, w};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateSolidBrush, DEFAULT_GUI_FONT, DeleteObject, FillRect, HBRUSH, HDC, HGDIOBJ,
    InvalidateRect, SetBkColor, SetTextColor,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::Dialogs::{
    GetOpenFileNameW, GetSaveFileNameW, OFN_EXPLORER, OFN_FILEMUSTEXIST, OFN_HIDEREADONLY,
    OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST, OPENFILENAMEW,
};
use windows::Win32::UI::Controls::{
    CDDS_ITEMPREPAINT, CDDS_PREPAINT, CDRF_DODEFAULT, CDRF_NEWFONT, CDRF_NOTIFYITEMDRAW, LVCF_TEXT,
    LVCF_WIDTH, LVCOLUMNW, LVIF_TEXT, LVITEMW, LVNI_SELECTED, LVM_DELETEALLITEMS, LVM_GETHEADER,
    LVM_GETNEXTITEM, LVM_INSERTCOLUMNW, LVM_INSERTITEMW, LVM_SETBKCOLOR, LVM_SETEXTENDEDLISTVIEWSTYLE,
    LVM_SETITEMTEXTW, LVM_SETTEXTBKCOLOR, LVM_SETTEXTCOLOR, LVS_EX_FULLROWSELECT, LVS_REPORT,
    LVS_SHOWSELALWAYS, LVS_SINGLESEL, NMCUSTOMDRAW, NMHDR, NM_CUSTOMDRAW, NM_DBLCLK,
    SetWindowTheme, WC_LISTVIEW,
};
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::{
    BN_CLICKED, BeginDeferWindowPos, CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT,
    CreateWindowExW, DefWindowProcW, DeferWindowPos, DestroyWindow, EN_CHANGE, ES_AUTOHSCROLL,
    ES_AUTOVSCROLL, ES_MULTILINE, EndDeferWindowPos, GWLP_USERDATA, GetClientRect,
    GetWindowLongPtrW, IDC_ARROW, LoadCursorW, MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MB_YESNO,
    MINMAXINFO, MessageBoxW, PostMessageW, RegisterClassW, SWP_NOZORDER, SendMessageW,
    SetWindowLongPtrW, SetWindowPos, ShowWindow, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLOSE,
    WM_COMMAND, WM_CREATE, WM_CTLCOLOREDIT, WM_CTLCOLORSTATIC, WM_DESTROY, WM_ERASEBKGND,
    WM_GETMINMAXINFO, WM_NCCREATE, WM_NOTIFY, WM_PAINT, WM_SETFONT, WM_SIZE, WNDCLASSW, WS_BORDER,
    WS_CHILD,
    WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_OVERLAPPEDWINDOW, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOW;

use crate::mail::contacts::{self, Contact};
use crate::mail::store::Store;
use crate::state::AppState;
use crate::theme;
use crate::wide::{read_text, set_window_text, wide};

/// Главное окно узнаёт о закрытии книги (снимает hwnd_contacts).
pub const WM_APP_CONTACTS_CLOSED: u32 = crate::backend::WM_APP_DRAFT_SAVED + 1;
/// Карточка сохранила контакт — книге пора перечитать список.
const WM_APP_CARD_SAVED: u32 = WM_APP_CONTACTS_CLOSED + 1;
/// Карточка закрылась (любым способом) — книга забывает её HWND.
const WM_APP_CARD_CLOSED: u32 = WM_APP_CARD_SAVED + 1;

const WINDOW_CLASS: PCWSTR = w!("SorokaContactsWnd");
const CARD_CLASS: PCWSTR = w!("SorokaContactCardWnd");

/// Контролы книги.
const IDC_FILTER: i32 = 500;
const IDC_LIST: i32 = 501;
const IDC_BTN_ADD: i32 = 502;
const IDC_BTN_EDIT: i32 = 503;
const IDC_BTN_DEL: i32 = 504;
const IDC_BTN_IMPORT: i32 = 505;
const IDC_BTN_EXPORT: i32 = 506;
const IDC_BTN_CLOSE: i32 = 507;
const IDC_STATUS: i32 = 508;

/// Контролы карточки.
const IDC_NAME: i32 = 520;
const IDC_EMAIL: i32 = 521;
const IDC_ORG: i32 = 522;
const IDC_POS: i32 = 523;
const IDC_PHONE: i32 = 524;
const IDC_GROUPS: i32 = 525;
const IDC_NOTES: i32 = 526;
const IDC_BTN_SAVE: i32 = 527;
const IDC_BTN_CANCEL: i32 = 528;

/// Окно книги (GWLP_USERDATA).
struct BookState {
    parent: HWND,
    hwnd_filter: HWND,
    hwnd_list: HWND,
    hwnd_status: HWND,
    hwnd_btn_add: HWND,
    hwnd_btn_edit: HWND,
    hwnd_btn_del: HWND,
    hwnd_btn_import: HWND,
    hwnd_btn_export: HWND,
    hwnd_btn_close: HWND,
    contacts: Vec<Contact>,
    /// Индексы `contacts`, попавшие в текущий фильтр (строка списка → индекс).
    shown: Vec<usize>,
    dark: bool,
    hbr_bg: HBRUSH,
    hbr_surface: HBRUSH,
    hwnd_card: HWND,
}

/// Окно карточки (GWLP_USERDATA).
struct CardState {
    parent: HWND,
    dark: bool,
    /// Редактируемый контакт: None — новый.
    contact: Option<Contact>,
    hwnd_name: HWND,
    hwnd_email: HWND,
    hwnd_org: HWND,
    hwnd_pos: HWND,
    hwnd_phone: HWND,
    hwnd_groups: HWND,
    hwnd_notes: HWND,
    hwnd_btn_save: HWND,
    hwnd_btn_cancel: HWND,
    hbr_bg: HBRUSH,
    hbr_surface: HBRUSH,
}

/// Открывает книгу контактов (одна на приложение).
pub fn open(hwnd_main: HWND, st: &AppState) -> Option<HWND> {
    if let Some(h) = st.hwnd_contacts {
        let _ = unsafe { SetFocus(Some(h)) };
        return None;
    }
    let hmod = unsafe { GetModuleHandleW(None) }.expect("GetModuleHandleW");
    let wc = WNDCLASSW {
        // Перерисовка клиента целиком при ресайзе — как у прочих окон.
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(wndproc),
        hInstance: windows::Win32::Foundation::HINSTANCE(hmod.0),
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW) }.expect("LoadCursorW"),
        lpszClassName: WINDOW_CLASS,
        ..Default::default()
    };
    unsafe { RegisterClassW(&wc) };

    let state = Box::new(BookState {
        parent: hwnd_main,
        hwnd_filter: HWND::default(),
        hwnd_list: HWND::default(),
        hwnd_status: HWND::default(),
        hwnd_btn_add: HWND::default(),
        hwnd_btn_edit: HWND::default(),
        hwnd_btn_del: HWND::default(),
        hwnd_btn_import: HWND::default(),
        hwnd_btn_export: HWND::default(),
        hwnd_btn_close: HWND::default(),
        contacts: Vec::new(),
        shown: Vec::new(),
        dark: st.dark,
        hbr_bg: HBRUSH::default(),
        hbr_surface: HBRUSH::default(),
        hwnd_card: HWND::default(),
    });
    let raw = Box::into_raw(state);

    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            WINDOW_CLASS,
            w!("Контакты — POS Mail"),
            WINDOW_STYLE(WS_OVERLAPPEDWINDOW.0 | WS_CLIPCHILDREN.0),
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            860,
            560,
            Some(hwnd_main),
            None,
            Some(windows::Win32::Foundation::HINSTANCE(hmod.0)),
            Some(raw as *const std::ffi::c_void),
        )
        .expect("создать окно контактов")
    };
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetFocus(Some((*raw).hwnd_filter));
    }
    Some(hwnd)
}

unsafe fn st_of(hwnd: HWND) -> Option<&'static mut BookState> {
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut BookState;
    if ptr.is_null() {
        None
    } else {
        Some(unsafe { &mut *ptr })
    }
}

unsafe fn card_of(hwnd: HWND) -> Option<&'static mut CardState> {
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut CardState;
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

unsafe fn move_window(hwnd: HWND, x: i32, y: i32, w: i32, h: i32) {
    let _ = unsafe { SetWindowPos(hwnd, None, x, y, w, h, SWP_NOZORDER) };
}

/// Раскладка книги: фильтр сверху, список, кнопки, статус.
unsafe fn layout(hwnd: HWND, st: &BookState) {
    let mut rc = RECT::default();
    if unsafe { GetClientRect(hwnd, &mut rc) }.is_err() {
        return;
    }
    let w = rc.right;
    let h = rc.bottom;
    let m = 12;
    let row_y = h - 40;
    let entries: [(HWND, i32, i32, i32, i32); 8] = [
        (st.hwnd_filter, m + 56, 12, (w - m * 2 - 56).max(140), 24),
        (st.hwnd_list, m, 44, (w - m * 2).max(200), (h - 44 - 56).max(120)),
        (st.hwnd_btn_add, m, row_y, 110, 28),
        (st.hwnd_btn_edit, m + 118, row_y, 110, 28),
        (st.hwnd_btn_del, m + 236, row_y, 100, 28),
        (st.hwnd_btn_import, m + 344, row_y, 110, 28),
        (st.hwnd_btn_export, m + 462, row_y, 110, 28),
        (st.hwnd_btn_close, (w - 112).max(m + 580), row_y, 100, 28),
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
                    move_window(*h2, *x, *y, *cx, *cy);
                }
            }
        }
        let _ = SetWindowPos(
            st.hwnd_status,
            None,
            m,
            row_y - 24,
            (w - m * 2).max(200),
            20,
            SWP_NOZORDER,
        );
        // Перекладывание детей обнажает полосы фона — перерисовываем клиент
        // целиком (иначе после старта остаются белые полосы в тёмной теме).
        let _ = InvalidateRect(Some(hwnd), None, true);
    }
}

/// Раскладка карточки: подписи слева, поля справа, заметки и кнопки снизу.
unsafe fn card_layout(hwnd: HWND, st: &CardState) {
    let mut rc = RECT::default();
    if unsafe { GetClientRect(hwnd, &mut rc) }.is_err() {
        return;
    }
    let w = rc.right;
    let h = rc.bottom;
    let m = 14;
    let lx = m + 96;
    let fw = (w - lx - m).max(160);
    let entries: [(HWND, i32, i32, i32, i32); 9] = [
        (st.hwnd_name, lx, 14, fw, 24),
        (st.hwnd_email, lx, 46, fw, 24),
        (st.hwnd_org, lx, 78, fw, 24),
        (st.hwnd_pos, lx, 110, fw, 24),
        (st.hwnd_phone, lx, 142, fw, 24),
        (st.hwnd_groups, lx, 174, fw, 24),
        (st.hwnd_notes, lx, 206, fw, (h - 206 - 56).max(60)),
        (st.hwnd_btn_save, (w - 260).max(lx + 120), h - 40, 120, 28),
        (st.hwnd_btn_cancel, (w - 130).max(lx + 248), h - 40, 116, 28),
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
                    move_window(*h2, *x, *y, *cx, *cy);
                }
            }
        }
    }
}

/// Кисти и subtheme книги.
unsafe fn apply_brushes(hwnd: HWND, st: &mut BookState) {
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
        let sub = if st.dark {
            w!("DarkMode_Explorer")
        } else {
            w!("Explorer")
        };
        // Поле фильтра — без темы: WM_CTLCOLOREDIT красит его нашей кистью,
        // а themed EDIT рисует свой фон сам.
        let _ = SetWindowTheme(st.hwnd_filter, w!(""), w!(""));
        // Тема списку нужна ради тёмной шапки и скроллбаров; строки сами
        // рисуем в NM_CUSTOMDRAW — с темой comctl32 v6 красит их своей
        // белой кистью и игнорирует LVM_SETTEXTBKCOLOR.
        let _ = SetWindowTheme(st.hwnd_list, sub, PCWSTR::null());
        // Шапка колонок — отдельный контрол: ей тема тоже нужна, иначе
        // в тёмной палитре остаётся светлая полоса (как в ui.rs).
        let hdr = SendMessageW(st.hwnd_list, LVM_GETHEADER, Some(WPARAM(0)), None);
        if hdr.0 != 0 {
            let _ = SetWindowTheme(
                HWND(hdr.0 as *mut core::ffi::c_void),
                sub,
                PCWSTR::null(),
            );
        }
        let _ = SendMessageW(
            st.hwnd_list,
            LVM_SETBKCOLOR,
            Some(WPARAM(0)),
            Some(LPARAM(p.surface.0 as isize)),
        );
        let _ = SendMessageW(
            st.hwnd_list,
            LVM_SETTEXTCOLOR,
            Some(WPARAM(0)),
            Some(LPARAM(p.text.0 as isize)),
        );
        let _ = SendMessageW(
            st.hwnd_list,
            LVM_SETTEXTBKCOLOR,
            Some(WPARAM(0)),
            Some(LPARAM(p.surface.0 as isize)),
        );
        // Список мог отрисоваться до установки цветов — просим перерисовать
        // его целиком, иначе остаётся белым пятном в тёмной теме.
        let _ = InvalidateRect(Some(st.hwnd_list), None, true);
        let _ = InvalidateRect(Some(st.hwnd_filter), None, true);
        let _ = InvalidateRect(Some(hwnd), None, true);
    }
}

/// Кисти и subtheme карточки.
unsafe fn card_brushes(hwnd: HWND, st: &mut CardState) {
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
        let sub = if st.dark {
            w!("DarkMode_Explorer")
        } else {
            w!("Explorer")
        };
        for h in [
            st.hwnd_name,
            st.hwnd_email,
            st.hwnd_org,
            st.hwnd_pos,
            st.hwnd_phone,
            st.hwnd_groups,
            st.hwnd_notes,
        ] {
            let _ = SetWindowTheme(h, sub, PCWSTR::null());
        }
        let _ = InvalidateRect(Some(hwnd), None, true);
    }
}

/// Индексы контактов под фильтр: подстрока по имени, адресу, организации,
/// группам и телефону (регистр не важен; пустой фильтр — все).
fn filter_contacts(list: &[Contact], needle: &str) -> Vec<usize> {
    let n = needle.trim().to_lowercase();
    if n.is_empty() {
        return (0..list.len()).collect();
    }
    list.iter()
        .enumerate()
        .filter(|(_, c)| {
            [&c.name, &c.email, &c.organization, &c.groups, &c.phone]
                .iter()
                .any(|f| f.to_lowercase().contains(&n))
        })
        .map(|(i, _)| i)
        .collect()
}

/// Перечитывает книгу из кэша и заполняет список.
unsafe fn reload(st: &mut BookState) {
    st.contacts = Store::open_default()
        .and_then(|s| s.load_contacts())
        .unwrap_or_default();
    unsafe { fill_list(st) };
}

/// Заполняет список по текущему фильтру и обновляет статус.
unsafe fn fill_list(st: &mut BookState) {
    let needle = read_text(st.hwnd_filter);
    st.shown = filter_contacts(&st.contacts, &needle);
    unsafe {
        SendMessageW(st.hwnd_list, LVM_DELETEALLITEMS, None, None);
        for (row, idx) in st.shown.iter().enumerate() {
            let c = &st.contacts[*idx];
            let cols = [
                c.name.clone(),
                c.email.clone(),
                c.organization.clone(),
                c.phone.clone(),
                c.groups.clone(),
            ];
            for (sub, text) in cols.iter().enumerate() {
                let mut buf = wide(text);
                let item = LVITEMW {
                    mask: LVIF_TEXT,
                    iItem: row as i32,
                    iSubItem: sub as i32,
                    pszText: PWSTR(buf.as_mut_ptr()),
                    cchTextMax: buf.len() as i32,
                    ..Default::default()
                };
                if sub == 0 {
                    SendMessageW(
                        st.hwnd_list,
                        LVM_INSERTITEMW,
                        None,
                        Some(LPARAM(&item as *const LVITEMW as isize)),
                    );
                } else {
                    SendMessageW(
                        st.hwnd_list,
                        LVM_SETITEMTEXTW,
                        Some(WPARAM(row)),
                        Some(LPARAM(&item as *const LVITEMW as isize)),
                    );
                }
            }
        }
    }
    let text = if needle.trim().is_empty() {
        format!("контактов: {}", st.contacts.len())
    } else {
        format!("контактов: {}, по фильтру: {}", st.contacts.len(), st.shown.len())
    };
    let _ = set_window_text(st.hwnd_status, &text);
}

/// Выбранный контакт (по строке списка через карту `shown`).
fn selected(st: &BookState) -> Option<Contact> {
    let row = unsafe {
        SendMessageW(
            st.hwnd_list,
            LVM_GETNEXTITEM,
            Some(WPARAM(-1i32 as usize)),
            Some(LPARAM(LVNI_SELECTED as isize)),
        )
    }
    .0;
    if row < 0 {
        return None;
    }
    st.shown
        .get(row as usize)
        .and_then(|i| st.contacts.get(*i))
        .cloned()
}

/// Заголовки колонок книги.
unsafe fn setup_columns(hwnd_list: HWND) {
    let cols = [
        ("Имя", 220),
        ("E-mail", 240),
        ("Организация", 190),
        ("Телефон", 130),
        ("Группы", 130),
    ];
    for (i, (title, cx)) in cols.iter().enumerate() {
        let mut buf = wide(title);
        let col = LVCOLUMNW {
            mask: LVCF_TEXT | LVCF_WIDTH,
            pszText: PWSTR(buf.as_mut_ptr()),
            cchTextMax: buf.len() as i32,
            cx: *cx,
            ..Default::default()
        };
        unsafe {
            SendMessageW(
                hwnd_list,
                LVM_INSERTCOLUMNW,
                Some(WPARAM(i)),
                Some(LPARAM(&col as *const LVCOLUMNW as isize)),
            );
        }
    }
    unsafe {
        SendMessageW(
            hwnd_list,
            LVM_SETEXTENDEDLISTVIEWSTYLE,
            Some(WPARAM(0)),
            Some(LPARAM(LVS_EX_FULLROWSELECT as isize)),
        );
    }
}

/// Диалог выбора файла: открыть (true) или сохранить (false).
unsafe fn ask_file(hwnd: HWND, open: bool) -> Option<PathBuf> {
    let mut buf = vec![0u16; 4096];
    let filter = wide("Контакты (*.csv;*.vcf)\0*.csv;*.vcf\0CSV (*.csv)\0*.csv\0vCard (*.vcf)\0*.vcf\0Все файлы\0*.*\0\0");
    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: hwnd,
        lpstrFilter: PCWSTR(filter.as_ptr()),
        lpstrFile: PWSTR(buf.as_mut_ptr()),
        nMaxFile: buf.len() as u32,
        Flags: OFN_EXPLORER
            | OFN_PATHMUSTEXIST
            | OFN_HIDEREADONLY
            | if open {
                OFN_FILEMUSTEXIST
            } else {
                OFN_OVERWRITEPROMPT
            },
        ..Default::default()
    };
    let ok = if open {
        unsafe { GetOpenFileNameW(&mut ofn) }
    } else {
        unsafe { GetSaveFileNameW(&mut ofn) }
    };
    if !ok.as_bool() {
        return None;
    }
    let len = buf.iter().position(|&u| u == 0).unwrap_or(0);
    if len == 0 {
        return None;
    }
    Some(PathBuf::from(String::from_utf16_lossy(&buf[..len])))
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

/// Импорт CSV/vCard: разбор ядром, вставка через Store::import_contacts.
unsafe fn on_import(hwnd: HWND, st: &mut BookState) {
    let Some(path) = (unsafe { ask_file(hwnd, true) }) else {
        return;
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        msgbox(hwnd, "Не удалось прочитать файл.", true);
        return;
    };
    let vcf = path
        .extension()
        .map(|e| e.to_string_lossy().eq_ignore_ascii_case("vcf"))
        .unwrap_or(false);
    let list = if vcf {
        contacts::parse_vcf(&text)
    } else {
        contacts::parse_csv(&text)
    };
    if list.is_empty() {
        msgbox(hwnd, "В файле не нашлось контактов.", true);
        return;
    }
    match Store::open_default().and_then(|s| s.import_contacts(&list)) {
        Ok(n) => {
            unsafe { reload(st) };
            msgbox(hwnd, &format!("Импортировано контактов: {n}."), false);
        }
        Err(e) => msgbox(hwnd, &format!("Импорт не удался: {e}"), true),
    }
}

/// Экспорт всей книги в CSV или vCard.
unsafe fn on_export(hwnd: HWND, st: &BookState) {
    if st.contacts.is_empty() {
        msgbox(hwnd, "Книга пуста — экспортировать нечего.", false);
        return;
    }
    let Some(mut path) = (unsafe { ask_file(hwnd, false) }) else {
        return;
    };
    let vcf = path
        .extension()
        .map(|e| e.to_string_lossy().eq_ignore_ascii_case("vcf"))
        .unwrap_or(false);
    if path.extension().is_none() {
        path.set_extension(if vcf { "vcf" } else { "csv" });
    }
    let text = if vcf {
        contacts::to_vcf(&st.contacts)
    } else {
        contacts::to_csv(&st.contacts)
    };
    match std::fs::write(&path, text) {
        Ok(()) => msgbox(
            hwnd,
            &format!("Сохранено: {}", path.display()),
            false,
        ),
        Err(e) => msgbox(hwnd, &format!("Не удалось записать файл: {e}"), true),
    }
}

/// Удаление выбранного контакта с подтверждением.
unsafe fn on_delete(hwnd: HWND, st: &mut BookState) {
    let Some(c) = selected(st) else {
        msgbox(hwnd, "Сначала выберите контакт в списке.", false);
        return;
    };
    let text = wide(&format!("Удалить контакт «{}» ({})?", c.name, c.email));
    let answer = unsafe {
        MessageBoxW(
            Some(hwnd),
            PCWSTR(text.as_ptr()),
            w!("POS Mail"),
            MB_YESNO | MB_ICONINFORMATION,
        )
    };
    if answer != windows::Win32::UI::WindowsAndMessaging::IDYES {
        return;
    }
    match Store::open_default().and_then(|s| s.delete_contact(c.id)) {
        Ok(()) => unsafe { reload(st) },
        Err(e) => msgbox(hwnd, &format!("Удалить не удалось: {e}"), true),
    }
}

/// Карточка контакта: None — новый.
unsafe fn open_card(book: HWND, st: &mut BookState, contact: Option<Contact>) {
    if !st.hwnd_card.0.is_null() {
        let _ = unsafe { SetFocus(Some(st.hwnd_card)) };
        return;
    }
    let hmod = unsafe { GetModuleHandleW(None) }.expect("GetModuleHandleW");
    let wc = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(card_proc),
        hInstance: windows::Win32::Foundation::HINSTANCE(hmod.0),
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW) }.expect("LoadCursorW"),
        lpszClassName: CARD_CLASS,
        ..Default::default()
    };
    unsafe { RegisterClassW(&wc) };

    let title = if contact.is_some() {
        "Контакт — POS Mail"
    } else {
        "Новый контакт — POS Mail"
    };
    let state = Box::new(CardState {
        parent: book,
        dark: st.dark,
        contact,
        hwnd_name: HWND::default(),
        hwnd_email: HWND::default(),
        hwnd_org: HWND::default(),
        hwnd_pos: HWND::default(),
        hwnd_phone: HWND::default(),
        hwnd_groups: HWND::default(),
        hwnd_notes: HWND::default(),
        hwnd_btn_save: HWND::default(),
        hwnd_btn_cancel: HWND::default(),
        hbr_bg: HBRUSH::default(),
        hbr_surface: HBRUSH::default(),
    });
    let raw = Box::into_raw(state);
    let mut title_buf = wide(title);
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CARD_CLASS,
            PCWSTR(title_buf.as_mut_ptr()),
            WINDOW_STYLE(WS_OVERLAPPEDWINDOW.0 | WS_CLIPCHILDREN.0),
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            560,
            420,
            Some(book),
            None,
            Some(windows::Win32::Foundation::HINSTANCE(hmod.0)),
            Some(raw as *const std::ffi::c_void),
        )
        .expect("создать карточку контакта")
    };
    st.hwnd_card = hwnd;
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetFocus(Some((*raw).hwnd_name));
    }
}

/// Заполняет поля карточки значениями контакта.
unsafe fn fill_fields(st: &CardState) {
    if let Some(c) = &st.contact {
        let _ = set_window_text(st.hwnd_name, &c.name);
        let _ = set_window_text(st.hwnd_email, &c.email);
        let _ = set_window_text(st.hwnd_org, &c.organization);
        let _ = set_window_text(st.hwnd_pos, &c.position);
        let _ = set_window_text(st.hwnd_phone, &c.phone);
        let _ = set_window_text(st.hwnd_groups, &c.groups);
        let _ = set_window_text(st.hwnd_notes, &c.notes);
    }
}

/// Сохранение карточки: валидация и запись в кэш ядра.
unsafe fn save_card(hwnd: HWND, st: &mut CardState) {
    let email = read_text(st.hwnd_email).trim().to_string();
    if !email.contains('@') {
        msgbox(hwnd, "E-mail обязателен и должен содержать «@».", true);
        let _ = unsafe { SetFocus(Some(st.hwnd_email)) };
        return;
    }
    let contact = Contact {
        id: st.contact.as_ref().map(|c| c.id).unwrap_or(0),
        name: read_text(st.hwnd_name).trim().to_string(),
        email,
        organization: read_text(st.hwnd_org).trim().to_string(),
        position: read_text(st.hwnd_pos).trim().to_string(),
        phone: read_text(st.hwnd_phone).trim().to_string(),
        notes: read_text(st.hwnd_notes).trim().to_string(),
        groups: read_text(st.hwnd_groups).trim().to_string(),
    };
    match Store::open_default().and_then(|s| s.save_contact(&contact)) {
        Ok(_) => {
            unsafe {
                let _ = PostMessageW(Some(st.parent), WM_APP_CARD_SAVED, WPARAM(0), LPARAM(0));
                let _ = DestroyWindow(hwnd);
            }
        }
        Err(e) => msgbox(hwnd, &format!("Сохранить не удалось: {e}"), true),
    }
}

/// Рисование строк списка своей кистью: с темой comctl32 v6 красит их
/// белым и игнорирует LVM_SETTEXTBKCOLOR. Заливаем строку цветом панели
/// и отдаём контролу только текст (CDRF_NEWFONT).
unsafe fn on_list_custom_draw(st: &BookState, nm: &NMCUSTOMDRAW) -> LRESULT {
    let stage = nm.dwDrawStage.0;
    if stage == CDDS_PREPAINT.0 {
        return LRESULT(CDRF_NOTIFYITEMDRAW as isize);
    }
    if stage == CDDS_ITEMPREPAINT.0 {
        let p = theme::palette(st.dark);
        unsafe {
            if !st.hbr_surface.is_invalid() {
                FillRect(nm.hdc, &nm.rc, st.hbr_surface);
            }
            let _ = SetTextColor(nm.hdc, p.text);
        }
        return LRESULT(CDRF_NEWFONT as isize);
    }
    LRESULT(CDRF_DODEFAULT as isize)
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
            let hfont = unsafe { windows::Win32::Graphics::Gdi::GetStockObject(DEFAULT_GUI_FONT) };
            unsafe {
                let label = child(hwnd, w!("STATIC"), Some("Поиск:"), 0, 12, 16, 52, 20, -1);
                set_font(label, hfont);
                st.hwnd_filter = child(
                    hwnd,
                    w!("EDIT"),
                    None,
                    WS_TABSTOP.0 | ES_AUTOHSCROLL as u32 | WS_BORDER.0,
                    68,
                    12,
                    400,
                    24,
                    IDC_FILTER,
                );
                set_font(st.hwnd_filter, hfont);
                st.hwnd_list = child(
                    hwnd,
                    WC_LISTVIEW,
                    None,
                    LVS_REPORT | LVS_SHOWSELALWAYS | LVS_SINGLESEL,
                    12,
                    44,
                    600,
                    300,
                    IDC_LIST,
                );
                set_font(st.hwnd_list, hfont);
                setup_columns(st.hwnd_list);
                st.hwnd_btn_add = child(hwnd, w!("BUTTON"), Some("Добавить..."), WS_TABSTOP.0, 12, 0, 110, 28, IDC_BTN_ADD);
                st.hwnd_btn_edit = child(hwnd, w!("BUTTON"), Some("Изменить..."), WS_TABSTOP.0, 130, 0, 110, 28, IDC_BTN_EDIT);
                st.hwnd_btn_del = child(hwnd, w!("BUTTON"), Some("Удалить"), WS_TABSTOP.0, 248, 0, 100, 28, IDC_BTN_DEL);
                st.hwnd_btn_import = child(hwnd, w!("BUTTON"), Some("Импорт..."), WS_TABSTOP.0, 356, 0, 110, 28, IDC_BTN_IMPORT);
                st.hwnd_btn_export = child(hwnd, w!("BUTTON"), Some("Экспорт..."), WS_TABSTOP.0, 474, 0, 110, 28, IDC_BTN_EXPORT);
                st.hwnd_btn_close = child(hwnd, w!("BUTTON"), Some("Закрыть"), WS_TABSTOP.0, 600, 0, 100, 28, IDC_BTN_CLOSE);
                for b in [
                    st.hwnd_btn_add,
                    st.hwnd_btn_edit,
                    st.hwnd_btn_del,
                    st.hwnd_btn_import,
                    st.hwnd_btn_export,
                    st.hwnd_btn_close,
                ] {
                    set_font(b, hfont);
                }
                st.hwnd_status = child(hwnd, w!("STATIC"), Some(""), 0, 12, 0, 300, 20, IDC_STATUS);
                set_font(st.hwnd_status, hfont);
                layout(hwnd, st);
                reload(st);
                // Кисти и цвета списка — после заполнения: вставка строк могла
                // сбросить цвета контрола на дефолтные.
                apply_brushes(hwnd, st);
            }
            LRESULT(0)
        }
        WM_GETMINMAXINFO => {
            let mmi = unsafe { &mut *(lparam.0 as *mut MINMAXINFO) };
            mmi.ptMinTrackSize = POINT { x: 620, y: 360 };
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
            // На всякий случай красим клиент и в WM_PAINT: если система не
            // пришлёт WM_ERASEBKGND (класс без фоновой кисти), окно не
            // останется белым пятном в тёмной теме.
            let mut ps = windows::Win32::Graphics::Gdi::PAINTSTRUCT::default();
            let hdc = unsafe { windows::Win32::Graphics::Gdi::BeginPaint(hwnd, &mut ps) };
            if let Some(st) = unsafe { st_of(hwnd) } {
                let p = theme::palette(st.dark);
                let br = unsafe { CreateSolidBrush(p.bg) };
                unsafe { FillRect(hdc, &ps.rcPaint, br) };
                let _ = unsafe { DeleteObject(HGDIOBJ(br.0)) };
            }
            let _ = unsafe { windows::Win32::Graphics::Gdi::EndPaint(hwnd, &ps) };
            LRESULT(0)
        }
        WM_SIZE => {
            if let Some(st) = unsafe { st_of(hwnd) } {
                unsafe { layout(hwnd, st) };
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT => {
            if let Some(st) = unsafe { st_of(hwnd) } {
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
        WM_COMMAND => {
            let id = (wparam.0 & 0xFFFF) as i32;
            let code = ((wparam.0 >> 16) & 0xFFFF) as u32;
            if lparam.0 != 0 && code == EN_CHANGE && id == IDC_FILTER {
                if let Some(st) = unsafe { st_of(hwnd) } {
                    unsafe { fill_list(st) };
                }
                return LRESULT(0);
            }
            if lparam.0 != 0 && code == BN_CLICKED {
                let Some(st) = (unsafe { st_of(hwnd) }) else {
                    return LRESULT(0);
                };
                match id {
                    IDC_BTN_ADD => unsafe { open_card(hwnd, st, None) },
                    IDC_BTN_EDIT => {
                        if let Some(c) = selected(st) {
                            unsafe { open_card(hwnd, st, Some(c)) };
                        } else {
                            msgbox(hwnd, "Сначала выберите контакт в списке.", false);
                        }
                    }
                    IDC_BTN_DEL => unsafe { on_delete(hwnd, st) },
                    IDC_BTN_IMPORT => unsafe { on_import(hwnd, st) },
                    IDC_BTN_EXPORT => unsafe { on_export(hwnd, st) },
                    IDC_BTN_CLOSE => {
                        let _ = unsafe { DestroyWindow(hwnd) };
                    }
                    _ => {}
                }
                return LRESULT(0);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_NOTIFY => {
            let hdr = unsafe { &*(lparam.0 as *const NMHDR) };
            if let Some(st) = unsafe { st_of(hwnd) }
                && hdr.hwndFrom == st.hwnd_list
            {
                if hdr.code == NM_CUSTOMDRAW {
                    let nm = unsafe { &*(lparam.0 as *const NMCUSTOMDRAW) };
                    return unsafe { on_list_custom_draw(st, nm) };
                }
                if hdr.code == NM_DBLCLK {
                    // Двойной клик по строке — карточка контакта.
                    if let Some(c) = selected(st) {
                        unsafe { open_card(hwnd, st, Some(c)) };
                    }
                    return LRESULT(0);
                }
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_APP_CARD_SAVED => {
            if let Some(st) = unsafe { st_of(hwnd) } {
                unsafe { reload(st) };
            }
            LRESULT(0)
        }
        WM_APP_CARD_CLOSED => {
            if let Some(st) = unsafe { st_of(hwnd) } {
                st.hwnd_card = HWND::default();
            }
            LRESULT(0)
        }
        crate::WM_APP_DLG_THEME => {
            if let Some(st) = unsafe { st_of(hwnd) } {
                st.dark = wparam.0 != 0;
                unsafe { apply_brushes(hwnd, st) };
                if !st.hwnd_card.0.is_null() {
                    let _ = unsafe {
                        PostMessageW(
                            Some(st.hwnd_card),
                            crate::WM_APP_DLG_THEME,
                            WPARAM(st.dark as usize),
                            LPARAM(0),
                        )
                    };
                }
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
                    if !st.hbr_bg.is_invalid() {
                        let _ = DeleteObject(HGDIOBJ(st.hbr_bg.0));
                    }
                    if !st.hbr_surface.is_invalid() {
                        let _ = DeleteObject(HGDIOBJ(st.hbr_surface.0));
                    }
                    let _ = PostMessageW(Some(st.parent), WM_APP_CONTACTS_CLOSED, WPARAM(0), LPARAM(0));
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                    drop(Box::from_raw(st as *mut BookState));
                }
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

unsafe extern "system" fn card_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_NCCREATE => {
            let cs = unsafe { &*(lparam.0 as *const CREATESTRUCTW) };
            unsafe {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_CREATE => {
            let Some(st) = (unsafe { card_of(hwnd) }) else {
                return LRESULT(-1);
            };
            let hfont = unsafe { windows::Win32::Graphics::Gdi::GetStockObject(DEFAULT_GUI_FONT) };
            unsafe {
                for (text, y) in [
                    ("Имя:", 17),
                    ("E-mail:", 49),
                    ("Организация:", 81),
                    ("Должность:", 113),
                    ("Телефон:", 145),
                    ("Группы:", 177),
                    ("Заметки:", 209),
                ] {
                    let h = child(hwnd, w!("STATIC"), Some(text), 0, 14, y, 92, 20, -1);
                    set_font(h, hfont);
                }
                let edit_style = WS_TABSTOP.0 | ES_AUTOHSCROLL as u32 | WS_BORDER.0;
                st.hwnd_name = child(hwnd, w!("EDIT"), None, edit_style, 110, 14, 420, 24, IDC_NAME);
                st.hwnd_email = child(hwnd, w!("EDIT"), None, edit_style, 110, 46, 420, 24, IDC_EMAIL);
                st.hwnd_org = child(hwnd, w!("EDIT"), None, edit_style, 110, 78, 420, 24, IDC_ORG);
                st.hwnd_pos = child(hwnd, w!("EDIT"), None, edit_style, 110, 110, 420, 24, IDC_POS);
                st.hwnd_phone = child(hwnd, w!("EDIT"), None, edit_style, 110, 142, 420, 24, IDC_PHONE);
                st.hwnd_groups = child(hwnd, w!("EDIT"), None, edit_style, 110, 174, 420, 24, IDC_GROUPS);
                st.hwnd_notes = child(
                    hwnd,
                    w!("EDIT"),
                    None,
                    WS_TABSTOP.0
                        | ES_MULTILINE as u32
                        | ES_AUTOVSCROLL as u32
                        | WS_VSCROLL.0
                        | WS_BORDER.0,
                    110,
                    206,
                    420,
                    120,
                    IDC_NOTES,
                );
                for h in [
                    st.hwnd_name,
                    st.hwnd_email,
                    st.hwnd_org,
                    st.hwnd_pos,
                    st.hwnd_phone,
                    st.hwnd_groups,
                    st.hwnd_notes,
                ] {
                    set_font(h, hfont);
                }
                st.hwnd_btn_save = child(hwnd, w!("BUTTON"), Some("Сохранить"), WS_TABSTOP.0, 300, 340, 120, 28, IDC_BTN_SAVE);
                st.hwnd_btn_cancel = child(hwnd, w!("BUTTON"), Some("Отмена"), WS_TABSTOP.0, 430, 340, 116, 28, IDC_BTN_CANCEL);
                set_font(st.hwnd_btn_save, hfont);
                set_font(st.hwnd_btn_cancel, hfont);
                fill_fields(st);
                card_layout(hwnd, st);
                card_brushes(hwnd, st);
            }
            LRESULT(0)
        }
        WM_GETMINMAXINFO => {
            let mmi = unsafe { &mut *(lparam.0 as *mut MINMAXINFO) };
            mmi.ptMinTrackSize = POINT { x: 460, y: 320 };
            LRESULT(0)
        }
        WM_ERASEBKGND => {
            if let Some(st) = unsafe { card_of(hwnd) }
                && !st.hbr_bg.is_invalid()
            {
                let hdc = HDC(wparam.0 as *mut core::ffi::c_void);
                let mut rc = RECT::default();
                if unsafe { GetClientRect(hwnd, &mut rc) }.is_ok() {
                    unsafe { FillRect(hdc, &rc, st.hbr_bg) };
                }
            }
            LRESULT(1)
        }
        WM_SIZE => {
            if let Some(st) = unsafe { card_of(hwnd) } {
                unsafe { card_layout(hwnd, st) };
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT => {
            if let Some(st) = unsafe { card_of(hwnd) } {
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
        WM_COMMAND => {
            let id = (wparam.0 & 0xFFFF) as i32;
            let code = ((wparam.0 >> 16) & 0xFFFF) as u32;
            if lparam.0 != 0 && code == BN_CLICKED {
                match id {
                    IDC_BTN_SAVE => {
                        if let Some(st) = unsafe { card_of(hwnd) } {
                            unsafe { save_card(hwnd, st) };
                        }
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
        crate::WM_APP_DLG_THEME => {
            if let Some(st) = unsafe { card_of(hwnd) } {
                st.dark = wparam.0 != 0;
                unsafe { card_brushes(hwnd, st) };
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = unsafe { DestroyWindow(hwnd) };
            LRESULT(0)
        }
        WM_DESTROY => {
            if let Some(st) = unsafe { card_of(hwnd) } {
                unsafe {
                    if !st.hbr_bg.is_invalid() {
                        let _ = DeleteObject(HGDIOBJ(st.hbr_bg.0));
                    }
                    if !st.hbr_surface.is_invalid() {
                        let _ = DeleteObject(HGDIOBJ(st.hbr_surface.0));
                    }
                    // Книга забывает про карточку (иначе не откроется следующая).
                    let _ = PostMessageW(Some(st.parent), WM_APP_CARD_CLOSED, WPARAM(0), LPARAM(0));
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                    drop(Box::from_raw(st as *mut CardState));
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

    fn c(name: &str, email: &str, org: &str) -> Contact {
        Contact {
            id: 0,
            name: name.to_string(),
            email: email.to_string(),
            organization: org.to_string(),
            position: String::new(),
            phone: String::new(),
            notes: String::new(),
            groups: String::new(),
        }
    }

    #[test]
    fn filter_matches_name_email_and_org() {
        let list = vec![
            c("Иван Петров", "ivan@mail.ru", "Ромашка"),
            c("Бухгалтерия", "buh@corp.ru", "ООО Ромашка"),
            c("Мария", "m@ya.ru", ""),
        ];
        assert_eq!(filter_contacts(&list, ""), vec![0, 1, 2]);
        assert_eq!(filter_contacts(&list, "иван"), vec![0]);
        assert_eq!(filter_contacts(&list, "CORP"), vec![1]);
        // Организация тоже участвует.
        assert_eq!(filter_contacts(&list, "ромашка"), vec![0, 1]);
        assert!(filter_contacts(&list, "нет-такого").is_empty());
    }
}
