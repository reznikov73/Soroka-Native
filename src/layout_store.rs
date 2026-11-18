//! Сохранение компоновки (TASK-N04/N05/N06): веса панелей, тема и
//! геометрия окон — один файл %APPDATA%\Soroka\layout.json.
//! У Soroka то же самое лежит в localStorage «soroka.layout2» — файлы
//! не конфликтуют (кэш и аккаунты общие, компоновка — своя у каждой
//! оболочки). Срез N09 перенесёт тему в общий settings.json (порт ядра);
//! веса и геометрия остаются здесь — это состояние оболочки.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Веса панелей по умолчанию — LAYOUT_DEFAULTS из Soroka (окт 2026,
/// согласовано с пользователем: узкие папки, широкое письмо). Живут тут:
/// layout.json — их дом, ui.rs переиспользует.
pub const WF_DEF: f64 = 0.2827;
pub const WL_DEF: f64 = 1.2443;
pub const WR_DEF: f64 = 2.023;

/// Геометрия окна (TASK-N06): нормальная позиция/размер (когда окно не
/// развёрнуто) и флаг «было развёрнуто».
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WinPlacement {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    #[serde(default)]
    pub maximized: bool,
}

/// Какое окно кладём в layout.json.
pub enum WinKind {
    Main,
    Compose,
    Account,
}

#[derive(Serialize, Deserialize)]
struct Weights {
    folders: f64,
    list: f64,
    reading: f64,
}

impl Default for Weights {
    fn default() -> Self {
        Self {
            folders: WF_DEF,
            list: WL_DEF,
            reading: WR_DEF,
        }
    }
}

#[derive(Serialize, Deserialize, Default)]
struct LayoutFile {
    #[serde(default)]
    weights: Weights,
    /// «dark» (дефолт, как в Soroka) или «light» — TASK-N05.
    #[serde(default)]
    theme: Option<String>,
    /// Геометрия окон — TASK-N06 (в старых файлах нет — serde даст None).
    #[serde(default)]
    main: Option<WinPlacement>,
    #[serde(default)]
    compose: Option<WinPlacement>,
    #[serde(default)]
    account: Option<WinPlacement>,
}

/// Загруженная компоновка.
#[derive(Debug, PartialEq)]
pub struct Layout {
    pub wf: f64,
    pub wl: f64,
    pub wr: f64,
    pub dark: bool,
    pub main: Option<WinPlacement>,
    pub compose: Option<WinPlacement>,
    pub account: Option<WinPlacement>,
}

fn file_path() -> Option<PathBuf> {
    let mut p = PathBuf::from(std::env::var("APPDATA").ok()?);
    p.push("Soroka");
    p.push("layout.json");
    Some(p)
}

/// Компоновка из layout.json; None — файла нет или он битый (then
/// вызывающий берёт дефолты: веса LAYOUT_DEFAULTS, тёмная тема).
pub fn load() -> Option<Layout> {
    load_from(file_path()?.as_path())
}

/// Сохранить веса и тему; геометрия окон в файле сохраняется
/// (read-modify-write: она пишется отдельно, при закрытии окон).
pub fn save(wf: f64, wl: f64, wr: f64, dark: bool) {
    if let Some(p) = file_path() {
        save_at(&p, wf, wl, wr, dark);
    }
}

/// Записать геометрию окна (TASK-N06): вызывается при закрытии окна;
/// веса/тема/геометрия остальных окон в файле не трогаются.
pub fn store_window(kind: WinKind, wp: WinPlacement) {
    if let Some(p) = file_path() {
        store_window_at(&p, kind, wp);
    }
}

fn save_at(p: &Path, wf: f64, wl: f64, wr: f64, dark: bool) {
    let mut f = read_or_default(p);
    f.weights = Weights {
        folders: wf,
        list: wl,
        reading: wr,
    };
    f.theme = Some(if dark { "dark" } else { "light" }.to_string());
    write(p, &f);
}

