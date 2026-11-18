//! Разбор .ico для трея и иконки окна (TASK-N07).
//!
//! Файл .ico — это ICONDIR (6 байт) + массив записей по 16 байт + данные
//! кадров. `CreateIconFromResourceEx` принимает ОДИН кадр (данные записи,
//! PNG или DIB), поэтому нужен выбор лучшего кадра под размер панели задач.
//! Разбор — чистые байты, без Win32: тестируется на Linux в логик-прогоне.

/// Запись каталога .ico.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IcoEntry {
    /// Ширина кадра (0 в файле означает 256).
    pub width: u16,
    pub height: u16,
    /// Бит на пиксель.
    pub bpp: u16,
    /// Смещение данных кадра от начала файла.
    pub offset: u32,
    /// Длина данных кадра.
    pub len: u32,
}

/// Фактическая ширина записи (0 → 256).
fn real_dim(v: u16) -> u16 {
    if v == 0 { 256 } else { v }
}

/// Каталог кадров; `None` — файл не похож на .ico (короткий, не тот тип,
/// записи выходят за границы файла).
pub fn entries(data: &[u8]) -> Option<Vec<IcoEntry>> {
    if data.len() < 6 {
        return None;
    }
    // reserved == 0, type == 1 (иконка, не курсор).
    if data[0] != 0 || data[1] != 0 || data[2] != 1 || data[3] != 0 {
        return None;
    }
    let count = u16::from_le_bytes([data[4], data[5]]) as usize;
    if count == 0 {
        return None;
    }
    let need = 6 + count * 16;
    if data.len() < need {
        return None;
    }
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let e = &data[6 + i * 16..6 + i * 16 + 16];
        // bWidth/bHeight — одиночные байты (0 = 256); дальше bColorCount,
        // bReserved, wPlanes, wBitCount, dwBytesInRes, dwImageOffset.
        let width = e[0] as u16;
        let height = e[1] as u16;
        let bpp = u16::from_le_bytes([e[6], e[7]]);
        let len = u32::from_le_bytes([e[8], e[9], e[10], e[11]]);
        let offset = u32::from_le_bytes([e[12], e[13], e[14], e[15]]);
        // Кадр должен целиком лежать в файле — битые записи отбрасываем.
        let end = offset.checked_add(len)?;
        if (end as usize) > data.len() {
            return None;
        }
        out.push(IcoEntry {
            width: real_dim(width),
            height: real_dim(height),
            bpp,
            offset,
            len,
        });
    }
    Some(out)
}

