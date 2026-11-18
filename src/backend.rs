//! Почтовый бэкенд: рабочий поток + задачи + события в UI.
//!
//! Порт оркестрации из `src-tauri/src/commands/mod.rs` Soroka: логика та же,
//! транспорт другой — вместо `#[tauri::command]` + tauri-событий здесь
//! mpsc-канал задач и `PostMessageW(WM_APP_MAIL)` в главное окно.
//! Блокирующие операции (IMAP/SMTP/SQLite) никогда не выполняются в UI-потоке.
//!
//! Долгие задачи выполняются строго по очереди в одном рабочем потоке —
//! как в The Bat!: два параллельных IMAP-соединения к одному ящику
//! конфликтуют, а очередь гарантирует порядок «запросил → получил».

use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

use crate::mail::accounts::{self, Account};
use crate::mail::body::{self, AttachmentMeta};
use crate::mail::imap::{self, FETCH_LIMIT};
use crate::mail::smtp::{self, OutgoingAttachment};
use crate::mail::store::{DraftRecord, SearchHit, Store};

/// Лимит выдачи поиска — как у команды Soroka (TASK-033/082).
const SEARCH_LIMIT: usize = 200;

/// Сообщение «пришли события из рабочего потока» (WM_APP + 0).
pub const WM_APP_MAIL: u32 = WM_APP;
/// Ответ окна написания: lParam = *mut SendOutcome.
pub const WM_APP_SEND_RESULT: u32 = WM_APP + 1;
/// Ответ окна ящика (тест соединения): lParam = *mut LoginOutcome.
pub const WM_APP_LOGIN_RESULT: u32 = WM_APP + 2;
/// Ответ на сохранение черновика (TASK-N08): lParam = *mut DraftSaved.
/// Цепочка WM_APP живёт в разных модулях — продолжаем за треем (TASK-N07).
pub const WM_APP_DRAFT_SAVED: u32 = crate::tray::WM_APP_TRAY + 1;

/// Задача для рабочего потока.
pub enum Job {
    /// Синхронизировать папку (для INBOX — заодно обновить список папок).
    SyncFolder { key: String },
    /// Тело письма: кэш → сеть → кэш (порт get_message_body).
    FetchBody { key: String, uid: u32 },
    /// Пометить прочитанным/непрочитанным на сервере и в кэше (set_seen).
    SetSeen { key: String, uid: u32, seen: bool },
    /// Удалить письмо (в корзину сервера, TASK-092-логика из Soroka).
    DeleteMessage { key: String, uid: u32 },
    /// Отправить письмо (порт send_mail + очередь TASK-089 + копия в Отправленные).
    /// draft_id — сохранить и убрать черновик после отправки (TASK-N08).
    SendMail {
        hwnd_reply: HWND,
        account_id: String,
        to: String,
        cc: String,
        subject: String,
        body: String,
        atts: Vec<OutgoingAttachment>,
        draft_id: Option<String>,
    },
    /// Сохранить черновик (порт save_draft, TASK-022/N08): локальная копия
    /// в drafts и IMAP APPEND в папку \Drafts. print — отпечаток полей на
    /// момент постановки задачи (окно редактора по нему поймёт, что уже
    /// сохранено).
    SaveDraft {
        hwnd_reply: HWND,
        account_id: String,
        draft_id: String,
        to: String,
        cc: String,
        subject: String,
        body: String,
        atts: Vec<OutgoingAttachment>,
        print: String,
    },
    /// Сохранить вложение письма в «Загрузки» (порт save_attachment).
    SaveAttachment { key: String, uid: u32, index: u32 },
    /// Проверка логина/пароля для окна ящика.
    TestLogin {
        hwnd_reply: HWND,
        host: String,
        port: u16,
        user: String,
        password: String,
    },
    /// Поиск FTS5 по всем папкам ящика (порт search_messages, TASK-082).
    Search { account_id: String, query: String },
}

// В `Job` лежат HWND (hwnd_reply) — настоящие ручки окон, а не указатели на
// данные: PostMessageW/SendMessageW можно вызывать с ними из любого потока.
// Других не-Send полей в `Job` нет, поэтому отправка задачи в рабочий поток безопасна.
unsafe impl Send for Job {}

