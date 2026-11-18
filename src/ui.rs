//! Оболочка главного окна: меню, дерево папок, виртуальный список писем,
//! панель письма (HTML-шаблон), статус-бар, применение событий бэкенда.
//!
//! Все функции вызываются только из UI-потока. Блокирующие операции уходят
//! в `backend` (рабочий поток), сюда возвращаются события через WM_APP_MAIL.

use windows::core::{HSTRING, PCWSTR, w};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateFontIndirectW, InvalidateRect, LOGFONTW, SelectObject, HFONT, HGDIOBJ,
};
use windows::Win32::UI::Controls::{
    CDDS_ITEMPREPAINT, CDDS_PREPAINT, CDRF_DODEFAULT, CDRF_NOTIFYITEMDRAW,
    CDRF_NOTIFYSUBITEMDRAW, HTREEITEM, LVCOLUMNW, LVCF_TEXT, LVCF_WIDTH, LVM_DELETECOLUMN,
    LVM_INSERTCOLUMNW, LVM_SETEXTENDEDLISTVIEWSTYLE, LVM_SETITEMCOUNT, LVS_EX_DOUBLEBUFFER,
    LVS_EX_FULLROWSELECT, LVIF_TEXT, NMCUSTOMDRAW, NMLISTVIEW, NMLVDISPINFOW,
    NMTREEVIEWW, TVGN_CARET, TVIF_PARAM, TVIF_TEXT, TVINSERTSTRUCTW,
    TVINSERTSTRUCTW_0, TVI_LAST, TVI_ROOT, TVITEMW, TVM_DELETEITEM, TVM_EXPAND, TVM_INSERTITEMW,
    TVM_SELECTITEM, TVE_EXPAND,
};
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateMenu, CreatePopupMenu, KillTimer, MENU_ITEM_FLAGS, MF_POPUP,
    MF_SEPARATOR, MF_STRING, SendMessageW, SetMenu, ShowWindow, HACCEL, ACCEL,
    CreateAcceleratorTableW, SW_HIDE, SW_SHOW, FCONTROL, FNOINVERT, FSHIFT, FVIRTKEY,
    NONCLIENTMETRICSW,
};

use crate::backend::{BodyData, MailEvent};
use crate::mail::accounts::{load_accounts, split_key};
use crate::mail::imap::MessageHeader;
use crate::mail::store::Store;
use crate::state::{AppState, ListRow, PaneBody, TreeSlot};
use crate::wide::{escape_html, wide};

/// Сообщение статус-бара (SB_SETTEXTW из commctrl: WM_USER + 11).
pub const SB_SET_TEXT: u32 = 0x040B;

/// Раскладка, px.
pub const TREE_W: i32 = 250;
pub const PANE_W: i32 = 470;
pub const STATUS_H: i32 = 26;
/// Высота полосы поиска над списком (TASK-N03).
pub const SEARCH_H: i32 = 30;

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

/// Текст в статус-бар.
pub fn status_text(st: &AppState, text: &str) {
    let mut buf = wide(text);
    unsafe {
        SendMessageW(
            st.hwnd_status,
            SB_SET_TEXT,
            Some(WPARAM(0)),
            Some(LPARAM(buf.as_mut_ptr() as isize)),
        );
    }
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
pub fn create_menu(hwnd: HWND) -> windows::core::Result<HACCEL> {
    unsafe {
        let bar = CreateMenu()?;
        let mail = CreatePopupMenu()?;
        let folder = CreatePopupMenu()?;
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
        item(mail, MF_STRING, IDM_FORWARD as usize, w!("Переслать...\tCtrl+F"));
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
        item(folder, MF_STRING, IDM_SYNC as usize, w!("Синхронизировать\tF5"));

        item(
            accounts,
            MF_STRING,
            IDM_ACCOUNT_ADD as usize,
            w!("Добавить ящик..."),
        );

        item(file, MF_STRING, IDM_EXIT as usize, w!("Выход"));

        item(bar, MF_POPUP, mail.0 as usize, w!("&Почта"));
        item(bar, MF_POPUP, folder.0 as usize, w!("&Папка"));
        item(bar, MF_POPUP, accounts.0 as usize, w!("&Ящики"));
        item(bar, MF_POPUP, file.0 as usize, w!("&Файл"));

        SetMenu(hwnd, Some(bar))?;

        let accels = [
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
        ];
        CreateAcceleratorTableW(&accels)
    }
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
            Some(LPARAM((LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER) as isize)),
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
            SendMessageW(
                hwnd_list,
                LVM_DELETECOLUMN,
                Some(WPARAM(0)),
                None,
            );
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
    let unread = st.rows.iter().filter(|m| !m.seen).count();
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
            SendMessageW(
                hwnd_list,
                LVM_SETITEMCOUNT,
                Some(WPARAM(0)),
                None,
            );
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
            SendMessageW(
                hwnd_list,
                LVM_SETITEMCOUNT,
                Some(WPARAM(0)),
                None,
            );
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
        SendMessageW(
            hwnd_list,
            LVM_SETITEMCOUNT,
            Some(WPARAM(0)),
            None,
        );
        let _ = InvalidateRect(Some(hwnd_list), None, false);
    }
    status_text(st, &format!("ищу «{q}»…"));
    if let Some(b) = &st.backend {
        b.post(crate::backend::Job::Search { account_id, query: q });
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
            None => row.hdr.date.clone(),
        },
        _ => row.hdr.date.clone(),
    };
    copy_utf16(nm.item.pszText, nm.item.cchTextMax, &text);
}

