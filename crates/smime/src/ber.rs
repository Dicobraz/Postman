//! Минимальный читатель BER/DER (только однобайтовые теги): обход CMS без строгого разбора всей структуры.
//!
//! Зачем свой, а не `der`: (1) Outlook пишет `signed-data` потоком — неопределённые длины и составные
//! `OCTET STRING` (BER); (2) подпись считается над **исходными** байтами `signedAttrs` и сертификатов, а строгий
//! разбор с повторным кодированием их может изменить (порядок в `SET OF`); (3) один странный сертификат в
//! `SignedData` не должен ронять проверку всего письма. Типизированный разбор отдельных частей — `der`/`cms`.

/// Элемент: тег, весь элемент (`raw`, с заголовком) и содержимое (`value`; у неопределённой длины — без `00 00`).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Tlv<'a> {
    pub tag: u8,
    pub raw: &'a [u8],
    pub value: &'a [u8],
    pub indefinite: bool,
}

/// Глубже вложенность не разбираем (враждебный ввод).
const MAX_DEPTH: usize = 48;

impl<'a> Tlv<'a> {
    pub(crate) fn constructed(&self) -> bool {
        self.tag & 0x20 != 0
    }

    /// Дочерние элементы составного элемента.
    pub(crate) fn children(&self) -> Option<Vec<Tlv<'a>>> {
        children(self.value, 0)
    }

    /// Содержимое строки (`OCTET STRING`): примитивной — как есть, составной (BER) — склеенные части.
    pub(crate) fn octets(&self) -> Option<Vec<u8>> {
        octets(self, 0)
    }
}

fn octets(t: &Tlv<'_>, depth: usize) -> Option<Vec<u8>> {
    if depth > MAX_DEPTH {
        return None;
    }
    if !t.constructed() {
        return Some(t.value.to_vec());
    }
    let mut out = Vec::new();
    for c in children(t.value, depth + 1)? {
        out.extend(octets(&c, depth + 1)?);
    }
    Some(out)
}

fn children(mut input: &[u8], depth: usize) -> Option<Vec<Tlv<'_>>> {
    let mut out = Vec::new();
    while !input.is_empty() {
        let (t, rest) = read_at(input, depth)?;
        out.push(t);
        input = rest;
    }
    Some(out)
}

/// Первый элемент и остаток.
pub(crate) fn read(input: &[u8]) -> Option<(Tlv<'_>, &[u8])> {
    read_at(input, 0)
}

fn read_at(input: &[u8], depth: usize) -> Option<(Tlv<'_>, &[u8])> {
    if depth > MAX_DEPTH {
        return None;
    }
    let tag = *input.first()?;
    if tag & 0x1f == 0x1f {
        return None;
    }
    let first = *input.get(1)?;
    let (len, header) = match first {
        l if l < 0x80 => (Some(usize::from(l)), 2),
        0x80 => (None, 2),
        l => {
            let n = usize::from(l & 0x7f);
            if n > 4 {
                return None;
            }
            let bytes = input.get(2..2 + n)?;
            let len = bytes.iter().fold(0usize, |acc, &b| (acc << 8) | usize::from(b));
            (Some(len), 2 + n)
        }
    };
    match len {
        Some(len) => {
            let end = header.checked_add(len)?;
            let raw = input.get(..end)?;
            Some((Tlv { tag, raw, value: &raw[header..], indefinite: false }, &input[end..]))
        }
        None => {
            // Неопределённая длина: только у составных; конец — `00 00` после дочерних.
            if tag & 0x20 == 0 {
                return None;
            }
            let mut pos = header;
            loop {
                if input.get(pos..pos + 2)? == [0, 0] {
                    let value = &input[header..pos];
                    let raw = &input[..pos + 2];
                    return Some((Tlv { tag, raw, value, indefinite: true }, &input[pos + 2..]));
                }
                let (child, _) = read_at(&input[pos..], depth + 1)?;
                pos += child.raw.len();
            }
        }
    }
}

/// DER-заголовок длины `len` с тегом `tag`.
pub(crate) fn header(tag: u8, len: usize) -> Vec<u8> {
    let mut out = vec![tag];
    if len < 0x80 {
        out.push(len as u8);
    } else {
        let bytes: Vec<u8> = len.to_be_bytes().iter().copied().skip_while(|&b| b == 0).collect();
        out.push(0x80 | bytes.len() as u8);
        out.extend(bytes);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definite_and_indefinite() {
        // SEQUENCE { INTEGER 5, OCTET STRING (составная, неопределённая) { "ab", "c" } }
        let ber = [0x30, 0x80, 0x02, 0x01, 0x05, 0x24, 0x80, 0x04, 0x02, b'a', b'b', 0x04, 0x01, b'c', 0, 0, 0, 0, 0xff];
        let (seq, rest) = read(&ber).unwrap();
        assert_eq!(rest, &[0xff]);
        assert!(seq.indefinite);
        let kids = seq.children().unwrap();
        assert_eq!(kids.len(), 2);
        assert_eq!(kids[0].value, &[5]);
        assert_eq!(kids[1].octets().unwrap(), b"abc");
        let der = [0x30, 0x81, 0x03, 0x02, 0x01, 0x07];
        let (t, _) = read(&der).unwrap();
        assert_eq!(t.children().unwrap()[0].value, &[7]);
        assert_eq!(header(0x31, 3), vec![0x31, 3]);
        assert_eq!(header(0x31, 300), vec![0x31, 0x82, 1, 44]);
    }

    #[test]
    fn hostile_input_does_not_panic() {
        assert!(read(&[0x30, 0x84, 0xff, 0xff, 0xff, 0xff]).is_none());
        assert!(read(&[0x04, 0x80, 0, 0]).is_none(), "неопределённая длина у примитивного");
        assert!(read(&[0x30, 0x80, 0x30, 0x80]).is_none());
        // Глубокая вложенность неопределённых длин.
        let deep: Vec<u8> = std::iter::repeat_n([0x30u8, 0x80], 10_000).flatten().collect();
        assert!(read(&deep).is_none());
        for n in 0..20 {
            let junk: Vec<u8> = (0..n).map(|i| (i * 37 + 11) as u8).collect();
            let _ = read(&junk).map(|(t, _)| (t.children(), t.octets()));
        }
    }
}
