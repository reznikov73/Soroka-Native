//! Общие настройки «Сороки» — TASK-020.
//!
//! Порт `src-tauri/src/settings.rs` Soroka дословно (правило WORKFLOW —
//! как для `src/mail/`): категории ИИ, системные промпты и подписи живут
//! в одном файле `%APPDATA%\Soroka\settings.json`, общем для всех ящиков
//! и обоих клиентов. Настройки отдельного ящика (серверы, логин) остаются
//! в `accounts.json`.
//!
//! Отличие от Soroka — запись (TASK-N09): Soroka переписывает файл всей
//! своей моделью (`save()`), мы же **не переносим** этот `save()`. Файл
//! общий, Soroka живёт в нём параллельно — поэтому свои ключи мы пишем
//! только поверх прочитанного файла (read-modify-write), сохраняя всё
//! остальное содержимое как есть, включая поля, которых мы не знаем.
//!
//! Наши ключи (срез N09):
//! - `native_theme`: «dark» (дефолт, как в Soroka) | «light» — тема
//!   оболочки; переехала из layout.json, старое значение переносится
//!   одноразовой миграцией (`init_theme`);
//! - `native_autosync_min`: интервал автопроверки почты в минутах для
//!   будущего трея (N07), 0 — не проверять (дефолт);
//! - `ui.minimize_to_tray` / `ui.notifications` / `ui.autostart` — поля
//!   UiSettings Soroka (TASK-040/099): поведение общее для обоих клиентов.
//!
//! serde по умолчанию игнорирует неизвестные поля — наши ключи не ломают
//! схему Soroka, а её поля не ломают нашу.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Правила, по которым письмо попадает в категорию без ИИ.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct CategoryRule {
    #[serde(default)]
    pub from_contains: Vec<String>,
    #[serde(default)]
    pub subject_contains: Vec<String>,
}

impl CategoryRule {
    fn is_empty(&self) -> bool {
        self.from_contains.iter().all(|s| s.trim().is_empty())
            && self.subject_contains.iter().all(|s| s.trim().is_empty())
    }

    /// Совпадает ли письмо с правилом (без учёта регистра).
    pub fn matches(&self, from: &str, subject: &str) -> bool {
        let f = from.to_lowercase();
        let s = subject.to_lowercase();
        self.from_contains
            .iter()
            .map(|r| r.trim().to_lowercase())
            .any(|r| !r.is_empty() && f.contains(&r))
            || self
                .subject_contains
                .iter()
                .map(|r| r.trim().to_lowercase())
                .any(|r| !r.is_empty() && s.contains(&r))
    }
}

/// Категория разбора. Порядок в списке = приоритет правил.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Category {
    pub name: String,
    #[serde(default = "default_color")]
    pub color: String,
    #[serde(default)]
    pub hint: String,
    #[serde(default)]
    pub rules: CategoryRule,
    /// Встроенные категории нельзя удалить, но можно переименовать правило.
    #[serde(default)]
    pub builtin: bool,
}

fn default_color() -> String {
    "#9aa3b2".to_string()
}

/// Системные промпты. Плейсхолдеры подставляются при вызове.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Prompts {
    pub analysis: String,
    pub reply: String,
    pub improve: String,
    pub translate: String,
}

/// Подпись (как в The Bat!): имя для списка и текст.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Signature {
    pub name: String,
    pub text: String,
    #[serde(default)]
    pub is_default: bool,
}

/// Подпись конкретного ящика — TASK-030.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AccountSignature {
    pub account_id: String,
    #[serde(default)]
    pub text: String,
    #[serde(default = "signature_enabled_by_default")]
    pub enabled: bool,
}

fn signature_enabled_by_default() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub categories: Vec<Category>,
    pub prompts: Prompts,
    /// Старый общий список подписей: остаётся для миграции (TASK-030).
    #[serde(default)]
    pub signatures: Vec<Signature>,
    /// Подписи ящиков: у каждого ящика своя (TASK-030).
    #[serde(default)]
    pub account_signatures: Vec<AccountSignature>,
    #[serde(default)]
    pub appearance: Appearance,
    /// Поведение программы: трей, автозапуск (TASK-040).
    #[serde(default)]
    pub ui: UiSettings,
}

fn default_transparent() -> bool {
    false
}

fn default_true() -> bool {
    true
}

