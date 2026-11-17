//! Модель данных ИИ-разбора письма — дословно из Soroka (`ai/gigachat.rs`,
//! TASK-010), без сетевой части. Структура хранится в `soroka.db` в колонках
//! `ai_summary`/`ai_category`/…, поэтому поля менять нельзя.

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
