//! Черновики письма (TASK-N08): чистые функции без Win32 — тестируются
//! на Linux в логик-прогоне.
//!
//! Порт механики TASK-022 Soroka: идентификатор черновика, отпечаток
//! изменений (автосохранение не пересохраняет неотредактированное),
//! служебный заголовок X-Soroka-Draft (UID на серверах без UIDPLUS ищется
//! по нему). Дополнительно к Сороке — JSON копии (cc) и вложений в колонке
//! attachments записи drafts: у Сороки этих полей нет (пишет None),
//! запись остаётся совместимой — Сорока читает её как свою.

use base64::Engine as _;
use serde::{Deserialize, Serialize};

/// Идентификатор черновика (порт newDraftId: время + случайность).
/// Заголовок X-Soroka-Draft — токен без пробелов, годится hex.
pub fn new_draft_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    // RandomState сеется на процесс; его хеш — псевдослучайное число
    // без внешнего rand-крейта (как Math.random в фолбэке Сороки).
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(millis);
    format!("sn-draft-{millis:016x}-{:016x}", h.finish())
}

/// Пустой ли черновик: все поля пусты и вложений нет — сохранять нечего.
pub fn is_empty(to: &str, cc: &str, subject: &str, body: &str, att_count: usize) -> bool {
    to.trim().is_empty()
        && cc.trim().is_empty()
        && subject.trim().is_empty()
        && body.trim().is_empty()
        && att_count == 0
}

/// Отпечаток содержимого (порт lastDraftPayload): не изменилось — не
/// пересохраняем, каждое сохранение это IMAP APPEND + удаление старой
/// копии. У Сороки в отпечаток входят имена вложений; мы добавляем размер —
/// замена файла с тем же именем тоже ловится.
pub fn fingerprint(to: &str, cc: &str, subject: &str, body: &str, atts: &[(&str, u64)]) -> String {
    let att: Vec<String> = atts.iter().map(|a| format!("{}:{}", a.0, a.1)).collect();
    serde_json::to_string(&(to, cc, subject, body, att)).unwrap_or_default()
}

/// Вставляет служебный заголовок черновика в готовое MIME-письмо (порт
/// insert_draft_header): перед первым пустой строкой, чтобы остаться
/// в шапке письма.
pub fn insert_draft_header(raw: &[u8], id: &str) -> Vec<u8> {
    let sep = b"\r\n\r\n";
    if let Some(pos) = raw.windows(4).position(|w| w == sep) {
        let mut out = Vec::with_capacity(raw.len() + 40);
        out.extend_from_slice(&raw[..pos]);
        out.extend_from_slice(format!("\r\nX-Soroka-Draft: {id}").as_bytes());
        out.extend_from_slice(&raw[pos..]);
        out
    } else {
        raw.to_vec()
    }
}

/// Вложение черновика (имя, MIME, данные в base64 — как QueuedFile очереди).
#[derive(Serialize, Deserialize)]
pub struct DraftFile {
    pub name: String,
    pub mime: String,
    pub data_b64: String,
}

/// Дополнительные данные черновика, которых нет в DraftRecord Сороки:
/// копия (cc) и вложения. Хранятся JSON-строкой в колонке attachments.
#[derive(Serialize, Deserialize, Default)]
pub struct DraftExtra {
    #[serde(default)]
    pub cc: String,
    #[serde(default)]
    pub atts: Vec<DraftFile>,
}

/// Кодирует cc и вложения в колонку attachments записи drafts.
pub fn encode_extra(cc: &str, atts: &[(String, String, Vec<u8>)]) -> Option<String> {
    let extra = DraftExtra {
        cc: cc.to_string(),
        atts: atts
            .iter()
            .map(|(name, mime, data)| DraftFile {
                name: name.clone(),
                mime: mime.clone(),
                data_b64: base64::engine::general_purpose::STANDARD.encode(data),
            })
            .collect(),
    };
    serde_json::to_string(&extra).ok()
}

/// Разбирает колонку attachments: нет файла/битый JSON — черновик Сороки
/// (без копии и вложений), битый base64 у отдельного файла — файл
/// пропускается, остальное сохраняется.
pub fn decode_extra(json: Option<&str>) -> (String, Vec<(String, String, Vec<u8>)>) {
    let extra: DraftExtra = json
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    let atts = extra
        .atts
        .into_iter()
        .filter_map(|f| {
            base64::engine::general_purpose::STANDARD
                .decode(f.data_b64.as_bytes())
                .ok()
                .map(|data| (f.name, f.mime, data))
        })
        .collect();
    (extra.cc, atts)
}

