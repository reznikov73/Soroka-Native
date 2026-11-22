//! Русские даты для списка и шапки письма — TASK-N05.
//!
//! Чистые функции без Win32: разбор дат RFC 5322 («Tue, 18 Nov 2026
//! 14:32:10 +0300», как их отдаёт ENVELOPE и хранит ядро в
//! `MessageHeader.date`) и григорианская математика по алгоритму
//! Говарда Хиннанта (days_from_civil / civil_from_days). Локальное
//! смещение зоны оболочка передаёт снаружи (GetLocalTime/GetSystemTime);
//! здесь только арифметика — поэтому всё проверяется тестами на любой
//! платформе.
//!
//! Форматы (как в Soroka):
//! - список: «сегодня, 14:32» / «вчера» / «12 ноября» / «12 ноября 2025»;
//! - шапка письма: «сегодня, 14:32» / «вчера, 09:10» / «18 ноября 2026, 14:32».

/// Названия месяцев в родительном падеже («12 ноября»).
const MONTHS_RU: [&str; 12] = [
    "января",
    "февраля",
    "марта",
    "апреля",
    "мая",
    "июня",
    "июля",
    "августа",
    "сентября",
    "октября",
    "ноября",
    "декабря",
];

/// Дней от 1970-01-01 до указанной даты (алгоритм Хиннанта, пролептический
/// григорианский календарь). Работает для любых лет в разумных пределах.
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let m = m as i64;
    let d = d as i64;
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = (m + 9) % 12; // [0, 11]
    let doy = (153 * mp + 2) / 5 + d - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146097 + doe - 719468
}

/// Обратное преобразование: (год, месяц, день) по числу дней от 1970-01-01.
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m as u32, d as u32)
}

/// Unix-время (UTC, секунды) из компонентов даты/времени.
pub fn from_ymdhms(y: i64, m: u32, d: u32, h: u32, mi: u32, s: u32) -> i64 {
    days_from_civil(y, m, d) * 86_400 + h as i64 * 3600 + mi as i64 * 60 + s as i64
}

/// Компоненты локальной даты/времени по Unix-времени и смещению зоны.
fn civil_local(ts: i64, off: i64) -> (i64, u32, u32, u32, u32) {
    let days = (ts + off).div_euclid(86_400);
    let secs = (ts + off).rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    (y, m, d, (secs / 3600) as u32, (secs % 3600 / 60) as u32)
}

/// Индекс месяца (1–12) по трём первым буквам английского названия.
fn month_index(tok: &str) -> Option<u32> {
    let t = tok.trim_start_matches('(').trim_end_matches(')');
    // Латиница и минимум три буквы: срез по байтам безопасен только
    // для ASCII (кириллица в «не дата» — двухбайтовые буквы).
    if !t.is_ascii() || t.len() < 3 {
        return None;
    }
    let t = t[..3].to_ascii_lowercase();
    Some(match t.as_str() {
        "jan" => 1,
        "feb" => 2,
        "mar" => 3,
        "apr" => 4,
        "may" => 5,
        "jun" => 6,
        "jul" => 7,
        "aug" => 8,
        "sep" => 9,
        "oct" => 10,
        "nov" => 11,
        "dec" => 12,
        _ => return None,
    })
}

/// Смещение зоны из токена: «+0300»/«-0500», «Z», «GMT», «MSK», «EST»…
/// Неизвестное имя трактуется как UTC (как -0000 в RFC 5322).
fn zone_offset(tok: &str) -> i64 {
    let b = tok.as_bytes();
    if b.len() == 5 && (b[0] == b'+' || b[0] == b'-') && b[1..].iter().all(u8::is_ascii_digit) {
        let hh = (b[1] - b'0') as i64 * 10 + (b[2] - b'0') as i64;
        let mm = (b[3] - b'0') as i64 * 10 + (b[4] - b'0') as i64;
        let v = hh * 3600 + mm * 60;
        return if b[0] == b'+' { v } else { -v };
    }
    match tok
        .trim_start_matches('(')
        .trim_end_matches(')')
        .to_ascii_uppercase()
        .as_str()
    {
        "UT" | "UTC" | "GMT" | "Z" => 0,
        "MSK" => 3 * 3600,
        "EST" => -5 * 3600,
        "EDT" => -4 * 3600,
        "CST" => -6 * 3600,
        "CDT" => -5 * 3600,
        "MST" => -7 * 3600,
        "MDT" => -6 * 3600,
        "PST" => -8 * 3600,
        "PDT" => -7 * 3600,
        _ => 0,
    }
}