/// Ответ NM_CUSTOMDRAW списка: непрочитанные — полужирным.
/// На каждый ITEMPREPAINT (включая сабайтемы) выбираем нужный шрифт заново.
pub fn on_list_custom_draw(st: &AppState, nm: &NMCUSTOMDRAW) -> LRESULT {
    match nm.dwDrawStage {
        CDDS_PREPAINT => LRESULT(CDRF_NOTIFYITEMDRAW as isize),
        stage if stage.0 & CDDS_ITEMPREPAINT.0 != 0 => {
            let i = nm.dwItemSpec;
            let bold = st.rows.get(i).is_some_and(|r| !r.hdr.seen);
            let hfont = if bold { st.font_bold } else { st.font };
            unsafe {
                SelectObject(nm.hdc, HGDIOBJ(hfont.0));
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
        let _ = unsafe { wv.NavigateToString(&HSTRING::from(loading_html(&row.hdr.subject))) };
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

/// Тёмный шаблон панели письма (WebView2), как в шипе Фазы 0.
pub fn letter_html(hdr: &MessageHeader, body: &BodyData) -> String {
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
body {{ font-family: 'Segoe UI', sans-serif; background: #1c1e24; color: #d6d9e0; margin: 0; }}
.hdr {{ padding: 14px 20px 12px; border-bottom: 1px solid #2a2d35; }}
h1 {{ font-size: 17px; font-weight: 600; margin: 0 0 6px; }}
.meta {{ color: #8b93a3; font-size: 13px; line-height: 1.5; }}
.atts {{ padding: 8px 20px; border-bottom: 1px solid #2a2d35; font-size: 13px; color: #a9b4c9; }}
.atts a {{ color: #6ea8ff; text-decoration: none; border-bottom: 1px dotted #6ea8ff; }}
.warn {{ padding: 8px 20px; color: #d0a95e; font-size: 12px; border-bottom: 1px solid #2a2d35; }}
.body {{ padding: 16px 20px; line-height: 1.55; font-size: 14px; }}
pre {{ white-space: pre-wrap; word-wrap: break-word; font-family: 'Segoe UI', Consolas, monospace; }}
img {{ max-width: 100%; }}
</style></head><body>
<div class="hdr"><h1>{}</h1><div class="meta">От: {}<br>Кому: {}<br>{}</div></div>
{}{}
<div class="body">{}</div>
</body></html>"#,
        escape_html(&hdr.subject),
        escape_html(&hdr.from),
        escape_html(&hdr.to),
        escape_html(&hdr.date),
        atts,
        truncated,
        inner
    )
}

/// Заглушка «загружается».
fn loading_html(subject: &str) -> String {
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><style>
body {{ font-family: 'Segoe UI', sans-serif; background: #1c1e24; color: #8b93a3; margin: 24px; }}
</style></head><body>Загрузка «{}»…</body></html>"#,
        escape_html(subject)
    )
}

/// Пустая панель: папка выбрана, письмо — нет.
pub fn empty_pane_html() -> String {
    r#"<!doctype html><html><head><meta charset="utf-8"><style>
body { font-family: 'Segoe UI', sans-serif; background: #1c1e24; color: #8b93a3; margin: 24px; }
</style></head><body>Выберите письмо в списке.</body></html>"#
        .to_string()
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
    });
    if let Some(wv) = &st.webview {
        let _ = unsafe { wv.NavigateToString(&HSTRING::from(letter_html(&hdr, &body))) };
    }
    status_text(
        st,
        &format!(
            "{}{}",
            hdr.subject,
            if from_cache { " (кэш)" } else { "" }
        ),
    );
    // Прочитано — на сервер и в кэш (как set_seen в Soroka).
    if was_unread && let Some(b) = &st.backend {
        b.post(crate::backend::Job::SetSeen { key: key.to_string(), uid, seen: true });
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
            if st.search_query.is_none()
                && st.current_folder.as_deref() == Some(key.as_str())
            {
                show_folder(hwnd_list, st, &key);
            }
            match error {
                Some(err) => status_text(st, &format!("сеть: {err}")),
                None => status_text(
                    st,
                    &format!("{key}: всего {total}, загружено {fetched}"),
                ),
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
                            wv.NavigateToString(&HSTRING::from(empty_pane_html()))
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
            status_text(st, &format!("очередь: отправлено писем, ждавших сети: {sent}"));
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
        && st.tree_slots.iter().any(|s| matches!(s, TreeSlot::Folder { key: k, .. } if *k == key))
    {
        show_folder(hwnd_list, st, &key);
    }
}