/// Событие, доставляемое в UI-поток через общую очередь + PostMessage.
pub enum MailEvent {
    /// Синхронизация папки завершилась (успешно или с ошибкой сети).
    SyncDone {
        key: String,
        // Пока не читается UI (дерево живёт по ключу), но нужно для точечного
        // обновления счётчиков по ящику — Фаза 2.
        #[allow(dead_code)]
        account_id: String,
        total: u32,
        fetched: u32,
        error: Option<String>,
        folders_refreshed: bool,
    },
    /// Тело письма готово к показу.
    BodyReady {
        key: String,
        uid: u32,
        from_cache: bool,
        body: BodyData,
    },
    /// Тело не удалось получить.
    BodyFailed {
        key: String,
        uid: u32,
        error: String,
    },
    /// Письмо помечено прочитанным (сервер + кэш).
    Seen { key: String, uid: u32 },
    /// Письмо помечено непрочитанным.
    Unseen { key: String, uid: u32 },
    /// Письмо удалено (перенесено в корзину).
    Deleted { key: String, uid: u32 },
    /// Вложение сохранено в «Загрузки».
    AttachSaved { path: String },
    /// Вложение не сохранилось.
    AttachFailed { error: String },
    /// Очередь исходящих разослана после синхронизации (TASK-089).
    OutboxFlushed { sent: usize },
    /// Результаты поиска; query — эхо, чтобы UI отбросил устаревшие (TASK-N03).
    SearchDone { query: String, hits: Vec<SearchHit> },
    /// Информационное сообщение в статус-бар.
    Status(String),
}

/// Тело письма для показа (то, что в Soroka уходило в BodyPayload).
pub struct BodyData {
    pub text: Option<String>,
    pub html: Option<String>,
    pub truncated: bool,
    pub attachments: Vec<AttachmentMeta>,
}

/// Результат отправки для окна написания (WM_APP_SEND_RESULT).
pub struct SendOutcome {
    pub ok: bool,
    // Признак «письмо ушло в очередь» (TASK-089): UI пока показывает msg целиком.
    #[allow(dead_code)]
    pub queued: bool,
    pub msg: String,
}

/// Результат проверки соединения (WM_APP_LOGIN_RESULT).
pub struct LoginOutcome {
    pub ok: bool,
    pub msg: String,
}

/// Результат сохранения черновика (WM_APP_DRAFT_SAVED, TASK-N08):
/// окно редактора показывает label, по print помечает состояние
/// сохранённым (окно закрывается, если ждало ответа).
pub struct DraftSaved {
    pub ok: bool,
    /// Отпечаток сохранённого содержимого (эхо из задачи).
    pub print: String,
    /// «✓ сохранено ЧЧ:ММ» / «✓ сохранено локально ЧЧ:ММ».
    pub label: String,
    /// Заметка (причина локального сохранения / текст ошибки).
    pub msg: String,
}

/// Локальное время ЧЧ:ММ для подписи «сохранено» (порт composeSaved).
fn local_hhmm() -> (u32, u32) {
    let t = unsafe { GetLocalTime() };
    (t.wHour as u32, t.wMinute as u32)
}

/// Общая шина: задачи в рабочий поток, события обратно.
pub struct Backend {
    tx: Sender<Job>,
    events: Arc<Mutex<VecDeque<MailEvent>>>,
}

impl Backend {
    /// Запускает рабочий поток; `hwnd_main` — окно, которому приходят PostMessage.
    pub fn spawn(hwnd_main: HWND) -> Self {
        let (tx, rx) = std::sync::mpsc::channel::<Job>();
        let events = Arc::new(Mutex::new(VecDeque::new()));
        let ev_for_thread = Arc::clone(&events);
        // HWND (*mut c_void) не Send: передаём ручку числом и восстанавливаем
        // в рабочем потоке. PostMessageW потокобезопасен.
        let hwnd_raw = hwnd_main.0 as isize;
        std::thread::Builder::new()
            .name("mail-worker".into())
            .spawn(move || {
                let hwnd = HWND(hwnd_raw as *mut std::ffi::c_void);
                worker_loop(rx, ev_for_thread, hwnd)
            })
            .expect("запустить почтовый поток");
        Self { tx, events }
    }

