//! Вкладки окна (S5-01, DESIGN «Главное окно»): заголовок окна — только вкладки. «Почта» — первая, не закрывается;
//! остальные открываются и закрываются (Ctrl+W), переключаются Ctrl+Tab / Ctrl+Shift+Tab. Модель общая:
//! настройки ящика (автоответ, правила), письма (S5-06б: по вкладке на черновик), дальше — настройки (S5-09).

/// Что во вкладке.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum TabKind {
    /// Почта, календарь, контакты (рейл модулей слева) — главная вкладка.
    Mail,
    /// Панель настроек ящика на сервере (`settings::Panel`).
    Settings,
    /// Письмо (`composer::Composer` с этим `id`); черновиков — сколько угодно.
    Compose(u64),
}

impl TabKind {
    /// Главную вкладку не закрыть.
    pub(crate) fn closable(self) -> bool {
        self != TabKind::Mail
    }

    /// Вкладка этого вида одна: повторное открытие показывает её.
    fn single(self) -> bool {
        matches!(self, TabKind::Mail | TabKind::Settings)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct Tab {
    /// Постоянный номер вкладки (id элемента в заголовке, адрес закрытия).
    pub(crate) id: u64,
    pub(crate) kind: TabKind,
}

/// Вкладки по порядку и активная.
#[derive(Debug)]
pub(crate) struct Tabs {
    tabs: Vec<Tab>,
    active: usize,
    next_id: u64,
}

impl Default for Tabs {
    fn default() -> Self {
        Tabs { tabs: vec![Tab { id: 0, kind: TabKind::Mail }], active: 0, next_id: 1 }
    }
}

impl Tabs {
    pub(crate) fn iter(&self) -> impl Iterator<Item = &Tab> {
        self.tabs.iter()
    }

    pub(crate) fn active(&self) -> Tab {
        self.tabs.get(self.active).copied().unwrap_or(Tab { id: 0, kind: TabKind::Mail })
    }

    pub(crate) fn is_active(&self, kind: TabKind) -> bool {
        self.active().kind == kind
    }

    /// Открыть вкладку (одиночную — показать уже открытую) и сделать её активной; её id.
    pub(crate) fn open(&mut self, kind: TabKind) -> u64 {
        if kind.single()
            && let Some(i) = self.tabs.iter().position(|t| t.kind == kind)
        {
            self.active = i;
            return self.tabs[i].id;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.tabs.push(Tab { id, kind });
        self.active = self.tabs.len() - 1;
        id
    }

    /// Сделать активной; нет такой — `false`.
    pub(crate) fn activate(&mut self, id: u64) -> bool {
        match self.tabs.iter().position(|t| t.id == id) {
            Some(i) => {
                self.active = i;
                true
            }
            None => false,
        }
    }

    /// Активной — вкладку вида (первую); нет — ничего.
    pub(crate) fn activate_kind(&mut self, kind: TabKind) -> bool {
        match self.tabs.iter().find(|t| t.kind == kind).map(|t| t.id) {
            Some(id) => self.activate(id),
            None => false,
        }
    }

    /// Закрыть вкладку; главную не закрыть. Закрыта активная — активной становится соседняя слева.
    pub(crate) fn close(&mut self, id: u64) -> Option<Tab> {
        let i = self.tabs.iter().position(|t| t.id == id)?;
        if !self.tabs[i].kind.closable() {
            return None;
        }
        let tab = self.tabs.remove(i);
        if self.active > i || (self.active == i && i > 0) {
            self.active -= 1;
        }
        self.active = self.active.min(self.tabs.len().saturating_sub(1));
        Some(tab)
    }

    /// Закрыть вкладку вида, если она открыта.
    pub(crate) fn close_kind(&mut self, kind: TabKind) -> Option<Tab> {
        let id = self.tabs.iter().find(|t| t.kind == kind)?.id;
        self.close(id)
    }

    /// Ctrl+Tab (`1`) / Ctrl+Shift+Tab (`-1`): по кругу.
    pub(crate) fn step(&mut self, delta: isize) {
        let n = self.tabs.len() as isize;
        if n > 0 {
            self.active = (self.active as isize + delta).rem_euclid(n) as usize;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{TabKind, Tabs};

    #[test]
    fn mail_tab_stays_and_others_open_close_and_cycle() {
        let mut t = Tabs::default();
        assert_eq!(t.active().kind, TabKind::Mail);
        assert!(t.close(0).is_none(), "«Почта» не закрывается");
        let s = t.open(TabKind::Settings);
        assert!(t.is_active(TabKind::Settings));
        assert_eq!(t.open(TabKind::Settings), s, "вторая вкладка настроек — та же");
        assert_eq!(t.iter().count(), 2);
        t.step(1);
        assert!(t.is_active(TabKind::Mail), "по кругу");
        t.step(-1);
        assert!(t.is_active(TabKind::Settings));
        assert!(t.activate_kind(TabKind::Mail));
        assert_eq!(t.close_kind(TabKind::Settings).map(|t| t.id), Some(s));
        assert!(t.is_active(TabKind::Mail));
        // Закрыли активную — активна соседняя слева.
        t.open(TabKind::Settings);
        assert_eq!(t.close(s + 1).map(|t| t.kind), Some(TabKind::Settings));
        assert!(t.is_active(TabKind::Mail));
        assert!(!t.activate(99));
    }

    /// S5-06б: у каждого черновика своя вкладка; закрыть одну — остальные на месте.
    #[test]
    fn every_draft_has_its_own_tab() {
        let mut t = Tabs::default();
        let a = t.open(TabKind::Compose(7));
        let b = t.open(TabKind::Compose(8));
        assert_ne!(a, b);
        assert!(t.is_active(TabKind::Compose(8)));
        assert_eq!(t.iter().count(), 3);
        assert_eq!(t.close(b).map(|t| t.kind), Some(TabKind::Compose(8)));
        assert!(t.is_active(TabKind::Compose(7)), "активна соседняя слева");
        assert!(t.activate_kind(TabKind::Mail));
    }
}
