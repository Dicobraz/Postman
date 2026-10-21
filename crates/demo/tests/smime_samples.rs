//! Подписанные образцы демо-ящика (S5-12 Ф1): `POSTMAN_UPDATE_SAMPLES=1 cargo test -p postman-demo --test
//! smime_samples` — пересобрать `src/smime/*`; без переменной — проверить, что образцы дают ожидаемые итоги.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // интеграционный тест: «не падать» — для кода приложения (S2-03)

use postman_demo::smime::{SAMPLES, Sample, anchors, raw};
use postman_smime::testkit::{self, Head, Key, Sign, Spec, issue};
use postman_smime::{Anchor, Status, Trust, oid};

fn head(s: &Sample) -> String {
    format!("{} <{}>", s.from_name, s.from_addr)
}

fn build() -> Vec<(String, Vec<u8>)> {
    let date = "Wed, 21 Oct 2026 09:30:00 +0300";
    let rsa_root_key = Key::rsa('a');
    let rsa_root = issue(&Spec { org: Some("Корпорация (демо)".into()), ..Spec::ca("Корпоративный УЦ (демо)") }, &rsa_root_key, None);
    let gost_root_key = Key::gost("demo-gost-root", oid::TC26_256_B);
    let gost_root = issue(&Spec { org: Some("АО «Контрагент» (демо)".into()), ..Spec::ca("УЦ ГОСТ (демо)") }, &gost_root_key, None);
    let other_key = Key::p256("demo-vector-root");
    let other_root = issue(&Spec { org: Some("ООО «Вектор»".into()), ..Spec::ca("Vector Internal CA") }, &other_key, None);
    let mut out = vec![("ca-rsa.der".to_string(), rsa_root.der.clone()), ("ca-gost.der".to_string(), gost_root.der.clone())];
    for s in SAMPLES {
        let (root, root_key, user_key): (&testkit::Issued, &Key, Key) = match s.kind {
            "gost" => (&gost_root, &gost_root_key, Key::gost("demo-gost-user", oid::TC26_256_B)),
            "untrusted" => (&other_root, &other_key, Key::p256("demo-vector-user")),
            _ => (&rsa_root, &rsa_root_key, Key::rsa('b')),
        };
        let user = issue(&Spec { org: root.cert.tbs_certificate().subject().organization().ok().flatten().map(|o| o.value().into_owned()), ..Spec::user(s.from_name, s.from_addr) }, &user_key, Some((root, root_key)));
        let from = head(s);
        let h = Head { from: &from, to: "Смирнова Анна <me@corp.example>", subject: s.subject, date };
        let opts = Sign { signing_time: Some(1_792_567_800), ..Sign::default() };
        let eml = if s.html == s.signed_html {
            testkit::signed_letter(&h, s.html, &user, &user_key, &[root], &opts)
        } else {
            // Текст изменён после подписи: подпись — над исходным, в письме — другой.
            let p7s = testkit::signed_data(&testkit::html_part(s.signed_html), &user, &user_key, &[root], &opts);
            testkit::multipart_signed(&h, &testkit::html_part(s.html), &p7s, testkit::micalg(user_key.default_scheme().hash()))
        };
        out.push((format!("{}.eml", s.kind), eml));
    }
    out
}

#[test]
fn samples_give_expected_verdicts() {
    if std::env::var_os("POSTMAN_UPDATE_SAMPLES").is_some() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("smime");
        std::fs::create_dir_all(&dir).unwrap();
        for (name, bytes) in build() {
            std::fs::write(dir.join(name), bytes).unwrap();
        }
        return;
    }
    let trust = Trust { roots: anchors().into_iter().map(|(der, corporate)| Anchor { der, corporate }).collect(), ..Default::default() };
    let now = 1_792_600_000;
    let check = |kind: &str| postman_smime::verify(raw(kind).unwrap(), &trust, now);
    let r = check("rsa");
    assert_eq!(r.status, Status::Valid, "{r:?}");
    assert!(r.corporate && r.address_ok);
    let g = check("gost");
    assert!(g.status == Status::Valid && g.is_gost() && !g.corporate, "{g:?}");
    assert_eq!(check("bad").status, Status::Invalid);
    assert!(matches!(check("untrusted").status, Status::Untrusted(_)));
}
