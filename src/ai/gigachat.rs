//! Модель данных ИИ-разбора письма — дословно из Soroka (`ai/gigachat.rs`,
//! TASK-010), без сетевой части. Структура хранится в `soroka.db` в колонках
//! `ai_summary`/`ai_category`/…, поэтому поля менять нельзя.
//!
//! Ключ GigaChat (Soroka TASK-041) перенесён в N09: диалогу настроек нужно
//! показать/изменить ключ. Сетевая часть (токены, запросы) вернётся в N13
//! вместе с `AiError`; пока ошибки — `Option`/`Result<String>`.

use serde::{Deserialize, Serialize};

/// Структурированный разбор письма (TASK-010).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AiAnalysis {
    pub summary: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub importance: String,
    #[serde(default)]
    pub dates: Vec<String>,
    #[serde(default)]
    pub tasks: Vec<String>,
    #[serde(default)]
    pub actions: Vec<String>,
}

// ===== Ключ GigaChat (Soroka TASK-041) =====

use std::path::{Path, PathBuf};

/// Разбор JSON конфига {"api_key": "..."} (вынесено для тестов).
pub fn parse_api_key(raw: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    let k = v.get("api_key")?.as_str()?.trim().to_string();
    if k.is_empty() { None } else { Some(k) }
}

/// Путь к файлу с ключом (TASK-041).
pub fn key_path() -> Option<PathBuf> {
    let appdata = std::env::var("APPDATA").ok()?;
    Some(PathBuf::from(appdata).join("Soroka").join("gigachat.json"))
}

fn env_key() -> Option<String> {
    std::env::var("SOROKA_GIGACHAT_KEY")
        .ok()
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())
}

/// Ключ: env SOROKA_GIGACHAT_KEY или %APPDATA%\Soroka\gigachat.json.
/// Пока не вызывается оболочкой — сетевая часть (N13) начнёт с него;
/// паритет API Soroka (TASK-041).
#[allow(dead_code)]
pub fn load_api_key() -> Option<String> {
    if let Some(k) = env_key() {
        return Some(k);
    }
    let path = key_path()?;
    let raw = std::fs::read_to_string(&path).ok()?;
    let raw = raw.trim_start_matches('\u{feff}');
    parse_api_key(raw)
}

/// Состояние ключа для интерфейса — сам ключ наружу не отдаём (TASK-041).
/// Поля has_file/env_set сейчас не читаются (диалогу достаточно source) —
/// пригодятся в N13; паритет структуры со структурой Soroka.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct KeyStatus {
    /// "env" | "file" | "none"
    pub source: String,
    pub has_file: bool,
    pub env_set: bool,
}

pub fn key_status() -> KeyStatus {
    let env_set = env_key().is_some();
    let has_file = key_path().is_some_and(|p| p.exists());
    let source = if env_set {
        "env"
    } else if has_file {
        "file"
    } else {
        "none"
    };
    KeyStatus {
        source: source.to_string(),
        has_file,
        env_set,
    }
}

/// Ключ из файла (для диалога настроек N09: показать/изменить).
/// Env-ключ не отдаём — он не наш и не показывается.
pub fn file_key() -> Option<String> {
    let path = key_path()?;
    let raw = std::fs::read_to_string(&path).ok()?;
    parse_api_key(raw.trim_start_matches('\u{feff}'))
}

/// Сохраняет ключ в %APPDATA%\Soroka\gigachat.json (TASK-041).
/// В отличие от Soroka — read-modify-write: прочие поля файла сохраняются
/// (проблема общего файла, как у settings.json — TASK-N09).
pub fn save_api_key(key: &str) -> Result<(), String> {
    let key = key.trim();
    if key.len() < 20 {
        return Err("ключ выглядит слишком коротким".to_string());
    }
    let path = key_path().ok_or("путь к gigachat.json недоступен".to_string())?;
    save_api_key_at(&path, key)
}

fn save_api_key_at(path: &Path, key: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let mut v: serde_json::Value = std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    v["api_key"] = serde_json::Value::String(key.to_string());
    let body = serde_json::to_vec_pretty(&v).map_err(|e| e.to_string())?;
    std::fs::write(path, body).map_err(|e| e.to_string())
}

/// Удаляет сохранённый ключ (TASK-041). Env-ключ SOROKA_GIGACHAT_KEY
/// (если задан) остаётся и имеет приоритет.
pub fn clear_api_key() -> Result<(), String> {
    if let Some(path) = key_path()
        && path.exists()
    {
        std::fs::remove_file(&path).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_api_key_variants() {
        assert_eq!(
            parse_api_key(r#"{"api_key": "abc"}"#).as_deref(),
            Some("abc")
        );
        assert_eq!(
            parse_api_key(r#"{"api_key": "  trimmed  "}"#).as_deref(),
            Some("trimmed")
        );
        assert!(parse_api_key("{}").is_none());
        assert!(parse_api_key("не json").is_none());
        assert!(parse_api_key(r#"{"api_key": "  "}"#).is_none());
    }

    #[test]
    fn save_is_read_modify_write() {
        let dir = std::env::temp_dir().join(format!("soroka-giga9-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("gigachat.json");
        std::fs::write(
            &p,
            r#"{"api_key":"старый-ключ-0000000000","note":"прочее"}"#,
        )
        .unwrap();
        save_api_key_at(&p, "новый-ключ-0000000000000").unwrap();
        let raw = std::fs::read_to_string(&p).unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["api_key"], "новый-ключ-0000000000000");
        assert_eq!(v["note"], "прочее"); // чужие поля сохранены
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn short_key_rejected() {
        assert_eq!(
            save_api_key("короткий").unwrap_err(),
            "ключ выглядит слишком коротким"
        );
    }
}
