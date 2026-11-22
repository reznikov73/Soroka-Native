//! Адресные подсказки в окне написания (TASK-N12): чистые функции.
//!
//! Позиции и длины считаются в единицах UTF-16: именно их отдают `EM_GETSEL`
//! и ждёт `EM_SETSEL`, а строка Rust — UTF-8 (кириллица занимает 2 байта на
//! символ, поэтому байтовые смещения здесь недопустимы).
//!
//! Модуль не знает про Win32 — всё покрыто юнит-тестами без окна.

use crate::mail::contacts::AddressSuggestion;

/// Символы-разделители получателей: `,` и `;` (ядро при отправке режет по ним).
const COMMA: u16 = b',' as u16;
const SEMI: u16 = b';' as u16;
const SPACE: u16 = b' ' as u16;

/// Текущий токен получателя под кареткой: `(начало, конец, текст)` в UTF-16.
/// Крайние пробелы в длину токена не входят — запрос для фильтра чистый.
pub fn token_at(text: &[u16], caret: usize) -> (usize, usize, String) {
    let caret = caret.min(text.len());
    let mut start = caret;
    while start > 0 && !matches!(text[start - 1], COMMA | SEMI) {
        start -= 1;
    }
    let mut end = caret;
    while end < text.len() && !matches!(text[end], COMMA | SEMI) {
        end += 1;
    }
    let mut s = start;
    while s < end && text[s] == SPACE {
        s += 1;
    }
    let mut e = end;
    while e > s && text[e - 1] == SPACE {
        e -= 1;
    }
    (s, e, String::from_utf16_lossy(&text[s..e]))
}

/// Подсказки под токен: совпадение с начала адреса/имени — выше, затем по
/// частоте переписки, затем по адресу (детерминированный порядок).
/// Пустой токен — самые частые (окно только открыли, ещё ничего не набрали).
pub fn rank<'a>(
    list: &'a [AddressSuggestion],
    token: &str,
    limit: usize,
) -> Vec<&'a AddressSuggestion> {
    let needle = token.trim().to_lowercase();
    let mut scored: Vec<(u8, u32, &AddressSuggestion)> = list
        .iter()
        .filter_map(|s| {
            let email = s.email.to_lowercase();
            let name = s.name.to_lowercase();
            if needle.is_empty() {
                return Some((3, s.count, s));
            }
            let score = if email.starts_with(&needle) {
                0
            } else if !name.is_empty() && name.starts_with(&needle) {
                1
            } else if email.contains(&needle) || name.contains(&needle) {
                2
            } else {
                return None;
            };
            Some((score, s.count, s))
        })
        .collect();
    scored.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| b.1.cmp(&a.1))
            .then_with(|| a.2.email.cmp(&b.2.email))
    });
    scored.into_iter().take(limit).map(|(_, _, s)| s).collect()
}

/// Текст, который вставляется в поле получателей: `Имя <адрес>, ` или `адрес, `.
/// Запятые/угловые скобки из имени убираются — иначе разбор получателей
/// в ядре (`smtp::parse`) разрежет адрес на части.
pub fn recipient_text(s: &AddressSuggestion) -> String {
    let email = s.email.trim();
    let name: String = s
        .name
        .chars()
        .map(|c| if matches!(c, ',' | ';' | '<' | '>') { ' ' } else { c })
        .collect();
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() || name.eq_ignore_ascii_case(email) {
        format!("{email}, ")
    } else {
        format!("{name} <{email}>, ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    fn s(email: &str, name: &str, count: u32) -> AddressSuggestion {
        AddressSuggestion {
            email: email.to_string(),
            name: name.to_string(),
            source: "contact".to_string(),
            count,
        }
    }

    #[test]
    fn token_found_under_caret_and_trimmed() {
        let text = u("ivan@mail.ru, маша");
        let (start, end, token) = token_at(&text, text.len());
        assert_eq!(token, "маша");
        assert_eq!(&text[start..end], u("маша").as_slice());

        // Каретка в середине первого адреса.
        let (_, _, token) = token_at(&text, 5);
        assert_eq!(token, "ivan@mail.ru");
    }

    #[test]
    fn token_is_empty_after_separator() {
        let text = u("ivan@mail.ru, ");
        let (start, end, token) = token_at(&text, text.len());
        assert!(token.is_empty());
        assert_eq!(start, end);
    }

    #[test]
    fn rank_prefers_prefix_then_frequency() {
        let list = vec![
            s("ivan@mail.ru", "Иван", 1),
            s("petr@mail.ru", "Пётр", 9),
            s("pavel@corp.ru", "Павел Иванов", 2),
        ];
        // Префикс адреса важнее частоты; совпадение по имени — ниже адреса.
        let top = rank(&list, "p", 3);
        assert_eq!(top[0].email, "petr@mail.ru");
        assert_eq!(top[1].email, "pavel@corp.ru");
        // «Иван» — начало имени у первого и часть имени у третьего:
        // совпадение с начала имени идёт выше совпадения внутри.
        let by_name = rank(&list, "иван", 3);
        assert_eq!(by_name.len(), 2);
        assert_eq!(by_name[0].email, "ivan@mail.ru");
        assert_eq!(by_name[1].email, "pavel@corp.ru");
        // Ничего не нашли.
        assert!(rank(&list, "zzz", 3).is_empty());
        // Пустой токен — по частоте.
        assert_eq!(rank(&list, "", 2)[0].email, "petr@mail.ru");
    }

    #[test]
    fn recipient_text_forms_and_sanitizes() {
        assert_eq!(
            recipient_text(&s("ivan@mail.ru", "Иван Петров", 0)),
            "Иван Петров <ivan@mail.ru>, "
        );
        assert_eq!(recipient_text(&s("buh@corp.ru", "", 0)), "buh@corp.ru, ");
        // Имя с запятой не должно ломать разбор получателей.
        assert_eq!(
            recipient_text(&s("x@y.ru", "Иванов, Иван", 0)),
            "Иванов Иван <x@y.ru>, "
        );
        // Имя совпадает с адресом — не дублируем.
        assert_eq!(
            recipient_text(&s("x@y.ru", "x@y.ru", 0)),
            "x@y.ru, "
        );
    }

    #[test]
    fn token_splits_on_semicolon_and_keeps_adjacent() {
        // Точка с запятой — тоже разделитель; соседние адреса не трогаем.
        let text = u("a@b.ru; ма");
        let (start, end, token) = token_at(&text, text.len());
        assert_eq!(token, "ма");
        assert_eq!(start, text.len() - 2);
        assert_eq!(end, text.len());
    }
}
