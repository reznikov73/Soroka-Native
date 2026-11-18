//! Состояние главного окна (живёт в GWLP_USERDATA, трогается только UI-потоком).

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::HFONT;
use webview2_com::Microsoft::Web::WebView2::Win32::{ICoreWebView2, ICoreWebView2Controller};

use crate::backend::Backend;
use crate::mail::accounts::Account;
use crate::mail::imap::MessageHeader;

/// Узел дерева папок: либо заголовок ящика, либо папка с ключом «id::ИМЯ».
// id/email и name заполняются при построении дерева, но UI пока работает по ключу
// «account::folder». Понадобятся в Фазе 3 (контакты/подписи) — поля не удаляем.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum TreeSlot {
    Account { id: String, email: String },
    Folder { key: String, name: String },
}

/// Тело текущего письма (для показа и цитирования при ответе).
pub struct PaneBody {
    pub uid: u32,
    pub text: Option<String>,
    pub html: Option<String>,
}

/// Главное состояние приложения.
pub struct AppState {
    /// Рабочий поток почты (создаётся после окна: нужен HWND).
    pub backend: Option<Backend>,
    /// Ящики из accounts.json.
    pub accounts: Vec<Account>,
    /// Письма текущей папки (виртуальный список отдаёт их по LVN_GETDISPINFO).
    pub rows: Vec<MessageHeader>,
    /// Ключ текущей папки «id::ИМЯ» (None — папка не выбрана).
    pub current_folder: Option<String>,
    /// Ящик текущей папки (для кнопки «написать»).
    pub current_account: Option<String>,
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

    // Дочерние окна и панель WebView2.
    pub hwnd_tree: HWND,
    pub hwnd_list: HWND,
    pub hwnd_status: HWND,
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
            selected_uid: None,
            pane_body: None,
            tree_slots: Vec::new(),
            hwnd_compose: None,
            hwnd_account: None,
            hwnd_tree: HWND::default(),
            hwnd_list: HWND::default(),
            hwnd_status: HWND::default(),
            controller: None,
            webview: None,
            font,
            font_bold,
        }
    }
}