/// Поведение программы (TASK-040).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiSettings {
    /// Кнопка «—» прячет окно в трей.
    #[serde(default = "default_true")]
    pub minimize_to_tray: bool,
    /// Запускать при входе в Windows.
    #[serde(default)]
    pub autostart: bool,
    /// Отложенные действия с отменой: удаление в корзину, «В спам», отправка (TASK-075).
    #[serde(default)]
    pub delayed_actions: bool,
    /// Мгновенное появление писем через IMAP IDLE (TASK-090). Выключено по умолчанию:
    /// на некоторых серверах IDLE ведёт себя непредсказуемо, включаем осознанно.
    #[serde(default)]
    pub idle_enabled: bool,
    /// Показывать уведомления Windows о новых письмах (TASK-099).
    #[serde(default = "default_true")]
    pub notifications: bool,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            minimize_to_tray: true,
            autostart: false,
            delayed_actions: false,
            idle_enabled: false,
            notifications: true,
        }
    }
}

/// Оформление окна.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Appearance {
    /// Прозрачное окно с Mica/Acrylic. `false` — обычное непрозрачное окно.
    #[serde(default = "default_transparent")]
    pub transparent: bool,
    /// Плотный список: одна строка на письмо (TASK-036).
    #[serde(default)]
    pub dense_list: bool,
    /// Маркировка категории в строке письма (TASK-068): скрывается при группировке по категориям.
    #[serde(default = "default_true")]
    pub show_badge: bool,
    /// Сворачивать письма одной переписки в одну строку списка (TASK-091).
    #[serde(default = "default_true")]
    pub threads: bool,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            transparent: false,
            dense_list: false,
            show_badge: true,
            threads: true,
        }
    }
}

impl Settings {
    /// Подпись ящика, если её уже сохраняли.
    pub fn account_signature(&self, account_id: &str) -> Option<&AccountSignature> {
        self.account_signatures
            .iter()
            .find(|s| s.account_id == account_id)
    }

