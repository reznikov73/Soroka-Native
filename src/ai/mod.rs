//! ИИ-слой. В Фазе 1 перенесена только модель данных `AiAnalysis` —
//! `mail/store.rs` хранит ИИ-разборы писем в тех же колонках, что и Soroka,
//! чтобы база `%APPDATA%\Soroka\soroka.db` читалась без миграций.
//! Сетевая часть (GigaChat, reqwest) вернётся в Фазе 3.

pub mod gigachat;