    /// Постановка задачи (неблокирующая; поток живёт всё приложение).
    pub fn post(&self, job: Job) {
        let _ = self.tx.send(job);
    }

    /// Клон канала задач — для окон-редакторов (compose, ящики).
    pub fn sender(&self) -> Sender<Job> {
        self.tx.clone()
    }

    /// Забрать все накопившиеся события (вызывается только в UI-потоке).
    pub fn drain(&self) -> Vec<MailEvent> {
        let mut q = self.events.lock().expect("очередь событий повреждена");
        q.drain(..).collect()
    }
}

/// Цикл рабочего потока: задача → события → PostMessage в UI.
fn worker_loop(rx: Receiver<Job>, events: Arc<Mutex<VecDeque<MailEvent>>>, hwnd: HWND) {
    while let Ok(job) = rx.recv() {
        let produced = handle(job);
        {
            let mut q = events.lock().expect("очередь событий повреждена");
            q.extend(produced);
        }
        // Пустой PostMessage «будит» цикл сообщений UI-потока.
        let _ = unsafe { PostMessageW(Some(hwnd), WM_APP_MAIL, WPARAM(0), LPARAM(0)) };
    }
}

/// Выполнение одной задачи; возвращает события для UI.
fn handle(job: Job) -> Vec<MailEvent> {
    match job {
        Job::SyncFolder { key } => sync_folder(&key),
        Job::FetchBody { key, uid } => fetch_body(&key, uid),
        Job::SetSeen { key, uid, seen } => set_seen(&key, uid, seen),
        Job::DeleteMessage { key, uid } => delete_message(&key, uid),
        Job::SendMail {
            hwnd_reply,
            account_id,
            to,
            cc,
            subject,
            body,
            atts,
            draft_id,
        } => send_mail(
            hwnd_reply,
            &account_id,
            &to,
            &cc,
            &subject,
            &body,
            &atts,
            draft_id.as_deref(),
        ),
        Job::SaveDraft {
            hwnd_reply,
            account_id,
            draft_id,
            to,
            cc,
            subject,
            body,
            atts,
            print,
        } => save_draft(
            hwnd_reply,
            &account_id,
            &draft_id,
            &to,
            &cc,
            &subject,
            &body,
            &atts,
            &print,
        ),
        Job::SaveAttachment { key, uid, index } => save_attachment(&key, uid, index),
        Job::TestLogin {
            hwnd_reply,
            host,
            port,
            user,
            password,
        } => {
            let outcome = match imap::test_login(&host, port, &user, &password) {
                Ok(msg) => LoginOutcome { ok: true, msg },
                Err(e) => LoginOutcome {
                    ok: false,
                    msg: e.to_string(),
                },
            };
            post_boxed(hwnd_reply, WM_APP_LOGIN_RESULT, outcome);
            Vec::new()
        }
        Job::Search { account_id, query } => search(&account_id, &query),
    }
}

/// Порт search_messages (TASK-033/036/082): FTS5 по всем папкам ящика.
fn search(account_id: &str, query: &str) -> Vec<MailEvent> {
    match Store::open_default()
        .and_then(|s| s.search_messages(account_id, None, query, SEARCH_LIMIT))
    {
        Ok(hits) => vec![MailEvent::SearchDone {
            query: query.to_string(),
            hits,
        }],
        Err(e) => vec![MailEvent::Status(format!("поиск: {e}"))],
    }
}

/// Отправка Box-значения окну диалога (освобождает принимающий wndproc).
fn post_boxed<T>(hwnd: HWND, msg: u32, value: T) {
    let raw = Box::into_raw(Box::new(value));
    let _ = unsafe { PostMessageW(Some(hwnd), msg, WPARAM(0), LPARAM(raw as isize)) };
}