    /// Сохраняет подпись ящика или добавляет новую.
    pub fn set_account_signature(&mut self, account_id: &str, text: &str, enabled: bool) {
        match self
            .account_signatures
            .iter_mut()
            .find(|s| s.account_id == account_id)
        {
            Some(s) => {
                s.text = text.to_string();
                s.enabled = enabled;
            }
            None => self.account_signatures.push(AccountSignature {
                account_id: account_id.to_string(),
                text: text.to_string(),
                enabled,
            }),
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self::defaults()
    }
}

/// Подстановка вида `{key}` — без зависимостей и без сюрпризов с фигурными скобками в тексте.
pub fn apply(template: &str, pairs: &[(&str, &str)]) -> String {
    let mut out = template.to_string();
    for (key, value) in pairs {
        out = out.replace(&format!("{{{key}}}"), value);
    }
    out
}

impl Settings {
    pub fn defaults() -> Self {
        let categories = vec![
            Category {
                name: "Обучение".to_string(),
                color: "#d5b907".to_string(),
                hint: "курсы, вебинары, учебные центры".to_string(),
                rules: CategoryRule {
                    from_contains: Vec::new(),
                    subject_contains: vec![
                        "курс".to_string(),
                        "вебинар".to_string(),
                        "обучени".to_string(),
                        "семинар".to_string(),
                    ],
                },
                builtin: true,
            },
            Category {
                name: "Партнёр 1С".to_string(),
                color: "#60cdff".to_string(),
                hint: "письма от 1С и партнёров: лицензии, акции, информационные письма"
                    .to_string(),
                rules: CategoryRule {
                    from_contains: vec![
                        "1c.ru".to_string(),
                        "1cnw.ru".to_string(),
                        "1С Информационные письма".to_string(),
                    ],
                    subject_contains: Vec::new(),
                },
                builtin: true,
            },
            Category {
                name: "Клиент".to_string(),
                color: "#396bc0".to_string(),
                hint: "письма клиентов и заказчиков".to_string(),
                rules: CategoryRule {
                    from_contains: Vec::new(),
                    subject_contains: Vec::new(),
                },
                builtin: true,
            },
            Category {
                name: "Госорган".to_string(),
                color: "#8fb8ff".to_string(),
                hint: "госуслуги, ведомства, налоговая".to_string(),
                rules: CategoryRule {
                    from_contains: Vec::new(),
                    subject_contains: Vec::new(),
                },
                builtin: true,
            },
            Category {
                name: "Финансы".to_string(),
                color: "#36b069".to_string(),
                hint: "счета, оплаты, банк".to_string(),
                rules: CategoryRule {
                    from_contains: Vec::new(),
                    subject_contains: Vec::new(),
                },
                builtin: true,
            },
            Category {
                name: "Уведомление".to_string(),
                color: "#75aebd".to_string(),
                hint: "сервисные уведомления и подтверждения".to_string(),
                rules: CategoryRule {
                    from_contains: Vec::new(),
                    subject_contains: Vec::new(),
                },
                builtin: true,
            },
            Category {
                name: "Реклама".to_string(),
                color: "#ea6c72".to_string(),
                hint: "рассылки, акции, промо".to_string(),
                rules: CategoryRule {
                    from_contains: Vec::new(),
                    subject_contains: Vec::new(),
                },
                builtin: true,
            },
            Category {
                name: "Личное".to_string(),
                color: "#3feb0f".to_string(),
                hint: "личная переписка".to_string(),
                rules: CategoryRule {
                    from_contains: Vec::new(),
                    subject_contains: Vec::new(),
                },
                builtin: true,
            },
            Category {
                name: "Осторожно".to_string(),
                color: "#ff8a80".to_string(),
                hint: "фишинг, мошенники, подозрительные ссылки".to_string(),
                rules: CategoryRule {
                    from_contains: Vec::new(),
                    subject_contains: Vec::new(),
                },
                builtin: true,
            },
            Category {
                name: "Социальные сети".to_string(),
                color: "#8330b0".to_string(),
                hint: "Письма из социальных сетей".to_string(),
                rules: CategoryRule {
                    from_contains: Vec::new(),
                    subject_contains: Vec::new(),
                },
                builtin: false,
            },
        ];
        let prompts = Prompts {
            analysis: "Ты — почтовый помощник. Разбери письмо и верни ТОЛЬКО JSON без markdown и пояснений, строго по схеме:\n{{\"summary\": \"суть письма в 1–2 предложениях\", \"category\": \"одно из: {categories}\", \"importance\": \"высокая | обычная | низкая\", \"dates\": [\"важные даты и события, если есть\"], \"tasks\": [\"что от вас могут ждать, если есть\"], \"actions\": [\"краткие предложения действий, если уместны\"]}}\nОценка важности: высокая — требует внимания человека (клиенты, госорганы, счета, личное); низкая — массовые рассылки и реклама; обычная — остальное.\nПодсказки по категориям:\n{categories_hint}\n\nТема: {subject}\n\nТекст письма:\n{body}".to_string(),
            reply: "Напиши ответное письмо ОТ ИМЕНИ владельца ящика (ты — не помощник, не сервис).\nСтиль владельца — по примерам ниже (обращение, тон, длина фраз).\nПравила:\n- по-русски, коротко (2–5 предложений), вежливо;\n- начни с приветствия; обращение по имени — только если оно есть во входящем письме, иначе просто «Здравствуйте!»;\n- ответь по сути входящего письма; не выдумывай фактов;\n- НЕ добавляй подпись, прощание, имена, адреса и телефоны — только сам текст ответа; никаких заглушек вроде «ваше имя здесь»;\n- не переноси из примеров разделители строк (--, ---, ===) и служебные строки;\n- без markdown; верни ТОЛЬКО текст письма, без пояснений и кавычек вокруг.\n\nПримеры писем владельца:\n{style}\n\nВходящее письмо — от: {from}; тема: {subject}\n{body}".to_string(),
            improve: "Ты — внимательный редактор. Перепиши текст письма лучше: исправь орфографию, пунктуацию, ясность и вежливый деловой тон.\nСохрани смысл, факты, имена, числа и язык оригинала. Ничего не добавляй от себя.\nВерни ТОЛЬКО улучшенный текст письма — без markdown, пояснений и кавычек.\n\nТекст письма:\n{text}".to_string(),
            translate: "Переведи письмо на русский язык.\nСохрани структуру, абзацы, списки, ссылки, имена и числа. Не пересказывай и не добавляй пояснений — только перевод.\nЕсли текст уже на русском, верни его без изменений.\nВерни ТОЛЬКО перевод, без markdown-обёрток и кавычек.\n\nТекст письма:\n{text}".to_string(),
        };
        let signatures = vec![Signature {
            name: "Основная".to_string(),
            text: "С уважением, Андрей Резников".to_string(),
            is_default: true,
        }];
        Self {
            categories,
            prompts,
            signatures,
            account_signatures: Vec::new(),
            appearance: Appearance {
                transparent: false,
                dense_list: false,
                show_badge: true,
                threads: true,
            },
            ui: UiSettings::default(),
        }
    }

