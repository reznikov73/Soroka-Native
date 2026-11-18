//! Состояние главного окна (живёт в GWLP_USERDATA, трогается только UI-потоком).

use std::collections::HashMap;

use webview2_com::Microsoft::Web::WebView2::Win32::{ICoreWebView2, ICoreWebView2Controller};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{HBRUSH, HFONT};
use windows::Win32::UI::WindowsAndMessaging::HICON;

use crate::backend::Backend;
use crate::mail::accounts::Account;
use crate::mail::body::AttachmentMeta;
use crate::mail::imap::MessageHeader;
use crate::ui::{WF_DEF, WL_DEF, WR_DEF};

/// Узел дерева папок: либо заголовок ящика, либо папка с ключом «id::ИМЯ».
// id/email и name заполняются при построении дерева, но UI пока работает по ключу
// «account::folder». Понадобятся в Фазе 3 (контакты/подписи) — поля не удаляем.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum TreeSlot {
    Account { id: String, email: String },
    Folder { key: String, name: String },
}

/// Строка списка: заголовок письма + папка, если строка из результатов поиска
/// (TASK-N03). В режиме папки folder = None — строка относится к current_folder,
/// в режиме поиска — Some(«id::ПАПКА») у каждой строки своя. body_hit — совпадение
/// только в теле письма (маркер в колонке «Папка», TASK-N10).
#[derive(Debug, Clone)]
pub struct ListRow {
    pub hdr: MessageHeader,
    pub folder: Option<String>,
    pub body_hit: bool,
}

/// Тело текущего письма (для показа и цитирования при ответе).
/// С TASK-N05 хранит и вложения с признаком обрезки — чтобы переключение
/// темы могло перерисовать панель без похода в бэкенд.
#[derive(Clone)]
pub struct PaneBody {
    pub uid: u32,
    pub text: Option<String>,
    pub html: Option<String>,
    pub attachments: Vec<AttachmentMeta>,
    pub truncated: bool,
}

/// Главное состояние приложения.
pub struct AppState {
    /// Рабочий поток почты (создаётся после окна: нужен HWND).
    pub backend: Option<Backend>,
    /// Ящики из accounts.json.
    pub accounts: Vec<Account>,
    /// Письма списка: папка или результаты поиска (виртуальный список отдаёт
    /// их по LVN_GETDISPINFO).
    pub rows: Vec<ListRow>,
    /// Ключ текущей папки «id::ИМЯ» (None — папка не выбрана).
    pub current_folder: Option<String>,
    /// Ящик текущей папки (для кнопки «написать»).
    pub current_account: Option<String>,
    /// Активный поиск (TASK-N03): Some(q) — список показывает/ждёт результаты
    /// «q» (пустая строка — поиск открыт, запрос ещё не выполнялся);
    /// None — обычный режим папки.
    pub search_query: Option<String>,
    /// UID выбранного письма (для ответа/удаления/вложений).
    pub selected_uid: Option<u32>,
    /// Тело показанного письма (для цитирования при ответе).
    pub pane_body: Option<PaneBody>,
    /// Слоты дерева (lParam узла = индекс сюда).
    pub tree_slots: Vec<TreeSlot>,
    /// Открыто ли окно написания (одновременно — только одно).
    pub hwnd_compose: Option<HWND>,
    /// Открыто ли окно ящика.
    pub hwnd_account: Option<HWND>,
    /// Открыто ли окно настроек (TASK-N09; одновременно — только одно).
    pub hwnd_settings: Option<HWND>,

