//! Сохранение компоновки панелей (TASK-N04): веса дерева/списка/тела.
//! Хранится рядом с общими данными Soroka: %APPDATA%\Soroka\layout.json.
//! У Soroka то же самое лежит в localStorage «soroka.layout2» — файлы
//! не конфликтуют (кэш и аккаунты общие, компоновка — своя у каждой оболочки).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize)]
struct Weights {
    folders: f64,
    list: f64,
    reading: f64,
}

#[derive(Serialize, Deserialize)]
struct LayoutFile {
    weights: Weights,
}

fn file_path() -> Option<PathBuf> {
    let mut p = PathBuf::from(std::env::var("APPDATA").ok()?);
    p.push("Soroka");
    p.push("layout.json");
    Some(p)
}

/// Веса из layout.json; None — файла нет или данные невалидны
/// (тогда вызывающий берёт дефолт из Soroka).
pub fn load() -> Option<(f64, f64, f64)> {
    let s = std::fs::read_to_string(file_path()?).ok()?;
    let f: LayoutFile = serde_json::from_str(&s).ok()?;
    let w = f.weights;
    let ok = [w.folders, w.list, w.reading]
        .iter()
        .all(|v| v.is_finite() && *v > 0.0);
    ok.then_some((w.folders, w.list, w.reading))
}

pub fn save(wf: f64, wl: f64, wr: f64) {
    let Some(p) = file_path() else { return };
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let body = serde_json::to_string(&LayoutFile {
        weights: Weights {
            folders: wf,
            list: wl,
            reading: wr,
        },
    });
    if let Ok(body) = body {
        let _ = std::fs::write(p, body);
    }
}