    /// Имена категорий через запятую — для строки «одно из: …».
    pub fn category_names(&self) -> String {
        self.categories
            .iter()
            .map(|c| c.name.trim())
            .filter(|n| !n.is_empty())
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Подсказки по категориям — по строке на категорию.
    pub fn categories_hint(&self) -> String {
        let lines: Vec<String> = self
            .categories
            .iter()
            .filter(|c| !c.hint.trim().is_empty())
            .map(|c| format!("- {}: {}", c.name.trim(), c.hint.trim()))
            .collect();
        if lines.is_empty() {
            "(без дополнительных подсказок)".to_string()
        } else {
            lines.join("\n")
        }
    }

    /// Первая категория, чьё правило совпало с письмом.
    pub fn match_category(&self, from: &str, subject: &str) -> Option<String> {
        self.categories
            .iter()
            .find(|c| !c.rules.is_empty() && c.rules.matches(from, subject))
            .map(|c| c.name.clone())
    }

    pub fn find_category(&self, name: &str) -> Option<&Category> {
        let n = name.trim().to_lowercase();
        self.categories
            .iter()
            .find(|c| c.name.trim().to_lowercase() == n)
    }

    pub fn analysis_prompt(&self, subject: &str, body: &str) -> String {
        apply(
            &self.prompts.analysis,
            &[
                ("categories", &self.category_names()),
                ("categories_hint", &self.categories_hint()),
                ("subject", subject),
                ("body", body),
            ],
        )
    }

    pub fn reply_prompt(&self, style: &str, from: &str, subject: &str, body: &str) -> String {
        let style = if style.trim().is_empty() {
            "(примеров нет — пиши нейтрально-деловым тоном)"
        } else {
            style
        };
        apply(
            &self.prompts.reply,
            &[
                ("style", style),
                ("from", from),
                ("subject", subject),
                ("body", body),
            ],
        )
    }

    pub fn improve_prompt(&self, text: &str) -> String {
        apply(&self.prompts.improve, &[("text", text)])
    }

    pub fn translate_prompt(&self, text: &str) -> String {
        apply(&self.prompts.translate, &[("text", text)])
    }
}

fn path() -> Result<PathBuf, String> {
    let appdata =
        std::env::var("APPDATA").map_err(|_| "переменная APPDATA недоступна".to_string())?;
    Ok(PathBuf::from(appdata).join("Soroka").join("settings.json"))
}

/// Чтение настроек. Битый или отсутствующий файл — стандартный набор.
pub fn load() -> Settings {
    let Ok(p) = path() else {
        return Settings::defaults();
    };
    let Ok(raw) = fs::read_to_string(&p) else {
        return Settings::defaults();
    };
    match serde_json::from_str::<Settings>(&raw) {
        Ok(mut s) => {
            if s.categories.is_empty() {
                s.categories = Settings::defaults().categories;
            }
            if s.prompts.analysis.trim().is_empty() {
                s.prompts.analysis = Settings::defaults().prompts.analysis;
            }
            if s.prompts.reply.trim().is_empty() {
                s.prompts.reply = Settings::defaults().prompts.reply;
            }
            if s.prompts.improve.trim().is_empty() {
                s.prompts.improve = Settings::defaults().prompts.improve;
            }
            if s.prompts.translate.trim().is_empty() {
                s.prompts.translate = Settings::defaults().prompts.translate;
            }
            s
        }
        Err(_) => Settings::defaults(),
    }
}

// ===== Наши ключи в общем файле (TASK-N09) =====
//
// Пишем только то, что меняем, поверх прочитанного файла; чужие и
// неизвестные поля сохраняем как есть. Функции `*_at` принимают путь —
// для тестов (в приложении путь всегда %APPDATA%\Soroka\settings.json).

/// Что оболочка читает из общего settings.json и меняет в диалоге настроек.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NativeView {
    /// Тема оболочки: тёмная (дефолт, как в Soroka) или светлая.
    pub dark: bool,
    /// Интервал автопроверки почты, минут (N07: трей). 0 — не проверять.
    pub autosync_min: u32,
    /// Кнопка «—» прячет окно в трей — ui.minimize_to_tray Soroka.
    pub minimize_to_tray: bool,
    /// Уведомления Windows о новых письмах — ui.notifications Soroka.
    pub notifications: bool,
    /// Запускать при входе в Windows — ui.autostart Soroka.
    pub autostart: bool,
}

/// Верхняя граница интервала автопроверки (сутки) — больше незачем.
pub const AUTOSYNC_MAX: u32 = 1440;

/// Прочитать наши ключи из общего settings.json.
pub fn native_view() -> NativeView {
    file_path()
        .map(|p| native_view_at(&p))
        .unwrap_or_else(default_view)
}

fn default_view() -> NativeView {
    NativeView {
        dark: true,
        autosync_min: 0,
        minimize_to_tray: true,
        notifications: true,
        autostart: false,
    }
}

fn native_view_at(p: &Path) -> NativeView {
    let v = read_value_at(p);
    // ui.* — поля Soroka: разбираем через UiSettings, serde-дефолты как в ядре.
    let ui = v
        .get("ui")
        .cloned()
        .and_then(|u| serde_json::from_value::<UiSettings>(u).ok())
        .unwrap_or_default();
    NativeView {
        dark: v.get("native_theme").and_then(|t| t.as_str()) != Some("light"),
        autosync_min: v
            .get("native_autosync_min")
            .and_then(|n| n.as_u64())
            .unwrap_or(0)
            .min(AUTOSYNC_MAX as u64) as u32,
        minimize_to_tray: ui.minimize_to_tray,
        notifications: ui.notifications,
        autostart: ui.autostart,
    }
}

/// Тема при старте (TASK-N09): settings.json имеет приоритет; если нашего
/// ключа там нет, а старый layout.json хранил тему (N05) — переносим её
/// одноразово. Нечего переносить и файла нет — дефолт (тёмная), файл не
/// создаём.
pub fn init_theme(layout_theme: Option<&str>) -> bool {
    file_path()
        .map(|p| init_theme_at(&p, layout_theme))
        .unwrap_or_else(|| layout_theme != Some("light"))
}

fn init_theme_at(p: &Path, layout_theme: Option<&str>) -> bool {
    if let Some(raw) = fs::read_to_string(p).ok()
        && let Ok(v) = serde_json::from_str::<Value>(&raw)
        && let Some(t) = v.get("native_theme").and_then(|t| t.as_str())
    {
        return t != "light";
    }
    // Нашего ключа нет: переносим тему из layout.json, если она там была.
    let Some(t) = layout_theme else {
        return true;
    };
    let mut v = read_value_at(p);
    v["native_theme"] = Value::String(t.to_string());
    write_value_at(p, &v);
    t != "light"
}

/// Сохранить только тему (быстрое переключение «Вид → Тёмная тема»).
pub fn store_theme(dark: bool) {
    if let Some(p) = file_path() {
        store_theme_at(&p, dark);
    }
}

fn store_theme_at(p: &Path, dark: bool) {
    let mut v = read_value_at(p);
    v["native_theme"] = Value::String(if dark { "dark" } else { "light" }.to_string());
    write_value_at(p, &v);
}

/// Сохранить всё, что меняет диалог настроек: наши ключи + поля ui.* Soroka.
/// Остальное содержимое файла (категории, промпты, подписи, неизвестные
/// поля) не трогается.
pub fn store_native(view: &NativeView) {
    if let Some(p) = file_path() {
        store_native_at(&p, view);
    }
}

fn store_native_at(p: &Path, view: &NativeView) {
    let mut v = read_value_at(p);
    v["native_theme"] = Value::String(if view.dark { "dark" } else { "light" }.to_string());
    v["native_autosync_min"] = Value::from(view.autosync_min);
    // ui.* — поля Soroka: объект создаём только если его нет или он битый;
    // чужие поля раздела (delayed_actions, idle_enabled) сохраняем.
    if !v.get("ui").is_some_and(|u| u.is_object()) {
        v["ui"] = Value::Object(serde_json::Map::new());
    }
    if let Some(ui) = v.get_mut("ui").and_then(Value::as_object_mut) {
        ui.insert(
            "minimize_to_tray".to_string(),
            Value::from(view.minimize_to_tray),
        );
        ui.insert("notifications".to_string(), Value::from(view.notifications));
        ui.insert("autostart".to_string(), Value::from(view.autostart));
    }
    write_value_at(p, &v);
}

fn file_path() -> Option<PathBuf> {
    path().ok()
}

/// Файл целиком как Value; отсутствующий/битый — полный набор дефолтов
/// (как если бы Soroka сам создал файл при первом сохранении).
fn read_value_at(p: &Path) -> Value {
    fs::read_to_string(p)
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .unwrap_or_else(|| {
            serde_json::to_value(Settings::defaults())
                .unwrap_or_else(|_| Value::Object(serde_json::Map::new()))
        })
}

fn write_value_at(p: &Path, v: &Value) {
    if let Some(dir) = p.parent() {
        let _ = fs::create_dir_all(dir);
    }
    if let Ok(body) = serde_json::to_string_pretty(v) {
        let _ = fs::write(p, body);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_signature_roundtrip() {
        let mut s = Settings::defaults();
        assert!(s.account_signatures.is_empty());
        assert!(s.account_signature("1").is_none());
        s.set_account_signature("1", "С уважением, Александр", true);
        s.set_account_signature("2", "Коротко: Александр", false);
        assert_eq!(
            s.account_signature("1").map(|x| x.text.as_str()),
            Some("С уважением, Александр")
        );
        assert_eq!(s.account_signature("2").map(|x| x.enabled), Some(false));
        s.set_account_signature("1", "Новый текст", false);
        assert_eq!(s.account_signatures.len(), 2);
        assert_eq!(
            s.account_signature("1").map(|x| x.text.as_str()),
            Some("Новый текст")
        );
        assert_eq!(s.account_signature("1").map(|x| x.enabled), Some(false));
    }

    #[test]
    fn defaults_have_1c_and_study() {
        let s = Settings::defaults();
        assert!(s.find_category("Партнёр 1С").is_some());
        assert!(s.find_category("Обучение").is_some());
        assert!(s.category_names().contains("Клиент"));
    }

    #[test]
    fn old_settings_default_to_opaque() {
        let raw = r#"{"categories":[],"prompts":{"analysis":"a","reply":"r","improve":"i","translate":"t"}}"#;
        let s: Settings = serde_json::from_str(raw).unwrap();
        assert!(!s.appearance.transparent);
    }

    #[test]
    fn old_ui_settings_default_to_instant_actions() {
        // TASK-075: в старых настройках нет delayed_actions — действия мгновенные.
        let raw = r#"{"categories":[],"prompts":{"analysis":"a","reply":"r","improve":"i","translate":"t"},"ui":{"minimize_to_tray":true,"autostart":false}}"#;
        let s: Settings = serde_json::from_str(raw).unwrap();
        assert!(!s.ui.delayed_actions);
        assert!(s.ui.minimize_to_tray);
    }

    #[test]
    fn rule_matches_sender_or_subject() {
        let s = Settings::defaults();
        assert_eq!(
            s.match_category("1С Информационные письма <info@1c.ru>", "Вебинар"),
            Some("Обучение".to_string())
        );
        assert_eq!(
            s.match_category("zakaz@1cnw.ru", "Лицензии"),
            Some("Партнёр 1С".to_string())
        );
        assert_eq!(s.match_category("boss@example.com", "Привет"), None);
    }

    #[test]
    fn placeholders_are_substituted() {
        let s = Settings::defaults();
        let p = s.analysis_prompt("Тема", "Тело");
        assert!(p.contains("Партнёр 1С"));
        assert!(p.contains("Тема"));
        assert!(!p.contains("{categories}"));
    }

    #[test]
    fn json_roundtrip_keeps_categories() {
        let s = Settings::defaults();
        let raw = serde_json::to_string(&s).unwrap();
        let back: Settings = serde_json::from_str(&raw).unwrap();
        assert_eq!(back.categories.len(), s.categories.len());
        assert_eq!(back.categories[0].name, s.categories[0].name);
    }

    // ===== Наши ключи: чтение, RMW-запись, миграция темы (TASK-N09) =====

    fn tmp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("soroka-settings9-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("settings.json")
    }

    #[test]
    fn native_view_reads_keys_and_ui() {
        let p = tmp("view");
        std::fs::write(
            &p,
            r#"{"native_theme":"light","native_autosync_min":15,
                "ui":{"minimize_to_tray":false,"notifications":true,"autostart":true,
                      "delayed_actions":true},
                "unknown_field": 42}"#,
        )
        .unwrap();
        let v = native_view_at(&p);
        assert!(!v.dark);
        assert_eq!(v.autosync_min, 15);
        assert!(!v.minimize_to_tray);
        assert!(v.notifications);
        assert!(v.autostart);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn native_view_defaults_when_file_missing() {
        let p = tmp("missing");
        let v = native_view_at(&p);
        assert_eq!(v, default_view()); // тёмная, без автопроверки, ui как у Soroka
    }

    #[test]
    fn store_native_preserves_rest_of_file() {
        let p = tmp("rmw");
        std::fs::write(
            &p,
            r##"{"categories":[{"name":"Своё","color":"#111111","hint":"","rules":{}}],
                "prompts":{"analysis":"a","reply":"r","improve":"i","translate":"t"},
                "signatures":[{"name":"Подпись","text":"текст"}],
                "ui":{"delayed_actions":true,"idle_enabled":true},
                "unknown_field": 42}"##,
        )
        .unwrap();
        store_native_at(
            &p,
            &NativeView {
                dark: false,
                autosync_min: 10,
                minimize_to_tray: false,
                notifications: false,
                autostart: true,
            },
        );
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        // Наши ключи записаны.
        assert_eq!(v["native_theme"], "light");
        assert_eq!(v["native_autosync_min"], 10);
        assert_eq!(v["ui"]["minimize_to_tray"], false);
        assert_eq!(v["ui"]["notifications"], false);
        assert_eq!(v["ui"]["autostart"], true);
        // Чужие поля ui сохранены.
        assert_eq!(v["ui"]["delayed_actions"], true);
        assert_eq!(v["ui"]["idle_enabled"], true);
        // Разделы Soroka и неизвестные поля не тронуты.
        assert_eq!(v["categories"][0]["name"], "Своё");
        assert_eq!(v["signatures"][0]["name"], "Подпись");
        assert_eq!(v["unknown_field"], 42);
        // И читается обратно тем же видом.
        let back = native_view_at(&p);
        assert_eq!(back.autosync_min, 10);
        assert!(!back.minimize_to_tray);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn store_native_creates_ui_section_when_absent() {
        let p = tmp("no-ui");
        std::fs::write(&p, r#"{"unknown_field": 42}"#).unwrap();
        store_native_at(&p, &default_view());
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(v["ui"]["minimize_to_tray"], true);
        assert_eq!(v["ui"]["notifications"], true);
        assert_eq!(v["ui"]["autostart"], false);
        assert_eq!(v["unknown_field"], 42);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn init_theme_migrates_from_layout_once() {
        let p = tmp("migrate");
        std::fs::write(
            &p,
            r#"{"categories":[],"prompts":{"analysis":"a","reply":"r","improve":"i","translate":"t"}}"#,
        )
        .unwrap();
        // В layout.json была светлая (N05) — переносим, файл дополняется.
        assert!(!init_theme_at(&p, Some("light")));
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(v["native_theme"], "light");
        assert_eq!(v["prompts"]["analysis"], "a"); // прочее не тронуто
        // Повторный запуск: settings.json уже главный, layout больше не смотрим.
        assert!(!init_theme_at(&p, Some("dark")));
        assert_eq!(
            serde_json::from_str::<Value>(&std::fs::read_to_string(&p).unwrap()).unwrap()["native_theme"],
            "light"
        );
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn init_theme_settings_wins_over_layout() {
        let p = tmp("priority");
        std::fs::write(&p, r#"{"native_theme":"dark"}"#).unwrap();
        assert!(init_theme_at(&p, Some("light"))); // settings.json главнее
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn init_theme_nothing_to_migrate_does_not_create_file() {
        let p = tmp("nomigrate");
        // Нет ключа и переносить нечего — дефолт, файл не создаём.
        assert!(init_theme_at(&p, None));
        assert!(!p.exists());
    }

    #[test]
    fn store_theme_touches_only_theme() {
        let p = tmp("theme");
        std::fs::write(&p, r#"{"native_autosync_min":15,"unknown_field":42}"#).unwrap();
        store_theme_at(&p, false);
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(v["native_theme"], "light");
        assert_eq!(v["native_autosync_min"], 15);
        assert_eq!(v["unknown_field"], 42);
        let _ = std::fs::remove_file(&p);
    }
}
