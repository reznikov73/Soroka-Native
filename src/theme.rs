//! Палитры Soroka (тёмная/светлая) для оболочки — TASK-N05.
//!
//! Тёмная снята с переменных `:root` из `src/styles.css` Soroka (дословно:
//! accent `#8b83ff`, bg-app `rgba(28,30,36,…)`, text `rgba(255,255,255,.92)`
//! и т.д.; полупрозрачные значения сведены к сплошным поверх фона —
//! GDI альфы не умеет). Светлой темы в Soroka нет (`color-scheme: dark`
//! без альтернатив) — она собрана в той же структуре для N05; когда светлая
//! тема появится в Soroka, значения заменяются на оригинальные.
//!
//! Дефолт — тёмная, как в Soroka. Хранение — `layout.json` (свой файл,
//! с данными Soroka не пересекается); срез N09 перенесёт тему в общий
//! settings.json.

use windows::Win32::Foundation::COLORREF;

/// RGB → COLORREF (Win32 хранит цвета как 0x00BBGGRR).
const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF(((b as u32) << 16) | ((g as u32) << 8) | (r as u32))
}

/// Одна тема: GDI-цвета оболочки + hex-цвета шаблона панели письма.
pub struct Palette {
    /// Фон панелей (дерево, список, статус, сплиттеры) — `--bg-app`.
    pub bg: COLORREF,
    /// Поверхность поля поиска — `--bg-card`.
    pub surface: COLORREF,
    /// Основной текст — `--text`.
    pub text: COLORREF,
    /// Вторичный текст: даты, мета — `--text2`.
    pub text_dim: COLORREF,
    /// Границы, линия сплиттера — `--stroke`.
    pub stroke: COLORREF,
    /// Акцент: хват сплиттера при наведении/жесте, рамка поиска в фокусе — `--accent`.
    pub accent: COLORREF,
    /// Фон выделенной строки списка — `--accent-weak-2` поверх bg-app
    /// (TASK-N10; вместо системного синего).
    pub selected: COLORREF,

    // HTML-цвета панели письма (WebView2): значения из той же палитры.
    pub pane_bg: &'static str,
    pub pane_hdr: &'static str,
    pub pane_text: &'static str,
    pub pane_meta: &'static str,
    pub pane_border: &'static str,
    pub pane_link: &'static str,
    pub pane_warn: &'static str,
}

/// Тёмная тема — палитра Soroka (styles.css, `:root`).
pub const DARK: Palette = Palette {
    bg: rgb(0x1C, 0x1E, 0x24),       // bg-app 28,30,36
    surface: rgb(0x2A, 0x2C, 0x33),  // bg-card 44,46,54 поверх bg-app
    text: rgb(0xED, 0xED, 0xEE),     // text: белый .92 поверх bg-app
    text_dim: rgb(0xA9, 0xAA, 0xAC), // text2: белый .62 поверх bg-app
    stroke: rgb(0x32, 0x33, 0x39),   // stroke: белый .095 поверх bg-app
    accent: rgb(0x8B, 0x83, 0xFF),   // --accent
    // accent-weak-2 rgba(139,131,255,.24) поверх bg-app.
    selected: rgb(0x37, 0x36, 0x59),

    pane_bg: "#1c1e24",
    pane_hdr: "#24262c", // --panel: белый .035 поверх bg-app
    pane_text: "#ededee",
    pane_meta: "#a9aaac",
    pane_border: "#323339",
    pane_link: "#8b83ff",
    pane_warn: "#d0a95e", // янтарь шипа Фазы 0: предупреждение о обрезке
};

/// Светлая тема — комплементарная (в Soroka отсутствует).
pub const LIGHT: Palette = Palette {
    bg: rgb(0xF6, 0xF7, 0xF9),
    surface: rgb(0xFF, 0xFF, 0xFF),
    text: rgb(0x1F, 0x24, 0x30),
    text_dim: rgb(0x5F, 0x66, 0x72),
    stroke: rgb(0xDF, 0xE3, 0xEA),
    accent: rgb(0x5B, 0x50, 0xD9), // акцент Soroka, затемнён для белого фона
    // Светлая секция styles.css: accent-weak-2 rgba(79,70,229,.14)
    // поверх нашего фона.
    selected: rgb(0xDF, 0xDE, 0xF6),

    pane_bg: "#ffffff",
    pane_hdr: "#f6f7f9",
    pane_text: "#1f2430",
    pane_meta: "#5f6672",
    pane_border: "#e3e7ee",
    pane_link: "#5b50d9",
    pane_warn: "#92400e",
};

/// Палитра по флагу темы.
pub fn palette(dark: bool) -> &'static Palette {
    if dark { &DARK } else { &LIGHT }
}