/// Порт sync_folder + sync_inbox (TASK-004.1/016/032/038): сеть → кэш → события.
fn sync_folder(key: &str) -> Vec<MailEvent> {
    let (acc, name) = match accounts::resolve_cfg(key) {
        Ok(v) => v,
        Err(e) => {
            return vec![MailEvent::SyncDone {
                key: key.to_string(),
                account_id: String::new(),
                total: 0,
                fetched: 0,
                error: Some(e.to_string()),
                folders_refreshed: false,
            }];
        }
    };
    let is_inbox = name == "INBOX";
    let encoded = imap::encode_mutf7(&name);
    let cfg = acc.config();

    let result = imap::fetch_headers(&cfg, &encoded, FETCH_LIMIT);
    let (total, headers) = match result {
        Ok(v) => v,
        Err(e) => {
            // Оффлайн: показываем кэш, если он есть (как sync_inbox в Soroka).
            return vec![MailEvent::SyncDone {
                key: key.to_string(),
                account_id: acc.id,
                total: 0,
                fetched: 0,
                error: Some(e.to_string()),
                folders_refreshed: false,
            }];
        }
    };
    let fetched = headers.len() as u32;

    let mut store = match Store::open_default() {
        Ok(s) => s,
        Err(e) => return vec![MailEvent::Status(format!("кэш не открылся: {e}"))],
    };
    // TASK-032/038: пустой ответ сервера не стирает кэш.
    let cached = store
        .load_folder(key)
        .map(|v| !v.is_empty())
        .unwrap_or(false);
    let keep_cache = headers.is_empty() && (total > 0 || cached);

    let mut folders_refreshed = false;
    if !keep_cache {
        if is_inbox {
            // Как sync_inbox: заодно обновляем список папок ящика.
            match imap::list_folders(&cfg) {
                Ok(list) => {
                    let keyed: Vec<crate::mail::imap::FolderInfo> = list
                        .iter()
                        .map(|f| crate::mail::imap::FolderInfo {
                            name: accounts::folder_key(&acc.id, &f.name),
                            delimiter: f.delimiter.clone(),
                            is_drafts: f.is_drafts,
                        })
                        .collect();
                    let _ = store.save_sync(&acc.id, total, &headers, &keyed);
                    folders_refreshed = true;
                }
                Err(_) => {
                    let _ = store.save_folder_sync(key, total, fetched, &headers);
                }
            }
        } else {
            let unseen = headers.iter().filter(|m| !m.seen).count() as u32;
            let _ = store.save_folder_sync(key, total, unseen, &headers);
        }
    }

    let mut events = vec![MailEvent::SyncDone {
        key: key.to_string(),
        account_id: acc.id.clone(),
        total,
        fetched,
        error: None,
        folders_refreshed,
    }];

    // Почта пришла — сеть есть: досылаем очередь (TASK-089), только для INBOX.
    if is_inbox && !keep_cache {
        let sent = flush_outbox();
        if sent > 0 {
            events.push(MailEvent::OutboxFlushed { sent });
        }
    }
    events
}

/// Порт get_message_body (TASK-005): кэш → сеть → кэш → BodyData.
fn fetch_body(key: &str, uid: u32) -> Vec<MailEvent> {
    let store = match Store::open_default() {
        Ok(s) => s,
        Err(e) => {
            return vec![MailEvent::BodyFailed {
                key: key.to_string(),
                uid,
                error: e.to_string(),
            }];
        }
    };
    if let Ok(Some(c)) = store.load_body(key, uid) {
        return vec![MailEvent::BodyReady {
            key: key.to_string(),
            uid,
            from_cache: true,
            body: BodyData {
                text: c.text,
                html: c.html,
                truncated: c.truncated,
                attachments: c.attachments,
            },
        }];
    }
    let (acc, name) = match accounts::resolve_cfg(key) {
        Ok(v) => v,
        Err(e) => {
            return vec![MailEvent::BodyFailed {
                key: key.to_string(),
                uid,
                error: e.to_string(),
            }];
        }
    };
    match body::fetch_body(&acc.config(), &name, uid) {
        Ok(parsed) => {
            let _ = store.save_body(
                key,
                uid,
                parsed.text.as_deref(),
                parsed.html.as_deref(),
                parsed.truncated,
                &parsed.attachments,
            );
            vec![MailEvent::BodyReady {
                key: key.to_string(),
                uid,
                from_cache: false,
                body: BodyData {
                    text: parsed.text,
                    html: parsed.html,
                    truncated: parsed.truncated,
                    attachments: parsed.attachments,
                },
            }]
        }
        Err(e) => vec![MailEvent::BodyFailed {
            key: key.to_string(),
            uid,
            error: e.to_string(),
        }],
    }
}