/// Разбор даты письма (RFC 5322, практическое подмножество: [DoW,] DD Mon
/// YYYY HH:MM[:SS] ±ZZZZ; имена зон). Возвращает Unix-время UTC.
///
/// Ядро хранит `MessageHeader.date` как сырую строку заголовка — в тестах
/// ядра встречаются «Mon, 19 Oct 2026 10:00:00 +0300». Что не разобралось —
/// `None`, вызывающий покажет сырую строку.
pub fn parse_rfc5322(raw: &str) -> Option<i64> {
    let mut day: Option<u32> = None;
    let mut mon: Option<u32> = None;
    let mut year: Option<i64> = None;
    let (mut h, mut mi, mut s) = (None, None, 0u32);
    let mut off: i64 = 0;
    for tok in raw.split([' ', '\t', ',']) {
        let tok = tok.trim_matches(|c: char| c == '(' || c == ')');
        if tok.is_empty() {
            continue;
        }
        if day.is_none() && tok.len() <= 2 && tok.bytes().all(|b| b.is_ascii_digit()) {
            day = tok.parse().ok();
            continue;
        }
        if mon.is_none()
            && let Some(m) = month_index(tok)
        {
            mon = Some(m);
            continue;
        }
        if year.is_none() && tok.len() == 4 && tok.bytes().all(|b| b.is_ascii_digit()) {
            year = tok.parse().ok();
            continue;
        }
        if h.is_none()
            && let Some((hh, rest)) = tok.split_once(':')
        {
            let hh: u32 = hh.parse().ok()?;
            let (mm, ss) = match rest.split_once(':') {
                Some((mm, ss)) => (mm.parse().ok()?, ss.parse().ok()?),
                None => (rest.parse().ok()?, 0),
            };
            h = Some(hh);
            mi = Some(mm);
            s = ss;
            continue;
        }
        if tok.starts_with('+')
            || tok.starts_with('-')
            || tok.chars().all(|c| c.is_ascii_alphabetic())
        {
            off = zone_offset(tok);
        }
    }
    let (d, m, y) = (day?, mon?, year?);
    if !(1..=31).contains(&d) || !(1..=12).contains(&m) || !(1970..=2999).contains(&y) {
        return None;
    }
    let (h, mi) = (h?, mi?);
    if h > 23 || mi > 59 || s > 60 {
        return None;
    }
    Some(from_ymdhms(y, m, d, h, mi, s) - off)
}

/// Дата строки списка: «сегодня, 14:32» / «вчера» / «12 ноября» /
/// «12 ноября 2025» (другой год). `now` — Unix-время UTC, `off` — смещение
/// локальной зоны в секундах.
pub fn fmt_list(ts: i64, now: i64, off: i64) -> String {
    let (my, mmo, md, _, _) = civil_local(ts, off);
    let (ny, nmo, nd, _, _) = civil_local(now, off);
    let mday = days_from_civil(my, mmo, md);
    let nday = days_from_civil(ny, nmo, nd);
    if mday == nday {
        let (_, _, _, h, mi) = civil_local(ts, off);
        return format!("сегодня, {h:02}:{mi:02}");
    }
    if mday == nday - 1 {
        return "вчера".to_string();
    }
    let month = MONTHS_RU[(mmo - 1) as usize];
    if my == ny {
        format!("{md} {month}")
    } else {
        format!("{md} {month} {my}")
    }
}

/// Дата шапки письма: «сегодня, 14:32» / «вчера, 09:10» /
/// «18 ноября 2026, 14:32» (время — всегда).
pub fn fmt_full(ts: i64, now: i64, off: i64) -> String {
    let (my, mmo, md, _, _) = civil_local(ts, off);
    let (ny, nmo, nd, _, _) = civil_local(now, off);
    let mday = days_from_civil(my, mmo, md);
    let nday = days_from_civil(ny, nmo, nd);
    let (_, _, _, h, mi) = civil_local(ts, off);
    if mday == nday {
        return format!("сегодня, {h:02}:{mi:02}");
    }
    if mday == nday - 1 {
        return format!("вчера, {h:02}:{mi:02}");
    }
    let month = MONTHS_RU[(mmo - 1) as usize];
    format!("{md} {month} {my}, {h:02}:{mi:02}")
}