/// Подпись статуса после сохранения (порт composeSaved: «✓ сохранено ЧЧ:ММ»).
pub fn saved_label(hh: u32, mm: u32) -> String {
    format!("✓ сохранено {hh:02}:{mm:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_unique_and_token_safe() {
        let a = new_draft_id();
        let b = new_draft_id();
        assert_ne!(a, b);
        for id in [&a, &b] {
            assert!(!id.is_empty());
            assert!(
                id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-'),
                "в заголовке допустимы только символы токена: {id}"
            );
        }
        assert!(a.starts_with("sn-draft-"));
    }

    #[test]
    fn emptiness() {
        assert!(is_empty("", " ", "", "\r\n", 0));
        assert!(!is_empty("a@b.ru", "", "", "", 0));
        assert!(!is_empty("", "", "тема", "", 0));
        assert!(!is_empty("", "", "", "строка", 0));
        // Только вложения — уже не пусто.
        assert!(!is_empty("", "", "", "", 1));
    }

    #[test]
    fn fingerprint_stable_and_sensitive() {
        let atts = [("отчёт.pdf", 1024u64)];
        let a = fingerprint("a@b.ru", "c@d.ru", "тема", "текст", &atts);
        assert_eq!(a, fingerprint("a@b.ru", "c@d.ru", "тема", "текст", &atts));
        // Каждое поле и каждое вложение участвуют.
        assert_ne!(a, fingerprint("x@y.ru", "c@d.ru", "тема", "текст", &atts));
        assert_ne!(a, fingerprint("a@b.ru", "", "тема", "текст", &atts));
        assert_ne!(a, fingerprint("a@b.ru", "c@d.ru", "Тема", "текст", &atts));
        assert_ne!(a, fingerprint("a@b.ru", "c@d.ru", "тема", "другой", &atts));
        // Имя тоже в отпечатке: другой файл того же размера — изменение.
        assert_ne!(
            a,
            fingerprint("a@b.ru", "c@d.ru", "тема", "текст", &[("z", 1024)])
        );
        // То же имя и размер — отпечаток тот же (контент не хешируем).
        assert_eq!(
            a,
            fingerprint("a@b.ru", "c@d.ru", "тема", "текст", &[("отчёт.pdf", 1024)])
        );
        // Другой размер при том же имени — изменилось.
        assert_ne!(
            a,
            fingerprint("a@b.ru", "c@d.ru", "тема", "текст", &[("отчёт.pdf", 2048)])
        );
    }

    #[test]
    fn draft_header_inserted_into_head() {
        let raw = "From: a@b.ru\r\nTo: c@d.ru\r\nSubject: s\r\n\r\nтело";
        let out = insert_draft_header(raw.as_bytes(), "sn-1");
        let expected =
            "From: a@b.ru\r\nTo: c@d.ru\r\nSubject: s\r\nX-Soroka-Draft: sn-1\r\n\r\nтело";
        assert_eq!(out, expected.as_bytes());
        // Нет пустой строки (битое письмо) — возвращаем как есть.
        assert_eq!(
            insert_draft_header(b"no body here", "sn-1"),
            b"no body here"
        );
    }

    #[test]
    fn extra_roundtrip() {
        let atts = vec![
            (
                "отчёт.pdf".to_string(),
                "application/pdf".to_string(),
                vec![1u8, 2, 3],
            ),
            ("данные.csv".to_string(), "text/csv".to_string(), vec![]),
        ];
        let enc = encode_extra("cc@x.ru", &atts).expect("json");
        let (cc, back) = decode_extra(Some(&enc));
        assert_eq!(cc, "cc@x.ru");
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].0, "отчёт.pdf");
        assert_eq!(back[0].1, "application/pdf");
        assert_eq!(back[0].2, vec![1u8, 2, 3]);
        assert_eq!(back[1].2, Vec::<u8>::new());
        // Запись Сороки: None и пустая строка — без копии и вложений.
        assert_eq!(decode_extra(None), (String::new(), Vec::new()));
        assert_eq!(decode_extra(Some("")), (String::new(), Vec::new()));
        // Битый JSON — тоже (запись старой версии/чужого клиента).
        assert_eq!(decode_extra(Some("{oops")), (String::new(), Vec::new()));
    }

    #[test]
    fn extra_skips_broken_base64() {
        let extra = DraftExtra {
            cc: "cc@x.ru".into(),
            atts: vec![
                DraftFile {
                    name: "ok.bin".into(),
                    mime: "application/octet-stream".into(),
                    data_b64: "AQID".into(), // 1,2,3
                },
                DraftFile {
                    name: "битый".into(),
                    mime: "application/octet-stream".into(),
                    data_b64: "!!не base64!!".into(),
                },
            ],
        };
        let json = serde_json::to_string(&extra).unwrap();
        let (cc, atts) = decode_extra(Some(&json));
        assert_eq!(cc, "cc@x.ru");
        assert_eq!(atts.len(), 1);
        assert_eq!(atts[0].0, "ok.bin");
        assert_eq!(atts[0].2, vec![1u8, 2, 3]);
    }

    #[test]
    fn saved_label_format() {
        assert_eq!(saved_label(9, 5), "✓ сохранено 09:05");
        assert_eq!(saved_label(23, 59), "✓ сохранено 23:59");
    }
}