/// Лучший кадр под запрошенный размер: точное совпадение, иначе ближайший
/// БОЛЬШЕ (даунскейл), иначе самый большой из меньших. Возвращает срез
/// данных кадра.
pub fn best_frame(data: &[u8], size: i32) -> Option<&[u8]> {
    let list = entries(data)?;
    let target = size.max(1) as u16;
    let pick = list.iter().min_by(|a, b| {
        let da = (i32::from(a.width) - i32::from(target)).abs();
        let db = (i32::from(b.width) - i32::from(target)).abs();
        // Ближе по размеру — лучше; при равенстве дистанции крупнее —
        // лучше (даунскейл чище апскейла).
        da.cmp(&db)
            .then(i32::from(b.width).cmp(&i32::from(a.width)))
    })?;
    let e = *pick;
    let start = e.offset as usize;
    let end = start + e.len as usize;
    data.get(start..end)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Минимальный валидный .ico: каталог + кадры, данные каждой записи —
    /// свой байт-заполнитель (чтобы видеть, какой кадр выбран).
    fn ico(frames: &[(u16, u32, u8)]) -> Vec<u8> {
        let mut d = vec![0, 0, 1, 0, frames.len() as u8, 0];
        let mut off = 6 + frames.len() * 16;
        for &(w, len, _) in frames {
            let head = [w as u8, w as u8, 0, 0, 1, 0, 32, 0];
            let l = len.to_le_bytes();
            let o = (off as u32).to_le_bytes();
            d.extend(head.iter().chain(l.iter()).chain(o.iter()).copied());
            off += len as usize;
        }
        for &(_, len, payload) in frames {
            d.extend(std::iter::repeat_n(payload, len as usize));
        }
        d
    }

    #[test]
    fn parses_entries() {
        // Один кадр 32×32, данные — с offset 22 (6 + 16), длина 64.
        let d = ico(&[(32, 64, 0xAA)]);
        let es = entries(&d).expect("каталог");
        assert_eq!(es.len(), 1);
        assert_eq!(es[0].width, 32);
        assert_eq!(es[0].height, 32);
        assert_eq!(es[0].bpp, 32);
        assert_eq!(es[0].offset, 22);
        assert_eq!(es[0].len, 64);
    }

    #[test]
    fn rejects_garbage() {
        assert!(entries(b"").is_none());
        assert!(entries(b"\x00\x00\x00\x00\x00\x00").is_none()); // count = 0
        assert!(entries(b"\x00\x00\x02\x00\x01\x00").is_none()); // курсор, не иконка
        // Запись обещает больше, чем есть в файле.
        let d = ico(&[(32, 64, 0xAA)]);
        assert!(entries(&d[..6 + 16]).is_none());
    }

    #[test]
    fn zero_width_means_256() {
        let d = ico(&[(0, 64, 0xAA)]);
        let es = entries(&d).expect("каталог");
        assert_eq!(es[0].width, 256);
        assert_eq!(es[0].height, 256);
    }

    #[test]
    fn best_frame_prefers_exact_then_larger() {
        // Кадры 16/32/48px с разными заполнителями.
        let d = ico(&[(16, 32, 0x11), (32, 64, 0x22), (48, 64, 0x33)]);
        fn want(d: &[u8], off: usize, len: usize) -> Option<&[u8]> {
            d.get(off..off + len)
        }
        assert_eq!(best_frame(&d, 16), want(&d, 54, 32)); // точное совпадение
        assert_eq!(best_frame(&d, 32), want(&d, 54 + 32, 64));
        // 20 нет: дистанции 4 (16) и 12 (32) — берём 16.
        assert_eq!(best_frame(&d, 20), want(&d, 54, 32));
        // 24: дистанции 8 (16) и 8 (32) — при равной дистанции крупнее.
        assert_eq!(best_frame(&d, 24), want(&d, 54 + 32, 64));
        // 40: дистанции 8 (32) и 8 (48) — берём 48.
        assert_eq!(best_frame(&d, 40), want(&d, 54 + 96, 64));
        // За пределами диапазона — ближайший имеющийся (48).
        assert_eq!(best_frame(&d, 200), want(&d, 54 + 96, 64));
    }

    #[test]
    fn real_ico_files_parse() {
        // Основной крейт: <манифест>/src/icons; логик-прогон на Linux
        // (scripts/n07-logic-tests) — через относительный путь в репозиторий.
        let base = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let dir = base.join("src/icons");
        let dir = if dir.is_dir() {
            dir
        } else {
            // Логик-прогон: scripts/n07-logic-tests → две ступени вверх.
            base.join("../../soroka-native/src/icons")
        };
        // Встроенные иконки среза: у каждой есть кадры и лучший для 16px.
        for f in ["tray.ico", "tray-alert.ico", "app.ico"] {
            let p = dir.join(f);
            let d = std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
            let es = entries(&d).unwrap_or_else(|| panic!("{}: не разобрался", p.display()));
            assert!(!es.is_empty(), "{}: пусто", p.display());
            assert!(
                best_frame(&d, 16).is_some(),
                "{}: нет кадра для 16px",
                p.display()
            );
            assert!(
                best_frame(&d, 32).is_some(),
                "{}: нет кадра для 32px",
                p.display()
            );
        }
    }
}
