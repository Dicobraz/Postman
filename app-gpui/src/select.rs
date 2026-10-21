//! Выделение писем в списке (S5-08, DESIGN «Действия и состояния»): флажки на месте аватаров, Ctrl+щелчок —
//! добавить или убрать, Shift+щелчок и Shift+↑/↓ — диапазон от якоря, Ctrl+A — все, Esc — снять.
//!
//! Выделение — по id писем, а не по номерам строк: список перечитывается (синхронизация, сортировка, фильтры), а
//! выделенные письма остаются выделенными, где бы они ни оказались; исчезнувшие из списка уходят и из выделения
//! ([`Selection::retain`], проверка — в фоне по кэшу). Другая папка или поиск — выделение снимается. Без gpui — тесты.

use std::collections::HashSet;

/// Выделенные письма и якорь диапазона.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Selection {
    /// По порядку выделения (сводка показывает первые).
    ids: Vec<String>,
    set: HashSet<String>,
    /// Якорь Shift-диапазона: письмо и его номер в списке (номер обновляется после перечитывания).
    anchor: Option<(String, usize)>,
    /// Что выделено до Shift-диапазона: диапазон заменяет только свою часть (Ctrl+щелчок, затем Shift+щелчок).
    base: Vec<String>,
}

impl Selection {
    pub(crate) fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub(crate) fn len(&self) -> usize {
        self.ids.len()
    }

    pub(crate) fn contains(&self, id: &str) -> bool {
        self.set.contains(id)
    }

    pub(crate) fn ids(&self) -> &[String] {
        &self.ids
    }

    pub(crate) fn anchor(&self) -> Option<usize> {
        self.anchor.as_ref().map(|(_, i)| *i)
    }

    fn add(&mut self, id: &str) {
        if self.set.insert(id.to_owned()) {
            self.ids.push(id.to_owned());
        }
    }

    fn remove(&mut self, id: &str) {
        if self.set.remove(id) {
            self.ids.retain(|x| x != id);
        }
    }

    /// Снять всё.
    pub(crate) fn clear(&mut self) {
        *self = Selection::default();
    }

    /// Ctrl+щелчок или флажок: письмо `id` (строка `index`) — в выделение или из него; оно — новый якорь.
    pub(crate) fn toggle(&mut self, id: &str, index: usize) {
        if self.contains(id) {
            self.remove(id);
        } else {
            self.add(id);
        }
        self.anchor = Some((id.to_owned(), index));
        self.base = self.ids.clone();
    }

    /// Строка беседы (вид «как Outlook», S5-05б): все её письма — в выделение, если хоть одного там нет, иначе — из
    /// него; якорь — строка.
    pub(crate) fn toggle_all(&mut self, ids: &[String], index: usize) {
        let Some(first) = ids.first() else { return };
        if ids.iter().all(|id| self.contains(id)) {
            ids.iter().for_each(|id| self.remove(id));
        } else {
            ids.iter().for_each(|id| self.add(id));
        }
        self.anchor = Some((first.clone(), index));
        self.base = self.ids.clone();
    }

    /// Начать с одного письма (оно же якорь): Shift+↑/↓ от открытого письма.
    pub(crate) fn start(&mut self, id: &str, index: usize) {
        if self.anchor.is_none() {
            self.clear();
            self.add(id);
            self.anchor = Some((id.to_owned(), index));
            self.base = Vec::new();
        }
    }

    /// Shift: диапазон от якоря до письма `to` — `range` — id строк диапазона по порядку списка (от меньшего номера
    /// к большему). Выделенное до диапазона Ctrl+щелчками остаётся; прежний Shift-диапазон заменяется.
    pub(crate) fn set_range(&mut self, range: &[String]) {
        let base = std::mem::take(&mut self.base);
        let anchor = self.anchor.take();
        self.ids.clear();
        self.set.clear();
        for id in base.iter().chain(range) {
            self.add(id);
        }
        self.anchor = anchor;
        self.base = base;
    }

    /// Ctrl+A: все письма списка (по порядку списка); якоря нет.
    pub(crate) fn select_all(&mut self, ids: impl IntoIterator<Item = String>) {
        self.clear();
        for id in ids {
            self.add(&id);
        }
        self.base = self.ids.clone();
    }

