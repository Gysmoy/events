// Lectura del cuerpo igual que express.json() (body-parser 1.20, modo strict):
// solo se parsea si el Content-Type es application/json; si no, el cuerpo es {}.

use axum::body::Body;
use axum::http::{HeaderMap, StatusCode, header};
use serde_json::{Value, json};

// "100kb" de body-parser
pub const BODY_LIMIT: usize = 100 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BodyError {
    // JSON mal formado o que no empieza con { o [
    Malformed,
    // Más de 100kb
    TooLarge,
    // charset que no es utf-*
    UnsupportedCharset,
    // Content-Encoding comprimido: no se descomprime
    UnsupportedEncoding,
}

impl BodyError {
    pub fn status(&self) -> StatusCode {
        match self {
            BodyError::Malformed => StatusCode::BAD_REQUEST,
            BodyError::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            BodyError::UnsupportedCharset | BodyError::UnsupportedEncoding => StatusCode::UNSUPPORTED_MEDIA_TYPE,
        }
    }

    pub fn body(&self) -> Value {
        let message = match self {
            BodyError::Malformed => "JSON inválido",
            BodyError::TooLarge => "El cuerpo supera el límite de 100kb",
            BodyError::UnsupportedCharset => "Charset no soportado",
            BodyError::UnsupportedEncoding => "Content-Encoding no soportado",
        };
        json!({ "error": message })
    }
}

// Qué hacer con el cuerpo según las cabeceras
#[derive(Debug, PartialEq, Eq)]
enum Plan {
    // No hay cuerpo o no es JSON: queda {}
    Empty,
    Parse,
}

fn plan(headers: &HeaderMap) -> Result<Plan, BodyError> {
    // typeis.hasBody: hay Transfer-Encoding o Content-Length
    let has_body = headers.contains_key(header::TRANSFER_ENCODING) || headers.contains_key(header::CONTENT_LENGTH);
    if !has_body {
        return Ok(Plan::Empty);
    }

    let Some(content_type) = headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()) else {
        return Ok(Plan::Empty);
    };
    let mut parts = content_type.split(';');
    let mime = parts.next().unwrap_or_default().trim();
    if !mime.eq_ignore_ascii_case("application/json") {
        return Ok(Plan::Empty);
    }

    // Charset por RFC 7159: solo utf-*
    let charset = parts
        .filter_map(|param| param.split_once('='))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("charset"))
        .map(|(_, value)| value.trim().trim_matches('"').to_ascii_lowercase());
    if let Some(charset) = charset
        && !charset.starts_with("utf-")
    {
        return Err(BodyError::UnsupportedCharset);
    }

    let encoding = headers
        .get(header::CONTENT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("identity");
    if !encoding.trim().eq_ignore_ascii_case("identity") {
        return Err(BodyError::UnsupportedEncoding);
    }

    // Si el Content-Length ya pasa el límite no se lee nada
    let declared = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok());
    if declared.is_some_and(|length| length > BODY_LIMIT as u64) {
        return Err(BodyError::TooLarge);
    }

    Ok(Plan::Parse)
}

// Parseo estricto: vacío -> {}, debe empezar con { o [, JSON válido
pub fn parse_json(bytes: &[u8]) -> Result<Value, BodyError> {
    let text = String::from_utf8_lossy(bytes);
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    if text.is_empty() {
        return Ok(json!({}));
    }
    let first = text.trim_start_matches([' ', '\t', '\n', '\r']).chars().next();
    if !matches!(first, Some('{' | '[')) {
        return Err(BodyError::Malformed);
    }
    serde_json::from_str(text).map_err(|_| BodyError::Malformed)
}