/// Название месяца в именительном падеже («Ноябрь») — для заголовка календаря
/// (в форматировании дат используется родительный: «12 ноября»).
pub fn month_nom_ru(m: u32) -> &'static str {
    const NAMES: [&str; 12] = [
        "Январь",
        "Февраль",
        "Март",
        "Апрель",
        "Май",
        "Июнь",
        "Июль",
        "Август",
        "Сентябрь",
        "Октябрь",
        "Ноябрь",
        "Декабрь",
    ];
    NAMES[(m.clamp(1, 12) - 1) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-11-18 00:00 UTC (20 775 дней от эпохи).
    const NOW: i64 = 1_794_960_000;

    fn days(y: i64, m: u32, d: u32) -> i64 {
        days_from_civil(y, m, d)
    }

    #[test]
    fn epoch_and_known_days() {
        assert_eq!(days(1970, 1, 1), 0);
        assert_eq!(days(1970, 1, 2), 1);
        assert_eq!(days(1969, 12, 31), -1);
        assert_eq!(days(2026, 11, 18), NOW / 86_400);
        // Високосность: 2024-02-29 существует, 2025-02-29 сворачивается в 03-01.
        assert_eq!(days(2024, 3, 1) - days(2024, 2, 28), 2);
        assert_eq!(days(2025, 3, 1) - days(2025, 2, 28), 1);
        // 2100 — не високосный (делится на 100, но не на 400).
        assert_eq!(days(2100, 3, 1) - days(2100, 2, 28), 1);
        assert_eq!(days(2000, 3, 1) - days(2000, 2, 28), 2);
    }

    #[test]
    fn civil_roundtrip() {
        for z in [-100_000i64, -1, 0, 1, 59, 719468, 100_000, 60_000] {
            let (y, m, d) = civil_from_days(z);
            assert_eq!(days(y, m, d), z, "roundtrip для {z}");
        }
    }

    #[test]
    fn parse_full_form() {
        // Из тестов ядра (imap.rs): «Mon, 19 Oct 2026 10:00:00 +0300».
        let ts = parse_rfc5322("Mon, 19 Oct 2026 10:00:00 +0300").unwrap();
        assert_eq!(ts, from_ymdhms(2026, 10, 19, 10, 0, 0) - 3 * 3600);
    }

    #[test]
    fn parse_variants() {
        let full = parse_rfc5322("Tue, 18 Nov 2026 14:32:10 +0300").unwrap();
        // без дня недели
        assert_eq!(parse_rfc5322("18 Nov 2026 14:32:10 +0300"), Some(full));
        // без секунд
        assert_eq!(parse_rfc5322("18 Nov 2026 14:32 +0300"), Some(full - 10));
        // именованная зона: GMT = UTC, это на 3 часа ПОЗЖЕ по UTC, чем +0300
        assert_eq!(
            parse_rfc5322("Wed, 18 Nov 2026 14:32:10 GMT"),
            Some(full + 3 * 3600)
        );
        // минусовая зона: -0500 — на 8 часов позже по UTC, чем +0300
        assert_eq!(
            parse_rfc5322("18 Nov 2026 14:32:10 -0500"),
            Some(full + 8 * 3600)
        );
        // регистр месяца
        assert_eq!(parse_rfc5322("18 nov 2026 14:32:10 +0300"), Some(full));
    }

    #[test]
    fn parse_garbage_is_none() {
        assert_eq!(parse_rfc5322(""), None);
        assert_eq!(parse_rfc5322("не дата"), None);
        assert_eq!(parse_rfc5322("32 Xxx 2026 25:99:99 +0300"), None);
        // ISO-дата не разбирается — вызывающий покажет сырую строку.
        assert_eq!(parse_rfc5322("2026-11-18T14:32:10+03:00"), None);
    }

    #[test]
    fn list_relative_days() {
        // Смещение +3ч (Москва), «сегодня» в UTC-полдень.
        let off = 3 * 3600;
        let today_noon = from_ymdhms(2026, 11, 18, 12, 0, 0);
        assert_eq!(fmt_list(today_noon, NOW, off), "сегодня, 15:00");
        let yesterday = from_ymdhms(2026, 11, 17, 9, 5, 0);
        assert_eq!(fmt_list(yesterday, NOW, off), "вчера");
        // Граница суток по местному времени: 23:59 мск вчерашнего дня —
        // «вчера»; 00:00 мск сегодняшнего — уже «сегодня».
        let late = from_ymdhms(2026, 11, 17, 20, 59, 0);
        assert_eq!(fmt_list(late, NOW, off), "вчера");
        let midnight = from_ymdhms(2026, 11, 17, 21, 0, 0);
        assert_eq!(fmt_list(midnight, NOW, off), "сегодня, 00:00");
    }

    #[test]
    fn list_absolute_dates() {
        let off = 0;
        let this_year = from_ymdhms(2026, 5, 12, 10, 0, 0);
        assert_eq!(fmt_list(this_year, NOW, off), "12 мая");
        let other_year = from_ymdhms(2025, 11, 12, 10, 0, 0);
        assert_eq!(fmt_list(other_year, NOW, off), "12 ноября 2025");
    }

    #[test]
    fn full_keeps_time() {
        let off = 3 * 3600;
        let today = from_ymdhms(2026, 11, 18, 12, 30, 0);
        assert_eq!(fmt_full(today, NOW, off), "сегодня, 15:30");
        let yest = from_ymdhms(2026, 11, 17, 6, 7, 0);
        assert_eq!(fmt_full(yest, NOW, off), "вчера, 09:07");
        let old = from_ymdhms(2025, 1, 9, 14, 0, 0);
        assert_eq!(fmt_full(old, NOW, off), "9 января 2025, 17:00");
    }
}
