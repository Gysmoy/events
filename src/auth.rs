// Autenticación: secreto de los backends y token firmado de las conexiones.

use std::time::{SystemTime, UNIX_EPOCH};

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;

// Comparación en tiempo constante: primero el largo, luego los bytes
pub fn safe_equal(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && bool::from(a.ct_eq(b))
}

// hex(hmac_sha256(secreto, "servicio.expira"))
pub fn sign(secret: &str, service: &str, expires: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC acepta llaves de cualquier tamaño");
    mac.update(service.as_bytes());
    mac.update(b".");
    mac.update(expires.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

// Token de conexión: "<expira en segundos unix>.<hmac sha256 de 'servicio.expira'>".
// Lo firma el backend del servicio para sus usuarios con sesión.
// Como en JS, las partes después del segundo punto se ignoran.
pub fn is_valid_socket_token(secret: &str, service: &str, token: &str, now_seconds: f64) -> bool {
    let mut parts = token.split('.');
    let expires = parts.next().unwrap_or_default();
    let Some(signature) = parts.next() else {
        return false;
    };

    if expires.is_empty() || !expires.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    // Number(expires) en JS: con muchísimos dígitos da Infinity, que nunca vence
    let expires_number: f64 = expires.parse().unwrap_or(f64::INFINITY);
    if expires_number < now_seconds {
        return false;
    }

    safe_equal(signature.as_bytes(), sign(secret, service, expires).as_bytes())
}

// Date.now() / 1000: segundos con precisión de milisegundos
pub fn now_seconds() -> f64 {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or_default();
    millis as f64 / 1000.0
}

// Authorization sin el "Bearer " inicial (solo ese prefijo exacto, como el replace de JS)
pub fn bearer_token(authorization: Option<&[u8]>) -> &[u8] {
    let raw = authorization.unwrap_or_default();
    raw.strip_prefix(b"Bearer ").unwrap_or(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "parity-secret";
    const NOW: f64 = 1_800_000_000.5;

    fn token(service: &str, expires: &str) -> String {
        format!("{expires}.{}", sign(SECRET, service, expires))
    }

    #[test]
    fn firma_conocida() {
        // Mismo valor que crypto.createHmac en Node y hash_hmac('sha256', ...) en PHP
        assert_eq!(
            sign(SECRET, "devex", "1800000300"),
            "a1b5ac3b93e08711c24cf56b9e27a08303b5bef00b8a03ed49ab842236209e83"
        );
    }

    #[test]
    fn token_valido() {
        assert!(is_valid_socket_token(
            SECRET,
            "devex",
            &token("devex", "1800000300"),
            NOW
        ));
    }

    #[test]
    fn token_que_vence_justo_ahora_sigue_valido_hasta_el_segundo() {
        // 1800000000 < 1800000000.5 -> vencido; 1800000001 >= ahora -> válido
        assert!(!is_valid_socket_token(
            SECRET,
            "devex",
            &token("devex", "1800000000"),
            NOW
        ));
        assert!(is_valid_socket_token(
            SECRET,
            "devex",
            &token("devex", "1800000001"),
            NOW
        ));
    }

    #[test]
    fn token_vencido() {
        assert!(!is_valid_socket_token(
            SECRET,
            "devex",
            &token("devex", "1700000000"),
            NOW
        ));
    }

    #[test]
    fn firma_mala_o_hex_en_mayusculas() {
        let good = token("devex", "1800000300");
        let upper = good.to_uppercase();
        assert!(!is_valid_socket_token(SECRET, "devex", &upper, NOW));
        assert!(!is_valid_socket_token(SECRET, "devex", "1800000300.zz", NOW));
        assert!(!is_valid_socket_token(SECRET, "devex", "1800000300.", NOW));
        // Último dígito hex cambiado: mismo largo, distinta firma
        let mut tampered = good.clone();
        let last = tampered.pop().unwrap();
        tampered.push(if last == '0' { '1' } else { '0' });
        assert!(!is_valid_socket_token(SECRET, "devex", &tampered, NOW));
    }

    #[test]
    fn token_de_otro_servicio() {
        assert!(!is_valid_socket_token(
            SECRET,
            "devex",
            &token("otro", "1800000300"),
            NOW
        ));
        assert!(!is_valid_socket_token(
            "otro-secreto",
            "devex",
            &token("devex", "1800000300"),
            NOW
        ));
    }

    #[test]
    fn partes_extra_se_ignoran() {
        let extra = format!("{}.extra.mas", token("devex", "1800000300"));
        assert!(is_valid_socket_token(SECRET, "devex", &extra, NOW));
    }

    #[test]
    fn expira_debe_ser_solo_digitos() {
        for expires in ["", "-1800000300", "1800000300.5", "18e9", " 1800000300", "١٢٣"] {
            let tok = format!("{expires}.{}", sign(SECRET, "devex", expires));
            assert!(!is_valid_socket_token(SECRET, "devex", &tok, NOW), "{expires:?}");
        }
        assert!(!is_valid_socket_token(SECRET, "devex", "", NOW));
        assert!(!is_valid_socket_token(SECRET, "devex", "1800000300", NOW));
    }

    #[test]
    fn expira_enorme_nunca_vence() {
        let huge = "9".repeat(400);
        assert!(is_valid_socket_token(SECRET, "devex", &token("devex", &huge), NOW));
        assert!(is_valid_socket_token(
            SECRET,
            "devex",
            &token("devex", "00001800000300"),
            NOW
        ));
    }

    #[test]
    fn comparacion_segura() {
        assert!(safe_equal(b"abc", b"abc"));
        assert!(!safe_equal(b"abc", b"abd"));
        assert!(!safe_equal(b"abc", b"abcd"));
        assert!(safe_equal(b"", b""));
    }

    #[test]
    fn bearer_solo_quita_el_prefijo_exacto() {
        assert_eq!(bearer_token(Some(b"Bearer s3cr3t")), b"s3cr3t");
        assert_eq!(bearer_token(Some(b"bearer s3cr3t")), b"bearer s3cr3t");
        assert_eq!(bearer_token(Some(b"s3cr3t")), b"s3cr3t");
        assert_eq!(bearer_token(Some(b"Bearer Bearer x")), b"Bearer x");
        assert_eq!(bearer_token(None), b"");
    }
}
