//! Сохранение компоновки панелей (TASK-N04): веса дерева/списка/тела.
//! Хранится рядом с общими данными Soroka: %APPDATA%\Soroka\layout.json.
//! У Soroka то же самое лежит в localStorage «soroka.layout2» — файлы
//! не конфликтуют (кэш и аккаунты общие, компоновка — своя у каждой оболочки).
//!
//! TASK-N05: в тот же файл пишется тема оболочки (dark/light). Это наш
//! собственный файл, данные Soroka не затрагиваются; срез N09 перенесёт
//! тему в общий settings.json (порт ядра) — до тех пор живёт здесь.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize)]
struct Weights {
    folders: f64,
    list: f64,
    reading: f64,
}

#[derive(Serialize, Deserialize)]
struct LayoutFile {
    weights: Weights,
    /// «dark» (дефолт, как в Soroka) или «light». В старых файлах нет —
    /// serde подставит None, вызывающий берёт тёмную.
    #[serde(default)]
    theme: Option<String>,
}

fn file_path() -> Option<PathBuf> {
    let mut p = PathBuf::from(std::env::var("APPDATA").ok()?);
    p.push("Soroka");
    p.push("layout.json");
    Some(p)
}

/// Веса и тема из layout.json; None — файла нет или данные невалидны
/// (тогда вызывающий берёт дефолт из Soroka: веса LAYOUT_DEFAULTS, тёмная).
pub fn load() -> Option<(f64, f64, f64, bool)> {
    load_from(file_path()?.as_path())
}

pub fn save(wf: f64, wl: f64, wr: f64, dark: bool) {
    if let Some(p) = file_path() {
        save_to(&p, wf, wl, wr, dark);
    }
}

fn load_from(path: &Path) -> Option<(f64, f64, f64, bool)> {
    let s = std::fs::read_to_string(path).ok()?;
    let f: LayoutFile = serde_json::from_str(&s).ok()?;
    let w = f.weights;
    let ok = [w.folders, w.list, w.reading]
        .iter()
        .all(|v| v.is_finite() && *v > 0.0);
    ok.then_some((
        w.folders,
        w.list,
        w.reading,
        f.theme.as_deref() != Some("light"),
    ))
}

fn save_to(path: &Path, wf: f64, wl: f64, wr: f64, dark: bool) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let body = serde_json::to_string(&LayoutFile {
        weights: Weights {
            folders: wf,
            list: wl,
            reading: wr,
        },
        theme: Some(if dark { "dark" } else { "light" }.to_string()),
    });
    if let Ok(body) = body {
        let _ = std::fs::write(path, body);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("soroka-layout-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("layout.json")
    }

    #[test]
    fn weights_and_theme_roundtrip() {
        let p = tmp("roundtrip");
        assert_eq!(load_from(&p), None); // файла нет — дефолт за вызывающим
        save_to(&p, 0.28, 1.24, 2.0, false);
        assert_eq!(load_from(&p), Some((0.28, 1.24, 2.0, false)));
        save_to(&p, 0.28, 1.24, 2.0, true);
        assert_eq!(load_from(&p), Some((0.28, 1.24, 2.0, true)));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn file_without_theme_is_dark() {
        // Файл формата v0.3.2 (только веса) — читается, тема дефолтно тёмная.
        let p = tmp("old");
        std::fs::write(
            &p,
            r#"{"weights":{"folders":0.28,"list":1.24,"reading":2.0}}"#,
        )
        .unwrap();
        assert_eq!(load_from(&p), Some((0.28, 1.24, 2.0, true)));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn broken_file_is_none() {
        let p = tmp("broken");
        std::fs::write(&p, "не json").unwrap();
        assert_eq!(load_from(&p), None);
        // Невалидные веса (ноль/NaN) — тоже None, без паники.
        std::fs::write(&p, r#"{"weights":{"folders":0,"list":1,"reading":2}}"#).unwrap();
        assert_eq!(load_from(&p), None);
        let _ = std::fs::remove_file(&p);
    }
}