    // Трей и автопроверка (TASK-N07).
    /// Иконка трея без непрочитанных (конверт Сороки).
    pub hicon_tray: HICON,
    /// Иконка трея с точкой непрочитанных (TASK-079).
    pub hicon_tray_alert: HICON,
    /// Иконка добавлена в область уведомлений (для TaskbarCreated).
    pub tray_added: bool,
    /// Общее число непрочитанных по всем ящикам (подпись/иконка трея).
    pub unread_total: u32,
    /// Снимки UID кэша INBOX перед синхронизацией — для «новых писем»
    /// (ключ → UIDs). Снимок пустого кэша не кладём: первая проверка
    /// нового ящика — базлайн без уведомления.
    pub autosync_known: HashMap<String, Vec<u32>>,
    /// Подсказку «свёрнут в трей» показали (раз за запуск; Сорока
    /// показывает её при каждом сворачивании, balloon у нас заметнее).
    pub hide_hint_shown: bool,
    /// Сообщение TaskbarCreated (возврат иконки после рестарта Explorer).
    pub taskbar_created: u32,
    /// Кнопка «—» и крестик прячут окно в трей — ui.minimize_to_tray
    /// Soroka (кэш native_view, обновляется при сохранении настроек).
    pub minimize_to_tray: bool,
    /// Уведомления Windows о новых письмах — ui.notifications Soroka.
    pub notifications: bool,
    /// Интервал автопроверки, минут (0 — не проверять) — native_autosync_min.
    pub autosync_min: u32,

    // Дочерние окна и панель WebView2.
    pub hwnd_tree: HWND,
    pub hwnd_list: HWND,
    pub hwnd_status: HWND,
    /// Поле поиска над списком (скрыто, пока поиск не открыт) — TASK-N03.
    pub hwnd_search: HWND,
    /// Сплиттеры панелей (TASK-N04): «дерево|список» и «список|тело».
    pub hwnd_split_tree: HWND,
    pub hwnd_split_pane: HWND,
    /// Веса панелей как в Soroka (weights folders/list/reading) — TASK-N04,
    /// хранятся в %APPDATA%\Soroka\layout.json.
    pub wf: f64,
    pub wl: f64,
    pub wr: f64,
    /// Последняя раскладка в px (для математики перетаскивания сплиттеров).
    pub tree_px: i32,
    pub list_px: i32,
    pub pane_px: i32,
    /// Идёт жест сплиттера — SetBounds панели письма отложен до отпускания
    /// (WebView2 не участвует в кадре жеста, правило WORKFLOW).
    pub dragging: bool,
    /// Тёмная тема (дефолт — как в Soroka; TASK-N05). Хранится в layout.json.
    pub dark: bool,
    /// Кисть фона поля поиска (WM_CTLCOLOREDIT; пересоздаётся при смене темы).
    pub hbr_edit: HBRUSH,
    pub controller: Option<ICoreWebView2Controller>,
    pub webview: Option<ICoreWebView2>,

    // Шрифты (системный + полужирный для непрочитанных).
    pub font: HFONT,
    pub font_bold: HFONT,
}

impl AppState {
    pub fn new(font: HFONT, font_bold: HFONT) -> Self {
        Self {
            backend: None,
            accounts: Vec::new(),
            rows: Vec::new(),
            current_folder: None,
            current_account: None,
            search_query: None,
            selected_uid: None,
            pane_body: None,
            tree_slots: Vec::new(),
            hwnd_compose: None,
            hwnd_account: None,
            hwnd_settings: None,
            hicon_tray: HICON::default(),
            hicon_tray_alert: HICON::default(),
            tray_added: false,
            unread_total: 0,
            autosync_known: HashMap::new(),
            hide_hint_shown: false,
            taskbar_created: 0,
            minimize_to_tray: true,
            notifications: true,
            autosync_min: 0,
            hwnd_tree: HWND::default(),
            hwnd_list: HWND::default(),
            hwnd_status: HWND::default(),
            hwnd_search: HWND::default(),
            hwnd_split_tree: HWND::default(),
            hwnd_split_pane: HWND::default(),
            wf: WF_DEF,
            wl: WL_DEF,
            wr: WR_DEF,
            tree_px: 0,
            list_px: 0,
            pane_px: 0,
            dragging: false,
            dark: true,
            hbr_edit: HBRUSH::default(),
            controller: None,
            webview: None,
            font,
            font_bold,
        }
    }
}
