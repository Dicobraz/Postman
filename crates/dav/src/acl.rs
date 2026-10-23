//! Права коллекции (WebDAV ACL, RFC 3744): кому открыт календарь. Нужны занятости: Stalwart отдаёт её по
//! outbox, только если календарь участника открыт спрашивающему (`docs/STALWART.md` п. 6) — как `ShareFreeBusy`
//! у JMAP. Метод `ACL` заменяет список целиком: читаем, меняем одну запись, пишем свои (не `protected` и не
//! унаследованные) записи обратно.

use crate::multistatus::same_href;
use crate::xml::{self, CALDAV, CARDDAV, DAV, Element, escape};
use crate::{Client, Error, Result};

/// Право (`DAV:read`, `CALDAV:read-free-busy` …).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Privilege {
    pub ns: String,
    pub name: String,
}

impl Privilege {
    pub fn new(ns: &str, name: &str) -> Self {
        Self { ns: ns.into(), name: name.into() }
    }

    pub fn read() -> Self {
        Self::new(DAV, "read")
    }

    pub fn write() -> Self {
        Self::new(DAV, "write")
    }

    /// Только занятость (RFC 4791 §6.1.1).
    pub fn read_free_busy() -> Self {
        Self::new(CALDAV, "read-free-busy")
    }

    fn xml(&self) -> String {
        let name = escape(&self.name);
        match self.ns.as_str() {
            DAV => format!("<d:{name}/>"),
            CALDAV => format!("<c:{name}/>"),
            CARDDAV => format!("<r:{name}/>"),
            ns => format!("<x:{name} xmlns:x=\"{}\"/>", escape(ns)),
        }
    }
}

/// Кому запись (RFC 3744 §5.5.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Who {
    /// Принципал по пути (`/dav/pal/user1%40itt.test/`).
    Href(String),
    All,
    Authenticated,
    Unauthenticated,
    /// `DAV:self`.
    Myself,
    /// `DAV:property` и прочее — записать обратно не умеем.
    Other,
}

/// Запись списка прав.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ace {
    pub who: Who,
    pub grant: Vec<Privilege>,
    pub deny: Vec<Privilege>,
    /// Защищена сервером — не меняется и не пишется.
    pub protected: bool,
    /// Унаследована от родителя — не пишется.
    pub inherited: bool,
}

impl Ace {
    pub fn grant(principal: &str, privileges: &[Privilege]) -> Self {
        Self { who: Who::Href(principal.into()), grant: privileges.to_vec(), deny: Vec::new(), protected: false, inherited: false }
    }

    fn own(&self) -> bool {
        !self.protected && !self.inherited
    }

    fn is_for(&self, principal: &str) -> bool {
        matches!(&self.who, Who::Href(h) if same_href(h, principal))
    }
}

impl Client {
    /// Список прав коллекции.
    pub async fn acl(&self, href: &str) -> Result<Vec<Ace>> {
        let ms = self.multistatus("PROPFIND", href, Some("0"), xml::propfind(&["d:acl"])).await?;
        let acl = ms.responses.first().and_then(|r| r.prop(DAV, "acl")).ok_or_else(|| Error::Protocol("no DAV:acl".into()))?;
        Ok(parse_acl(acl))
    }

    /// Записать свои записи (`protected` и унаследованные сервер держит сам).
    pub async fn set_acl(&self, href: &str, aces: &[Ace]) -> Result<()> {
        let body = acl_body(aces)?;
        let reply = self.send("ACL", href, &[], Some(("application/xml; charset=utf-8", body))).await?;
        if !(200..300).contains(&reply.status) {
            return Err(crate::status_error(&reply));
        }
        Ok(())
    }

    /// Открыть коллекцию принципалу с этими правами (прежняя запись для него — заменяется).
    pub async fn share(&self, href: &str, principal: &str, privileges: &[Privilege]) -> Result<()> {
        let mut aces: Vec<Ace> = self.acl(href).await?.into_iter().filter(|a| a.own() && !a.is_for(principal)).collect();
        aces.push(Ace::grant(principal, privileges));
        self.set_acl(href, &aces).await
    }

    /// Закрыть коллекцию принципалу (его запись убирается). `false` — записи и не было.
    pub async fn unshare(&self, href: &str, principal: &str) -> Result<bool> {
        let all = self.acl(href).await?;
        if !all.iter().any(|a| a.own() && a.is_for(principal)) {
            return Ok(false);
        }
        let aces: Vec<Ace> = all.into_iter().filter(|a| a.own() && !a.is_for(principal)).collect();
        self.set_acl(href, &aces).await?;
        Ok(true)
    }
}