pub async fn read_json(headers: &HeaderMap, body: Body) -> Result<Value, BodyError> {
    match plan(headers)? {
        Plan::Empty => Ok(json!({})),
        Plan::Parse => {
            // Solo falla al pasar el límite (cuerpo chunked) o si el cliente cortó la conexión,
            // y en ese caso ya nadie lee la respuesta
            let bytes = axum::body::to_bytes(body, BODY_LIMIT)
                .await
                .map_err(|_| BodyError::TooLarge)?;
            parse_json(&bytes)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.append(*name, HeaderValue::from_static(value));
        }
        map
    }

    #[test]
    fn sin_cuerpo_o_sin_json_queda_vacio() {
        assert_eq!(plan(&headers(&[])), Ok(Plan::Empty));
        assert_eq!(plan(&headers(&[("content-type", "application/json")])), Ok(Plan::Empty));
        assert_eq!(plan(&headers(&[("content-length", "5")])), Ok(Plan::Empty));
        assert_eq!(
            plan(&headers(&[("content-length", "5"), ("content-type", "text/plain")])),
            Ok(Plan::Empty)
        );
        assert_eq!(
            plan(&headers(&[
                ("content-length", "5"),
                ("content-type", "application/x-www-form-urlencoded")
            ])),
            Ok(Plan::Empty)
        );
        assert_eq!(
            plan(&headers(&[
                ("content-length", "5"),
                ("content-type", "application/vnd.api+json")
            ])),
            Ok(Plan::Empty)
        );
    }

    #[test]
    fn json_con_parametros_se_parsea() {
        for content_type in [
            "application/json",
            "Application/JSON",
            "application/json; charset=utf-8",
            "application/json;charset=\"UTF-8\"",
        ] {
            let map = {
                let mut map = headers(&[("content-length", "2")]);
                map.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
                map
            };
            assert_eq!(plan(&map), Ok(Plan::Parse), "{content_type}");
        }
        assert_eq!(
            plan(&headers(&[
                ("transfer-encoding", "chunked"),
                ("content-type", "application/json")
            ])),
            Ok(Plan::Parse)
        );
    }

    #[test]
    fn charset_que_no_es_utf_es_415() {
        let map = headers(&[
            ("content-length", "2"),
            ("content-type", "application/json; charset=latin1"),
        ]);
        assert_eq!(plan(&map), Err(BodyError::UnsupportedCharset));
        assert_eq!(
            BodyError::UnsupportedCharset.status(),
            StatusCode::UNSUPPORTED_MEDIA_TYPE
        );
    }

    #[test]
    fn cuerpo_comprimido_es_415() {
        let map = headers(&[
            ("content-length", "2"),
            ("content-type", "application/json"),
            ("content-encoding", "gzip"),
        ]);
        assert_eq!(plan(&map), Err(BodyError::UnsupportedEncoding));
    }

    #[test]
    fn content_length_sobre_el_limite_es_413() {
        let map = headers(&[("content-length", "102401"), ("content-type", "application/json")]);
        assert_eq!(plan(&map), Err(BodyError::TooLarge));
        assert_eq!(BodyError::TooLarge.status(), StatusCode::PAYLOAD_TOO_LARGE);
        let map = headers(&[("content-length", "102400"), ("content-type", "application/json")]);
        assert_eq!(plan(&map), Ok(Plan::Parse));
    }

    #[test]
    fn parseo_estricto() {
        assert_eq!(parse_json(b""), Ok(json!({})));
        assert_eq!(parse_json(b"{\"a\":1}"), Ok(json!({"a": 1})));
        assert_eq!(parse_json(b" \r\n\t[1,2]"), Ok(json!([1, 2])));
        assert_eq!(parse_json("\u{feff}{}".as_bytes()), Ok(json!({})));
        for bad in [
            &b"{"[..],
            b"{\"a\":}",
            b"\"texto\"",
            b"123",
            b"null",
            b"true",
            b"   ",
            b"{} x",
        ] {
            assert_eq!(
                parse_json(bad),
                Err(BodyError::Malformed),
                "{}",
                String::from_utf8_lossy(bad)
            );
        }
    }

    #[test]
    fn claves_duplicadas_como_json_parse() {
        let value = parse_json(br#"{"a":1,"b":2,"a":3}"#).unwrap();
        assert_eq!(value, json!({"a": 3, "b": 2}));
        let keys: Vec<_> = value.as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys, ["a", "b"]);
    }

    #[tokio::test]
    async fn lectura_completa() {
        let map = headers(&[("content-length", "9"), ("content-type", "application/json")]);
        assert_eq!(read_json(&map, Body::from("{\"a\":[]}")).await, Ok(json!({"a": []})));
        let big = format!("{{\"a\":\"{}\"}}", "x".repeat(BODY_LIMIT));
        let map = headers(&[("transfer-encoding", "chunked"), ("content-type", "application/json")]);
        assert_eq!(read_json(&map, Body::from(big)).await, Err(BodyError::TooLarge));
    }
}