/// Порт set_seen: сервер + кэш (best-effort, ошибки сервера не роняем).
fn set_seen(key: &str, uid: u32, seen: bool) -> Vec<MailEvent> {
    if let Ok((acc, name)) = accounts::resolve_cfg(key) {
        let _ = imap::set_seen(&acc.config(), &name, uid, seen);
    }
    if let Ok(store) = Store::open_default() {
        let _ = store.mark_seen(key, uid, seen);
    }
    if seen {
        vec![MailEvent::Seen {
            key: key.to_string(),
            uid,
        }]
    } else {
        vec![MailEvent::Unseen {
            key: key.to_string(),
            uid,
        }]
    }
}

/// Порт delete_message (TASK-092): корзина сервера + кэш.
fn delete_message(key: &str, uid: u32) -> Vec<MailEvent> {
    let (acc, name) = match accounts::resolve_cfg(key) {
        Ok(v) => v,
        Err(e) => return vec![MailEvent::Status(format!("удаление: {e}"))],
    };
    match imap::move_to_trash(&acc.config(), &name, uid) {
        Ok(_) => {
            if let Ok(store) = Store::open_default() {
                let _ = store.remove_message(key, uid);
            }
            vec![MailEvent::Deleted {
                key: key.to_string(),
                uid,
            }]
        }
        Err(e) => vec![MailEvent::Status(format!("удаление не удалось: {e}"))],
    }
}

/// Порт save_attachment (TASK-006): «Загрузки» пользователя.
fn save_attachment(key: &str, uid: u32, index: u32) -> Vec<MailEvent> {
    let (acc, name) = match accounts::resolve_cfg(key) {
        Ok(v) => v,
        Err(e) => {
            return vec![MailEvent::AttachFailed {
                error: e.to_string(),
            }];
        }
    };
    let raw = match body::fetch_raw(&acc.config(), &name, uid) {
        Ok(v) => v,
        Err(e) => {
            return vec![MailEvent::AttachFailed {
                error: e.to_string(),
            }];
        }
    };
    match body::save_attachment_to_downloads(&raw, index) {
        Ok(path) => vec![MailEvent::AttachSaved { path }],
        Err(e) => vec![MailEvent::AttachFailed {
            error: e.to_string(),
        }],
    }
}