fn store_window_at(p: &Path, kind: WinKind, wp: WinPlacement) {
    let mut f = read_or_default(p);
    match kind {
        WinKind::Main => f.main = Some(wp),
        WinKind::Compose => f.compose = Some(wp),
        WinKind::Account => f.account = Some(wp),
    }
    write(p, &f);
}

fn read_or_default(p: &Path) -> LayoutFile {
    std::fs::read_to_string(p)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn write(p: &Path, f: &LayoutFile) {
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(body) = serde_json::to_string(f) {
        let _ = std::fs::write(p, body);
    }
}

fn load_from(path: &Path) -> Option<Layout> {
    let f: LayoutFile = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    // Невалидные веса (ноль/NaN — файл правили руками) молча заменяем
    // дефолтами: тема и геометрия при этом не теряются.
    let ok = |v: f64| v.is_finite() && v > 0.0;
    let wf = if ok(f.weights.folders) {
        f.weights.folders
    } else {
        WF_DEF
    };
    let wl = if ok(f.weights.list) {
        f.weights.list
    } else {
        WL_DEF
    };
    let wr = if ok(f.weights.reading) {
        f.weights.reading
    } else {
        WR_DEF
    };
    Some(Layout {
        wf,
        wl,
        wr,
        dark: f.theme.as_deref() != Some("light"),
        main: f.main,
        compose: f.compose,
        account: f.account,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("soroka-layout6-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("layout.json")
    }

    #[test]
    fn full_roundtrip() {
        let p = tmp("roundtrip");
        assert_eq!(load_from(&p), None); // файла нет — дефолт за вызывающим
        let wp = WinPlacement {
            x: 10,
            y: 20,
            w: 800,
            h: 600,
            maximized: true,
        };
        store_window_at(&p, WinKind::Main, wp);
        let l = load_from(&p).unwrap();
        assert_eq!(l.main, Some(wp));
        assert!((l.wf - WF_DEF).abs() < 1e-9); // веса не заданы — дефолт
        assert!(l.dark); // тема не задана — тёмная
        save_at(&p, 0.5, 1.0, 2.0, false);
        let l = load_from(&p).unwrap();
        assert!((l.wf - 0.5).abs() < 1e-9);
        assert!(!l.dark);
        assert_eq!(l.main, Some(wp)); // save() не теряет геометрию
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn store_window_keeps_other_windows() {
        let p = tmp("keep");
        let compose = WinPlacement {
            x: 1,
            y: 2,
            w: 780,
            h: 620,
            maximized: false,
        };
        store_window_at(&p, WinKind::Compose, compose);
        save_at(&p, 0.28, 1.24, 2.0, true);
        let main = WinPlacement {
            x: 5,
            y: 6,
            w: 1440,
            h: 900,
            maximized: false,
        };
        store_window_at(&p, WinKind::Main, main);
        let l = load_from(&p).unwrap();
        assert_eq!(l.main, Some(main));
        assert_eq!(l.compose, Some(compose));
        assert_eq!(l.account, None);
        assert!(l.dark); // тема пережила store_window
        assert!((l.wf - 0.28).abs() < 1e-9); // и веса тоже
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
        let l = load_from(&p).unwrap();
        assert!(l.dark);
        assert!((l.wf - 0.28).abs() < 1e-9);
        assert_eq!(l.main, None);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn broken_weights_fall_back_but_windows_survive() {
        let p = tmp("broken-weights");
        std::fs::write(
            &p,
            r#"{"weights":{"folders":0,"list":1.24,"reading":2.0},"main":{"x":1,"y":2,"w":3,"h":4}}"#,
        )
        .unwrap();
        let l = load_from(&p).unwrap();
        assert!((l.wf - WF_DEF).abs() < 1e-9); // нулевой вес — дефолт
        assert_eq!(l.main.map(|m| m.x), Some(1)); // окно не потерялось
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn broken_file_is_none() {
        let p = tmp("broken");
        std::fs::write(&p, "не json").unwrap();
        assert!(load_from(&p).is_none());
        let _ = std::fs::remove_file(&p);
    }
}
