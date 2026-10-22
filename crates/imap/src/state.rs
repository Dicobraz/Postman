//! Состояние синхронизации папки IMAP: `UIDVALIDITY` (сменился — папку заново), `UIDNEXT` (новые письма — с него)
//! и `HIGHESTMODSEQ` (CONDSTORE: изменённые флаги — после него). В кэше — одной строкой (`sync_state`).

/// Состояние папки на сервере.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FolderState {
    pub uid_validity: u32,
    pub uid_next: u32,
    /// `None` — сервер без CONDSTORE: флаги сверяются целиком.
    pub modseq: Option<u64>,
}

impl FolderState {
    /// Строка для кэша: `v<UIDVALIDITY>.n<UIDNEXT>[.m<MODSEQ>]`.
    pub fn encode(&self) -> String {
        match self.modseq {
            Some(m) => format!("v{}.n{}.m{m}", self.uid_validity, self.uid_next),
            None => format!("v{}.n{}", self.uid_validity, self.uid_next),
        }
    }

    pub fn decode(s: &str) -> Option<FolderState> {
        let (mut v, mut n, mut m) = (None, None, None);
        for part in s.split('.') {
            let (tag, num) = part.split_at_checked(1)?;
            match tag {
                "v" => v = Some(num.parse().ok()?),
                "n" => n = Some(num.parse().ok()?),
                "m" => m = Some(num.parse().ok()?),
                _ => return None,
            }
        }
        Some(FolderState { uid_validity: v?, uid_next: n?, modseq: m })
    }
}

/// Что делать с папкой по прежнему и новому состоянию.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plan {
    /// Ничего не изменилось.
    Same,
    /// `UIDVALIDITY` другой (или папка впервые): письма папки — заново.
    Reload,
    /// Новые письма с `from` (UID) и, если есть CONDSTORE, флаги после `modseq`; удалённые — сверкой UID.
    Delta { from: u32, modseq: Option<u64> },
}

pub fn plan(old: Option<FolderState>, new: FolderState) -> Plan {
    match old {
        None => Plan::Reload,
        Some(o) if o.uid_validity != new.uid_validity => Plan::Reload,
        Some(o) if o == new && new.modseq.is_some() => Plan::Same,
        Some(o) => Plan::Delta { from: o.uid_next, modseq: o.modseq },
    }
}

#[cfg(test)]
mod tests {
    use super::{FolderState, Plan, plan};

    #[test]
    fn state_round_trip() {
        let s = FolderState { uid_validity: 7, uid_next: 120, modseq: Some(9001) };
        assert_eq!(s.encode(), "v7.n120.m9001");
        assert_eq!(FolderState::decode(&s.encode()), Some(s));
        let plain = FolderState { uid_validity: 1, uid_next: 2, modseq: None };
        assert_eq!(FolderState::decode(&plain.encode()), Some(plain));
        assert_eq!(FolderState::decode("garbage"), None);
        assert_eq!(FolderState::decode("v1"), None);
    }

    #[test]
    fn plans() {
        let a = FolderState { uid_validity: 7, uid_next: 120, modseq: Some(10) };
        assert_eq!(plan(None, a), Plan::Reload);
        assert_eq!(plan(Some(a), FolderState { uid_validity: 8, ..a }), Plan::Reload);
        assert_eq!(plan(Some(a), a), Plan::Same);
        assert_eq!(plan(Some(a), FolderState { uid_next: 125, modseq: Some(14), ..a }), Plan::Delta { from: 120, modseq: Some(10) });
        // Без CONDSTORE «то же» не бывает: флаги сверяются каждый раз.
        let p = FolderState { modseq: None, ..a };
        assert_eq!(plan(Some(p), p), Plan::Delta { from: 120, modseq: None });
    }
}