/// Письмо, ждущее отправки (порт QueuedMail из TASK-089).
#[derive(serde::Serialize, serde::Deserialize)]
struct QueuedMail {
    to: String,
    cc: String,
    bcc: String,
    subject: String,
    body: String,
    html: Option<String>,
    files: Vec<QueuedFile>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct QueuedFile {
    name: String,
    mime: String,
    data_b64: String,
}

/// Отличаем «сети нет» от отказа сервера (порт looks_like_offline).
fn looks_like_offline(msg: &str) -> bool {
    let s = msg.to_lowercase();
    if s.contains("authentication") || s.contains("535") || s.contains("неверный пароль")
    {
        return false;
    }
    [
        "connect",
        "connection",
        "timeout",
        "timed out",
        "dns",
        "resolve",
        "refused",
        "unreachable",
        "network",
        "сеть",
        "temporarily",
    ]
    .iter()
    .any(|k| s.contains(k))
}

/// Порт send_mail: сборка → отправка → копия в «Отправленные» / очередь.
#[allow(clippy::too_many_arguments)]
fn send_mail(
    hwnd_reply: HWND,
    account_id: &str,
    to: &str,
    cc: &str,
    subject: &str,
    body: &str,
    atts: &[OutgoingAttachment],
    draft_id: Option<&str>,
) -> Vec<MailEvent> {
    let finish = |outcome: SendOutcome| -> Vec<MailEvent> {
        post_boxed(hwnd_reply, WM_APP_SEND_RESULT, outcome);
        Vec::new()
    };
    let account: Account = match accounts::account_by_id(account_id) {
        Some(a) => a,
        None => match accounts::first_account() {
            Some(a) => a,
            None => {
                return finish(SendOutcome {
                    ok: false,
                    queued: false,
                    msg: "ящик не настроен".into(),
                });
            }
        },
    };
    if to.trim().is_empty() {
        return finish(SendOutcome {
            ok: false,
            queued: false,
            msg: "не указан получатель".into(),
        });
    }
    let cfg = account.config();
    let message = match smtp::build_message_full(
        &cfg, to, cc,
        "", // bcc: в нативном редакторе Фазы 1 не вводится
        subject, body, None, atts,
    ) {
        Ok(m) => m,
        Err(e) => {
            return finish(SendOutcome {
                ok: false,
                queued: false,
                msg: e.to_string(),
            });
        }
    };
    let raw = message.formatted();
    match smtp::send_message(&cfg, to, cc, "", subject, body, None, atts) {
        Ok(()) => {
            let report = save_sent_copy(&cfg, &raw);
            // Черновик отправлен — больше не нужен (порт delete_draft,
            // TASK-022): убираем с сервера и из кэша.
            if let Some(id) = draft_id {
                cleanup_draft(&account, id);
            }
            let mut msg = String::from("письмо отправлено");
            if report.copied {
                msg.push_str(&format!(
                    "; копия в «{}»",
                    report.sent_folder.unwrap_or_default()
                ));
            } else if let Some(err) = report.copy_error {
                msg.push_str(&format!("; копия в «Отправленные» не сохранилась: {err}"));
            }
            finish(SendOutcome {
                ok: true,
                queued: false,
                msg,
            })
        }
        Err(err) => {
            let text = err.to_string();
            if looks_like_offline(&text) {
                // TASK-089: сети нет — письмо в очередь, не теряем.
                use base64::Engine as _;
                let queued = QueuedMail {
                    to: to.to_string(),
                    cc: cc.to_string(),
                    bcc: String::new(),
                    subject: subject.to_string(),
                    body: body.to_string(),
                    html: None,
                    files: atts
                        .iter()
                        .map(|a| QueuedFile {
                            name: a.name.clone(),
                            mime: a.mime.clone(),
                            data_b64: base64::engine::general_purpose::STANDARD.encode(&a.data),
                        })
                        .collect(),
                };
                let payload = match serde_json::to_string(&queued) {
                    Ok(p) => p,
                    Err(e) => {
                        return finish(SendOutcome {
                            ok: false,
                            queued: false,
                            msg: format!("очередь не собралась: {e}"),
                        });
                    }
                };
                match Store::open_default()
                    .and_then(|s| s.queue_outbox(&account.id, to, subject, &payload))
                {
                    Ok(_) => {
                        // Ушло в очередь — текст уже не потеряется; черновик
                        // убираем и тут (как Сорока: ok = отправлено/в очереди).
                        if let Some(id) = draft_id {
                            cleanup_draft(&account, id);
                        }
                        finish(SendOutcome {
                            ok: true,
                            queued: true,
                            msg: "сети нет — письмо поставлено в очередь (уйдёт при следующей синхронизации)".into(),
                        })
                    }
                    Err(e) => finish(SendOutcome {
                        ok: false,
                        queued: false,
                        msg: format!("не удалось поставить в очередь: {e}"),
                    }),
                }
            } else {
                finish(SendOutcome {
                    ok: false,
                    queued: false,
                    msg: text,
                })
            }
        }
    }
}

/// Порт save_draft (TASK-022): локальная копия в drafts + IMAP APPEND
/// в папку \Drafts, старая серверная копия убирается. Отличия от Сороки
/// (сознательные, см. TASK-N08): при недоступности сервера/папки черновик
/// сохраняется локально (uid=None) — текст не теряется, серверная копия
/// появится при первом успешном сохранении; недописанный адрес не мешает
/// сохранить черновик (получатели живут в локальной записи).
#[allow(clippy::too_many_arguments)]
fn save_draft(
    hwnd_reply: HWND,
    account_id: &str,
    draft_id: &str,
    to: &str,
    cc: &str,
    subject: &str,
    body: &str,
    atts: &[OutgoingAttachment],
    print: &str,
) -> Vec<MailEvent> {
    let finish = |ok: bool, local: bool, msg: &str| -> Vec<MailEvent> {
        let (hh, mm) = local_hhmm();
        let label = if local {
            format!("✓ сохранено локально {hh:02}:{mm:02}")
        } else {
            crate::draft::saved_label(hh, mm)
        };
        post_boxed(
            hwnd_reply,
            WM_APP_DRAFT_SAVED,
            DraftSaved {
                ok,
                print: print.to_string(),
                label,
                msg: msg.to_string(),
            },
        );
        Vec::new()
    };
    let account: Account = match accounts::account_by_id(account_id) {
        Some(a) => a,
        None => match accounts::first_account() {
            Some(a) => a,
            None => return finish(false, false, "ящик не настроен"),
        },
    };
    let cfg = account.config();
    // Как в Soroka: письмо без To: не строится — пустое «Кому» собирается
    // адресом самого ящика (в локальной записи остаётся пустым).
    let to_for_build = if to.trim().is_empty() {
        cfg.user.clone()
    } else {
        to.trim().to_string()
    };
    let build = |to_b: &str, cc_b: &str| -> Result<Vec<u8>, String> {
        smtp::build_message_full(&cfg, to_b, cc_b, "", subject, body, None, atts)
            .map(|m| crate::draft::insert_draft_header(&m.formatted(), draft_id))
            .map_err(|e| e.to_string())
    };
    let raw = match build(&to_for_build, cc) {
        Ok(r) => r,
        // Адрес ещё недописан — черновик не должен из-за этого теряться:
        // серверная копия собирается без получателей (они в локальной записи).
        Err(_) => match build(&cfg.user, "") {
            Ok(r) => r,
            Err(e) => return finish(false, false, &e),
        },
    };
    let Ok(mut store) = Store::open_default() else {
        return finish(false, false, "кэш не открылся");
    };
    let prev = store
        .load_drafts(&account.id)
        .ok()
        .and_then(|list| list.into_iter().find(|d| d.id == draft_id));
    // Папка черновиков: из кэша, иначе LIST (как в Soroka; в кэше имена
    // с префиксом ящика — на сервер уходит только имя).
    let folder: Option<String> = store
        .load_folders(&format!("{}::%", account.id))
        .ok()
        .and_then(|fs| imap::find_drafts(&fs))
        .or_else(|| {
            imap::list_folders(&cfg)
                .ok()
                .and_then(|fs| imap::find_drafts(&fs))
        })
        .map(|k| accounts::split_key(&k).1);
    let (uid, server_note): (Option<u32>, Option<String>) = match &folder {
        Some(f) => match imap::append_draft(&cfg, f, &raw, draft_id) {
            Ok(u) => (u, None),
            Err(e) => (None, Some(e.to_string())),
        },
        None => (None, Some("папка черновиков не найдена".into())),
    };
    // Старая серверная копия не нужна, когда есть новая (APPEND всегда
    // создаёт сообщение заново). Без новой копии старую не трогаем.
    if let (Some(f), Some(old)) = (&folder, prev.as_ref().and_then(|p| p.uid))
        && Some(old) != uid
    {
        let _ = imap::delete_permanently(&cfg, f, old);
    }
    let att_triples: Vec<(String, String, Vec<u8>)> = atts
        .iter()
        .map(|a| (a.name.clone(), a.mime.clone(), a.data.clone()))
        .collect();
    let rec = DraftRecord {
        account: account.id.clone(),
        id: draft_id.to_string(),
        folder: folder.clone().unwrap_or_default(),
        uid,
        to_addr: to.to_string(),
        subject: subject.to_string(),
        body: body.to_string(),
        attachments: crate::draft::encode_extra(cc, &att_triples),
        updated_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0),
    };
    if let Err(e) = store.save_draft(&rec) {
        return finish(false, false, &e.to_string());
    }
    match server_note {
        Some(note) => finish(true, true, &note),
        None => finish(true, false, ""),
    }
}

