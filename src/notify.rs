//! Тексты трея и уведомлений (TASK-N07): чистые функции без Win32 —
//! тестируются на Linux в логик-прогоне.
//!
//! Подпись трея и «первое письмо» — порт set_tray_unread / notify_new_mail
//! из Soroka (TASK-037/079/087/099). Отличие: заголовок уведомления
//! склоняется по-русски правильно («2 новых письма», «5 новых писем») —
//! в Soroka для любого числа писалось «новых письма».

/// «1 непрочитанное письмо», «2 непрочитанных письма», «5 непрочитанных
/// писем» — дословный порт plural_pisem из Soroka.
pub fn plural_pisem(n: u32) -> &'static str {
    let d10 = n % 10;
    let d100 = n % 100;
    if d10 == 1 && d100 != 11 {
        "непрочитанное письмо"
    } else if (2..=4).contains(&d10) && !(12..=14).contains(&d100) {
        "непрочитанных письма"
    } else {
        "непрочитанных писем"
    }
}

/// Подпись иконки трея: «POS Mail» или «POS Mail — N непрочитанных …»
/// (порт tooltip из set_tray_unread).
pub fn tooltip(unread: u32) -> String {
    if unread == 0 {
        "POS Mail".to_string()
    } else {
        format!("POS Mail — {unread} {}", plural_pisem(unread))
    }
}

/// Заголовок уведомления о новых письмах.
pub fn new_mail_title(n: u32) -> String {
    let d10 = n % 10;
    let d100 = n % 100;
    if d10 == 1 && d100 != 11 {
        if n == 1 {
            "Новое письмо".to_string()
        } else {
            format!("{n} новое письмо")
        }
    } else if (2..=4).contains(&d10) && !(12..=14).contains(&d100) {
        format!("{n} новых письма")
    } else {
        format!("{n} новых писем")
    }
}

/// Тело уведомления: «От кого: Тема», без данных — подсказка открыть
/// почту (порт body из notify_new_mail).
pub fn new_mail_body(first: Option<(&str, &str)>) -> String {
    match first {
        Some((from, subject)) => {
            let from = from.trim();
            let subject = subject.trim();
            if from.is_empty() && subject.is_empty() {
                "Проверьте почту в POS Mail".to_string()
            } else if from.is_empty() {
                subject.to_string()
            } else if subject.is_empty() {
                from.to_string()
            } else {
                format!("{from}: {subject}")
            }
        }
        None => "Проверьте почту в POS Mail".to_string(),
    }
}

/// Число писем, которых не было в прошлом снимке кэша (по UID).
pub fn new_count(known: &[u32], fresh: &[u32]) -> u32 {
    fresh.iter().filter(|uid| !known.contains(uid)).count() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plural_like_soroka() {
        assert_eq!(plural_pisem(1), "непрочитанное письмо");
        assert_eq!(plural_pisem(2), "непрочитанных письма");
        assert_eq!(plural_pisem(4), "непрочитанных письма");
        assert_eq!(plural_pisem(5), "непрочитанных писем");
        assert_eq!(plural_pisem(11), "непрочитанных писем");
        assert_eq!(plural_pisem(21), "непрочитанное письмо");
        assert_eq!(plural_pisem(22), "непрочитанных письма");
        assert_eq!(plural_pisem(112), "непрочитанных писем");
    }

    #[test]
    fn tooltip_states() {
        assert_eq!(tooltip(0), "POS Mail");
        assert_eq!(tooltip(1), "POS Mail — 1 непрочитанное письмо");
        assert_eq!(tooltip(3), "POS Mail — 3 непрочитанных письма");
        assert_eq!(tooltip(7), "POS Mail — 7 непрочитанных писем");
    }

    #[test]
    fn titles_decline() {
        assert_eq!(new_mail_title(1), "Новое письмо");
        assert_eq!(new_mail_title(2), "2 новых письма");
        assert_eq!(new_mail_title(5), "5 новых писем");
        assert_eq!(new_mail_title(11), "11 новых писем");
        assert_eq!(new_mail_title(21), "21 новое письмо");
    }

    #[test]
    fn body_variants() {
        assert_eq!(new_mail_body(Some(("Иванов", "Отчёт"))), "Иванов: Отчёт");
        assert_eq!(new_mail_body(Some(("", "Отчёт"))), "Отчёт");
        assert_eq!(new_mail_body(Some(("Иванов", ""))), "Иванов");
        assert_eq!(
            new_mail_body(Some(("  ", "  "))),
            "Проверьте почту в POS Mail"
        );
        assert_eq!(new_mail_body(None), "Проверьте почту в POS Mail");
    }

    #[test]
    fn counts_only_unseen_before() {
        assert_eq!(new_count(&[], &[1, 2, 3]), 3);
        assert_eq!(new_count(&[1, 2], &[1, 2, 3]), 1);
        assert_eq!(new_count(&[1, 2, 3], &[1, 2]), 0);
        assert_eq!(new_count(&[9], &[8]), 1); // письмо удалено, другое пришло
        assert_eq!(new_count(&[1, 2, 3], &[]), 0);
    }
}
