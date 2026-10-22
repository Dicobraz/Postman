//! Путь JMAP (Stalwart): типы `postman_jmap` ↔ нейтральная модель, один к одному.

use crate::model::{Auth, ContactCard, Filter, Principal, Release, VacationResponse};

impl From<&Auth> for postman_jmap::Auth {
    fn from(a: &Auth) -> Self {
        match a {
            Auth::Basic { user, password } => Self::Basic { user: user.clone(), password: password.clone() },
            Auth::Bearer(t) => Self::Bearer(t.clone()),
        }
    }
}

impl From<postman_jmap::Auth> for Auth {
    fn from(a: postman_jmap::Auth) -> Self {
        match a {
            postman_jmap::Auth::Basic { user, password } => Self::Basic { user, password },
            postman_jmap::Auth::Bearer(t) => Self::Bearer(t),
        }
    }
}

impl From<&Filter> for postman_jmap::Filter {
    fn from(f: &Filter) -> Self {
        use postman_jmap::Filter as J;
        let all = |v: &[Filter]| v.iter().map(J::from).collect();
        match f {
            Filter::InMailbox(s) => J::InMailbox(s.clone()),
            Filter::HasKeyword(s) => J::HasKeyword(s.clone()),
            Filter::NotKeyword(s) => J::NotKeyword(s.clone()),
            Filter::Text(s) => J::Text(s.clone()),
            Filter::From(s) => J::From(s.clone()),
            Filter::To(s) => J::To(s.clone()),
            Filter::Body(s) => J::Body(s.clone()),
            Filter::Subject(s) => J::Subject(s.clone()),
            Filter::HasAttachment(b) => J::HasAttachment(*b),
            Filter::And(v) => J::And(all(v)),
            Filter::Or(v) => J::Or(all(v)),
            Filter::Not(v) => J::Not(all(v)),
            Filter::Before(s) => J::Before(s.clone()),
            Filter::After(s) => J::After(s.clone()),
            Filter::MinSize(n) => J::MinSize(*n),
        }
    }
}

impl From<Release> for postman_jmap::Release {
    fn from(r: Release) -> Self {
        match r {
            Release::Now => Self::Now,
            Release::After(s) => Self::After(s),
            Release::At(t) => Self::At(t),
        }
    }
}

impl From<postman_jmap::Release> for Release {
    fn from(r: postman_jmap::Release) -> Self {
        match r {
            postman_jmap::Release::Now => Self::Now,
            postman_jmap::Release::After(s) => Self::After(s),
            postman_jmap::Release::At(t) => Self::At(t),
        }
    }
}

impl From<postman_jmap::ContactCard> for ContactCard {
    fn from(c: postman_jmap::ContactCard) -> Self {
        let postman_jmap::ContactCard { id, name, emails, phones, organization, title } = c;
        Self { id, name, emails, phones, organization, title }
    }
}

impl From<&ContactCard> for postman_jmap::ContactCard {
    fn from(c: &ContactCard) -> Self {
        let ContactCard { id, name, emails, phones, organization, title } = c.clone();
        Self { id, name, emails, phones, organization, title }
    }
}

impl From<postman_jmap::Principal> for Principal {
    fn from(p: postman_jmap::Principal) -> Self {
        let postman_jmap::Principal { id, name, description, email, kind } = p;
        Self { id, name, description, email, kind }
    }
}

impl From<postman_jmap::VacationResponse> for VacationResponse {
    fn from(v: postman_jmap::VacationResponse) -> Self {
        let postman_jmap::VacationResponse { is_enabled, from_date, to_date, subject, text_body, html_body } = v;
        Self { is_enabled, from_date, to_date, subject, text_body, html_body }
    }
}

impl From<&VacationResponse> for postman_jmap::VacationResponse {
    fn from(v: &VacationResponse) -> Self {
        let VacationResponse { is_enabled, from_date, to_date, subject, text_body, html_body } = v.clone();
        Self { is_enabled, from_date, to_date, subject, text_body, html_body }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_maps_to_the_same_jmap_query() {
        let f = Filter::And(vec![
            Filter::InMailbox("m1".into()),
            Filter::Or(vec![Filter::HasKeyword("pm.due".into()), Filter::NotKeyword("$seen".into())]),
            Filter::Not(vec![Filter::From("@example.test".into())]),
            Filter::Text("t".into()),
            Filter::To("a".into()),
            Filter::Body("b".into()),
            Filter::Subject("s".into()),
            Filter::HasAttachment(true),
            Filter::Before("2026-10-21T00:00:00Z".into()),
            Filter::After("2026-10-01T00:00:00Z".into()),
            Filter::MinSize(1024),
        ]);
        let j = postman_jmap::Filter::And(vec![
            postman_jmap::Filter::InMailbox("m1".into()),
            postman_jmap::Filter::Or(vec![
                postman_jmap::Filter::HasKeyword("pm.due".into()),
                postman_jmap::Filter::NotKeyword("$seen".into()),
            ]),
            postman_jmap::Filter::Not(vec![postman_jmap::Filter::From("@example.test".into())]),
            postman_jmap::Filter::Text("t".into()),
            postman_jmap::Filter::To("a".into()),
            postman_jmap::Filter::Body("b".into()),
            postman_jmap::Filter::Subject("s".into()),
            postman_jmap::Filter::HasAttachment(true),
            postman_jmap::Filter::Before("2026-10-21T00:00:00Z".into()),
            postman_jmap::Filter::After("2026-10-01T00:00:00Z".into()),
            postman_jmap::Filter::MinSize(1024),
        ]);
        assert_eq!(postman_jmap::Filter::from(&f), j);
        assert_eq!(postman_jmap::Filter::from(&f).to_json(), j.to_json());
    }

    #[test]
    fn release_and_cards_round_trip() {
        for r in [Release::Now, Release::After(13), Release::At(1_792_562_400)] {
            assert_eq!(Release::from(postman_jmap::Release::from(r)), r);
        }
        let card = ContactCard {
            id: "c1".into(),
            name: "Анна".into(),
            emails: vec!["a@example.test".into()],
            phones: vec!["+7 900".into()],
            organization: "ИТТ".into(),
            title: "инженер".into(),
        };
        assert_eq!(ContactCard::from(postman_jmap::ContactCard::from(&card)), card);
        // Патч карточки — по-прежнему только изменённые свойства (форма «Контакты», `app-gpui/src/contacts.rs`).
        let edited = ContactCard { phones: Vec::new(), title: "Директор".into(), ..card.clone() };
        let patch = postman_jmap::ContactCard::from(&edited).patch(&(&card).into());
        let mut keys: Vec<&str> = patch.keys().map(String::as_str).collect();
        keys.sort();
        assert_eq!(keys, ["phones", "titles"]);
        assert!(patch["phones"].is_null(), "убранный телефон — null");
        assert_eq!(patch["titles"]["t1"]["name"], "Директор");
        let v = VacationResponse { is_enabled: true, subject: Some("Нет".into()), ..Default::default() };
        assert_eq!(VacationResponse::from(postman_jmap::VacationResponse::from(&v)), v);
    }
}
