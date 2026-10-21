//! Идентификаторы объектов: CMS (RFC 5652), S/MIME (RFC 8551), алгоритмы RSA/ECDSA (RFC 3279, 4055, 5758) и
//! ГОСТ (RFC 9215, рекомендации ТК 26, RFC 4357).

use const_oid::ObjectIdentifier as Oid;

// --- CMS и S/MIME ---
pub const SIGNED_DATA: Oid = Oid::new_unwrap("1.2.840.113549.1.7.2");
pub const ENVELOPED_DATA: Oid = Oid::new_unwrap("1.2.840.113549.1.7.3");
pub const AUTH_ENVELOPED_DATA: Oid = Oid::new_unwrap("1.2.840.113549.1.9.16.1.23");
pub const DATA: Oid = Oid::new_unwrap("1.2.840.113549.1.7.1");
pub const ATTR_CONTENT_TYPE: Oid = Oid::new_unwrap("1.2.840.113549.1.9.3");
pub const ATTR_MESSAGE_DIGEST: Oid = Oid::new_unwrap("1.2.840.113549.1.9.4");
pub const ATTR_SIGNING_TIME: Oid = Oid::new_unwrap("1.2.840.113549.1.9.5");
pub const ATTR_SMIME_CAPABILITIES: Oid = Oid::new_unwrap("1.2.840.113549.1.9.15");
/// Электронная почта в имени (устаревшее, RFC 5280 §4.1.2.6).
pub const EMAIL_ADDRESS: Oid = Oid::new_unwrap("1.2.840.113549.1.9.1");
/// EKU «Защита электронной почты».
pub const EKU_EMAIL_PROTECTION: Oid = Oid::new_unwrap("1.3.6.1.5.5.7.3.4");

// --- хэши ---
pub const SHA1: Oid = Oid::new_unwrap("1.3.14.3.2.26");
pub const SHA256: Oid = Oid::new_unwrap("2.16.840.1.101.3.4.2.1");
pub const SHA384: Oid = Oid::new_unwrap("2.16.840.1.101.3.4.2.2");
pub const SHA512: Oid = Oid::new_unwrap("2.16.840.1.101.3.4.2.3");
pub const MD5: Oid = Oid::new_unwrap("1.2.840.113549.2.5");
/// ГОСТ Р 34.11-2012 «Стрибог», 256 и 512 бит.
pub const STREEBOG256: Oid = Oid::new_unwrap("1.2.643.7.1.1.2.2");
pub const STREEBOG512: Oid = Oid::new_unwrap("1.2.643.7.1.1.2.3");
/// ГОСТ Р 34.11-94 (устаревший, не поддерживается).
pub const GOST94: Oid = Oid::new_unwrap("1.2.643.2.2.9");

// --- RSA ---
pub const RSA_ENCRYPTION: Oid = Oid::new_unwrap("1.2.840.113549.1.1.1");
pub const MD5_RSA: Oid = Oid::new_unwrap("1.2.840.113549.1.1.4");
pub const SHA1_RSA: Oid = Oid::new_unwrap("1.2.840.113549.1.1.5");
pub const RSA_PSS: Oid = Oid::new_unwrap("1.2.840.113549.1.1.10");
pub const MGF1: Oid = Oid::new_unwrap("1.2.840.113549.1.1.8");
pub const SHA256_RSA: Oid = Oid::new_unwrap("1.2.840.113549.1.1.11");
pub const SHA384_RSA: Oid = Oid::new_unwrap("1.2.840.113549.1.1.12");
pub const SHA512_RSA: Oid = Oid::new_unwrap("1.2.840.113549.1.1.13");

// --- ECDSA ---
pub const EC_PUBLIC_KEY: Oid = Oid::new_unwrap("1.2.840.10045.2.1");
pub const ECDSA_SHA1: Oid = Oid::new_unwrap("1.2.840.10045.4.1");
pub const ECDSA_SHA256: Oid = Oid::new_unwrap("1.2.840.10045.4.3.2");
pub const ECDSA_SHA384: Oid = Oid::new_unwrap("1.2.840.10045.4.3.3");
pub const ECDSA_SHA512: Oid = Oid::new_unwrap("1.2.840.10045.4.3.4");
pub const P256: Oid = Oid::new_unwrap("1.2.840.10045.3.1.7");
pub const P384: Oid = Oid::new_unwrap("1.3.132.0.34");