    /// Список перечитан: остаются только письма, которые в нём есть; якорь — на новом номере своего письма
    /// (`index_of`), письма нет — якоря нет. `true` — выделение изменилось.
    pub(crate) fn retain(&mut self, alive: &HashSet<String>, index_of: impl Fn(&str) -> Option<usize>) -> bool {
        let before = self.ids.len();
        self.ids.retain(|id| alive.contains(id));
        self.set.retain(|id| alive.contains(id));
        self.base.retain(|id| alive.contains(id));
        if let Some((id, i)) = &mut self.anchor {
            match index_of(id) {
                Some(n) => *i = n,
                None if !alive.contains(id.as_str()) => self.anchor = None,
                None => {}
            }
        }
        self.ids.len() != before
    }
}

/// Номера строк Shift-диапазона от якоря `anchor` до `to` включительно, по порядку списка.
pub(crate) fn span(anchor: usize, to: usize) -> std::ops::RangeInclusive<usize> {
    anchor.min(to)..=anchor.max(to)
}

#[cfg(test)]
mod tests {
    use super::{Selection, span};
    use std::collections::HashSet;

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// Ctrl+щелчки — по одному; Shift+щелчок — диапазон от последнего Ctrl-щелчка, прежние Ctrl-письма остаются.
    #[test]
    fn ranges_from_the_anchor_keep_ctrl_picks() {
        let mut s = Selection::default();
        s.toggle("a", 0);
        s.toggle("e", 4);
        assert_eq!(s.ids(), ids(&["a", "e"]));
        assert_eq!(s.anchor(), Some(4));
        // Shift+щелчок по строке 6: диапазон 4..=6.
        assert_eq!(span(4, 6), 4..=6);
        s.set_range(&ids(&["e", "f", "g"]));
        assert_eq!(s.ids(), ids(&["a", "e", "f", "g"]));
        // Другой Shift-конец (вверх до 2) — прежний диапазон заменён, якорь тот же.
        assert_eq!(span(4, 2), 2..=4);
        s.set_range(&ids(&["c", "d", "e"]));
        assert_eq!(s.ids(), ids(&["a", "e", "c", "d"]));
        assert_eq!(s.anchor(), Some(4));
        // Ctrl+щелчок по выделенному — снять.
        s.toggle("c", 2);
        assert!(!s.contains("c") && s.len() == 3);
    }

    /// Строка беседы — все её письма: щелчок добавляет недостающие, повторный снимает все.
    #[test]
    fn thread_row_picks_the_whole_conversation() {
        let mut s = Selection::default();
        s.toggle("a1", 3);
        s.toggle_all(&ids(&["a3", "a2", "a1"]), 0);
        assert_eq!(s.ids(), ids(&["a1", "a3", "a2"]));
        assert_eq!(s.anchor(), Some(0));
        s.toggle_all(&ids(&["a3", "a2", "a1"]), 0);
        assert!(s.is_empty());
        s.toggle_all(&[], 5);
        assert_eq!(s.anchor(), Some(0), "пустая беседа ничего не меняет");
    }

    /// Shift+↓ от открытого письма: начало — оно само, дальше — диапазон.
    #[test]
    fn shift_arrows_start_at_the_open_letter() {
        let mut s = Selection::default();
        s.start("b", 1);
        assert_eq!((s.ids(), s.anchor()), (ids(&["b"]).as_slice(), Some(1)));
        s.set_range(&ids(&["b", "c"]));
        s.start("x", 9);
        assert_eq!(s.ids(), ids(&["b", "c"]), "якорь уже есть — начало не сбрасывает");
        s.set_range(&ids(&["b"]));
        assert_eq!(s.ids(), ids(&["b"]), "назад к якорю — одно письмо");
    }

    /// Список перечитан (пришло новое письмо сверху, одно выделенное удалено на сервере): выделение — по id.
    #[test]
    fn selection_survives_list_updates_by_id() {
        let mut s = Selection::default();
        s.select_all(ids(&["a", "b", "c"]));
        assert_eq!(s.anchor(), None);
        s.toggle("b", 1);
        s.toggle("b", 1);
        assert_eq!(s.anchor(), Some(1));
        let alive: HashSet<String> = ids(&["new", "b", "c"]).into_iter().collect();
        let index_of = |id: &str| ["new", "c", "b"].iter().position(|x| *x == id);
        assert!(s.retain(&alive, index_of));
        assert_eq!(s.ids(), ids(&["c", "b"]));
        assert_eq!(s.anchor(), Some(2), "якорь — на новом месте своего письма");
        assert!(!s.retain(&alive, index_of), "ничего не изменилось");
        // Якорное письмо исчезло — якоря нет.
        let alive: HashSet<String> = ids(&["c"]).into_iter().collect();
        s.retain(&alive, |_| None);
        assert_eq!((s.ids(), s.anchor()), (ids(&["c"]).as_slice(), None));
        s.clear();
        assert!(s.is_empty());
    }
}