/// После отправки черновик не нужен (порт delete_draft, TASK-022):
/// серверная копия (\Deleted + EXPUNGE) и запись в кэше.
fn cleanup_draft(account: &Account, draft_id: &str) {
    let Ok(mut store) = Store::open_default() else {
        return;
    };
    let Ok(list) = store.load_drafts(&account.id) else {
        return;
    };
    let Some(rec) = list.into_iter().find(|d| d.id == draft_id) else {
        return;
    };
    if let Some(uid) = rec.uid {
        let cfg = account.config();
        let (_, folder) = accounts::split_key(&rec.folder);
        let _ = imap::delete_permanently(&cfg, &folder, uid);
    }
    let _ = store.drop_draft(&account.id, draft_id);
}

/// Итог копии в «Отправленные» (порт SendReport).
struct SentReport {
    copied: bool,
    sent_folder: Option<String>,
    copy_error: Option<String>,
}

/// Порт save_sent_copy (TASK-054).
fn save_sent_copy(cfg: &imap::AccountConfig, raw: &[u8]) -> SentReport {
    if imap::autosaves_sent(cfg) {
        return SentReport {
            copied: false,
            sent_folder: None,
            copy_error: None,
        };
    }
    let folders = match imap::list_folders(cfg) {
        Ok(list) => list,
        Err(err) => {
            return SentReport {
                copied: false,
                sent_folder: None,
                copy_error: Some(format!("не удалось получить список папок: {err}")),
            };
        }
    };
    let Some(name) = imap::find_sent(&folders) else {
        return SentReport {
            copied: false,
            sent_folder: None,
            copy_error: Some("папка «Отправленные» не найдена на сервере".into()),
        };
    };
    match imap::append_sent(cfg, &name, raw) {
        Ok(()) => SentReport {
            copied: true,
            sent_folder: Some(name),
            copy_error: None,
        },
        Err(err) => SentReport {
            copied: false,
            sent_folder: Some(name),
            copy_error: Some(err.to_string()),
        },
    }
}