// --- ГОСТ Р 34.10-2012 (RFC 9215) ---
pub const GOST_KEY_256: Oid = Oid::new_unwrap("1.2.643.7.1.1.1.1");
pub const GOST_KEY_512: Oid = Oid::new_unwrap("1.2.643.7.1.1.1.2");
pub const GOST_SIGN_256: Oid = Oid::new_unwrap("1.2.643.7.1.1.3.2");
pub const GOST_SIGN_512: Oid = Oid::new_unwrap("1.2.643.7.1.1.3.3");
/// ГОСТ Р 34.10-2001 (устаревший, не поддерживается).
pub const GOST2001_KEY: Oid = Oid::new_unwrap("1.2.643.2.2.19");
pub const GOST2001_SIGN: Oid = Oid::new_unwrap("1.2.643.2.2.3");

// Наборы параметров (RFC 7836, RFC 4357, RFC 9215 приложение C).
pub const GOST_TEST_256: Oid = Oid::new_unwrap("1.2.643.2.2.35.0");
pub const CRYPTOPRO_A: Oid = Oid::new_unwrap("1.2.643.2.2.35.1");
pub const CRYPTOPRO_B: Oid = Oid::new_unwrap("1.2.643.2.2.35.2");
pub const CRYPTOPRO_C: Oid = Oid::new_unwrap("1.2.643.2.2.35.3");
pub const CRYPTOPRO_XCH_A: Oid = Oid::new_unwrap("1.2.643.2.2.36.0");
pub const CRYPTOPRO_XCH_B: Oid = Oid::new_unwrap("1.2.643.2.2.36.1");
pub const TC26_256_A: Oid = Oid::new_unwrap("1.2.643.7.1.2.1.1.1");
pub const TC26_256_B: Oid = Oid::new_unwrap("1.2.643.7.1.2.1.1.2");
pub const TC26_256_C: Oid = Oid::new_unwrap("1.2.643.7.1.2.1.1.3");
pub const TC26_256_D: Oid = Oid::new_unwrap("1.2.643.7.1.2.1.1.4");
pub const TC26_512_TEST: Oid = Oid::new_unwrap("1.2.643.7.1.2.1.2.0");
pub const TC26_512_A: Oid = Oid::new_unwrap("1.2.643.7.1.2.1.2.1");
pub const TC26_512_B: Oid = Oid::new_unwrap("1.2.643.7.1.2.1.2.2");
pub const TC26_512_C: Oid = Oid::new_unwrap("1.2.643.7.1.2.1.2.3");

// --- шифры в SMIMECapabilities (для сводки; шифрование — Ф4) ---
pub const AES128_CBC: Oid = Oid::new_unwrap("2.16.840.1.101.3.4.1.2");
pub const AES192_CBC: Oid = Oid::new_unwrap("2.16.840.1.101.3.4.1.22");
pub const AES256_CBC: Oid = Oid::new_unwrap("2.16.840.1.101.3.4.1.42");
pub const AES128_GCM: Oid = Oid::new_unwrap("2.16.840.1.101.3.4.1.6");
pub const AES256_GCM: Oid = Oid::new_unwrap("2.16.840.1.101.3.4.1.46");
pub const DES_EDE3_CBC: Oid = Oid::new_unwrap("1.2.840.113549.3.7");
pub const RSA_OAEP: Oid = Oid::new_unwrap("1.2.840.113549.1.1.7");
/// ГОСТ Р 34.12-2015 «Кузнечик» в CMS (рекомендации ТК 26, режим CTR-ACPKM, RFC 9189) [проверить номера документов
/// ТК 26].
pub const KUZNYECHIK_CTR_ACPKM: Oid = Oid::new_unwrap("1.2.643.7.1.1.5.2.1");
pub const KUZNYECHIK_CTR_ACPKM_OMAC: Oid = Oid::new_unwrap("1.2.643.7.1.1.5.2.2");

/// Имя шифра из `SMIMECapabilities` для карточки; неизвестный — `None` (показывается OID).
pub fn capability_name(oid: &str) -> Option<&'static str> {
    let oid = Oid::new(oid).ok()?;
    Some(match oid {
        AES128_CBC => "AES-128-CBC",
        AES192_CBC => "AES-192-CBC",
        AES256_CBC => "AES-256-CBC",
        AES128_GCM => "AES-128-GCM",
        AES256_GCM => "AES-256-GCM",
        DES_EDE3_CBC => "3DES",
        RSA_OAEP => "RSA-OAEP",
        KUZNYECHIK_CTR_ACPKM | KUZNYECHIK_CTR_ACPKM_OMAC => "Kuznyechik",
        _ => return None,
    })
}