/// Записи из свойства `DAV:acl`.
pub fn parse_acl(acl: &Element) -> Vec<Ace> {
    let privileges = |e: Option<&Element>| -> Vec<Privilege> {
        e.map(|g| g.all(DAV, "privilege").flat_map(|p| p.children.iter().map(|c| Privilege::new(&c.ns, &c.name))).collect()).unwrap_or_default()
    };
    acl.all(DAV, "ace")
        .map(|ace| {
            let who = match ace.child(DAV, "principal").and_then(|p| p.children.first()) {
                Some(p) if p.is(DAV, "href") => Who::Href(p.value().to_owned()),
                Some(p) if p.is(DAV, "all") => Who::All,
                Some(p) if p.is(DAV, "authenticated") => Who::Authenticated,
                Some(p) if p.is(DAV, "unauthenticated") => Who::Unauthenticated,
                Some(p) if p.is(DAV, "self") => Who::Myself,
                _ => Who::Other,
            };
            Ace {
                who,
                grant: privileges(ace.child(DAV, "grant")),
                deny: privileges(ace.child(DAV, "deny")),
                protected: ace.child(DAV, "protected").is_some(),
                inherited: ace.child(DAV, "inherited").is_some(),
            }
        })
        .collect()
}

/// Тело `ACL`: только свои записи.
fn acl_body(aces: &[Ace]) -> Result<String> {
    let mut s = xml::open("d:acl");
    for a in aces.iter().filter(|a| a.own()) {
        let who = match &a.who {
            Who::Href(h) => format!("<d:href>{}</d:href>", escape(h)),
            Who::All => "<d:all/>".into(),
            Who::Authenticated => "<d:authenticated/>".into(),
            Who::Unauthenticated => "<d:unauthenticated/>".into(),
            Who::Myself => "<d:self/>".into(),
            Who::Other => return Err(Error::Protocol("ACL entry for an unsupported principal".into())),
        };
        s.push_str(&format!("<d:ace><d:principal>{who}</d:principal>"));
        for (tag, list) in [("grant", &a.grant), ("deny", &a.deny)] {
            if list.is_empty() {
                continue;
            }
            s.push_str(&format!("<d:{tag}>"));
            for p in list {
                s.push_str(&format!("<d:privilege>{}</d:privilege>", p.xml()));
            }
            s.push_str(&format!("</d:{tag}>"));
        }
        s.push_str("</d:ace>");
    }
    s.push_str("</d:acl>");
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stand_acl_round_trips() {
        let ms = crate::multistatus::parse(include_bytes!("../tests/fixtures/acl.xml")).unwrap();
        let aces = parse_acl(ms.responses[0].prop(DAV, "acl").unwrap());
        assert_eq!(aces.len(), 1);
        assert_eq!(aces[0].who, Who::Href("/dav/pal/user1%40itt.test/".into()));
        assert!(aces[0].grant.contains(&Privilege::read()) && aces[0].grant.contains(&Privilege::read_free_busy()));
        let body = acl_body(&aces).unwrap();
        let back = crate::xml::parse(body.as_bytes()).unwrap();
        assert_eq!(parse_acl(&back), aces);
    }

    #[test]
    fn protected_entries_are_not_written_and_unknown_principals_refuse() {
        let doc = br#"<d:acl xmlns:d="DAV:"><d:ace><d:principal><d:self/></d:principal><d:grant><d:privilege><d:all/></d:privilege></d:grant><d:protected/></d:ace>
<d:ace><d:principal><d:all/></d:principal><d:deny><d:privilege><d:write/></d:privilege></d:deny></d:ace></d:acl>"#;
        let aces = parse_acl(&crate::xml::parse(doc).unwrap());
        assert_eq!((aces[0].who.clone(), aces[0].protected), (Who::Myself, true));
        let body = acl_body(&aces).unwrap();
        assert!(!body.contains("<d:self/>") && body.contains("<d:deny><d:privilege><d:write/></d:privilege></d:deny>"));
        let odd = [Ace { who: Who::Other, ..Ace::grant("/p/", &[]) }];
        assert!(acl_body(&odd).is_err());
        let x = Privilege::new("urn:x", "y").xml();
        assert_eq!(x, "<x:y xmlns:x=\"urn:x\"/>");
    }
}