/// Порт flush_outbox (TASK-089): досылка очереди после успешной синхронизации.
/// Возвращает число отправленных писем.
fn flush_outbox() -> usize {
    let Ok(store) = Store::open_default() else {
        return 0;
    };
    let Ok(items) = store.list_outbox() else {
        return 0;
    };
    let mut sent = 0usize;
    for (id, account_id, payload) in items {
        let Ok(q) = serde_json::from_str::<QueuedMail>(&payload) else {
            let _ = store.drop_outbox(id);
            continue;
        };
        let Some(account) = accounts::account_by_id(&account_id) else {
            let _ = store.mark_outbox_attempt(id, "ящик удалён из настроек");
            break;
        };
        let cfg = account.config();
        let files: Vec<smtp::AttachmentInput> = q
            .files
            .iter()
            .map(|f| smtp::AttachmentInput {
                name: f.name.clone(),
                mime: f.mime.clone(),
                data_b64: f.data_b64.clone(),
            })
            .collect();
        let Ok(atts) = smtp::decode_attachments(&files) else {
            let _ = store.mark_outbox_attempt(id, "вложение не читается");
            continue;
        };
        match smtp::send_message(
            &cfg,
            &q.to,
            &q.cc,
            &q.bcc,
            &q.subject,
            &q.body,
            q.html.as_deref(),
            &atts,
        ) {
            Ok(()) => {
                let _ = store.drop_outbox(id);
                sent += 1;
            }
            Err(err) => {
                let _ = store.mark_outbox_attempt(id, &err.to_string());
                // Сети нет — остальные тоже не уйдут.
                break;
            }
        }
    }
    sent
}
