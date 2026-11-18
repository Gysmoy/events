// Filtros con la misma semántica que el servidor original en JS.

use serde_json::{Map, Value};

pub type Filters = Map<String, Value>;

// typeof x === "object" && x !== null: objetos y arreglos
pub fn is_js_object(value: &Value) -> bool {
    matches!(value, Value::Object(_) | Value::Array(_))
}

// Valores falsy de JS que pueden venir en JSON: null, false, 0, -0 y ""
pub fn is_js_falsy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => true,
        Some(Value::Bool(b)) => !b,
        Some(Value::Number(n)) => n.as_f64().is_none_or(|f| f == 0.0 || f.is_nan()),
        Some(Value::String(s)) => s.is_empty(),
        Some(Value::Array(_) | Value::Object(_)) => false,
    }
}

// Los filtros incluyen automáticamente el servicio: { service, ...filters }.
// Igual que el spread de JS, un "service" dentro de filters lo reemplaza y un
// arreglo aporta sus índices "0", "1", ... como claves.
pub fn complete_filters(service: &str, filters: &Value) -> Filters {
    let mut complete = Filters::new();
    complete.insert("service".into(), Value::String(service.into()));
    match filters {
        Value::Object(map) => {
            for (key, value) in map {
                complete.insert(key.clone(), value.clone());
            }
        }
        Value::Array(items) => {
            for (index, value) in items.iter().enumerate() {
                complete.insert(index.to_string(), value.clone());
            }
        }
        _ => {}
    }
    js_key_order(complete)
}

// Un objeto de JS enumera primero las claves que son índices de arreglo (en orden
// numérico) y después las demás en orden de inserción.
fn js_key_order(map: Filters) -> Filters {
    if !map.keys().any(|key| array_index(key).is_some()) {
        return map;
    }
    let (mut indexes, others): (Vec<_>, Vec<_>) = map.into_iter().partition(|(key, _)| array_index(key).is_some());
    indexes.sort_by_key(|(key, _)| array_index(key));
    indexes.into_iter().chain(others).collect()
}

// "0", "1", ..., "4294967294": forma canónica, sin ceros a la izquierda
fn array_index(key: &str) -> Option<u32> {
    if key.is_empty() || (key.len() > 1 && key.starts_with('0')) || !key.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    key.parse::<u32>().ok().filter(|index| *index < u32::MAX)
}

// Todos los filtros de la notificación deben coincidir con los del cliente
pub fn filters_match(client: &Filters, notification: &Filters) -> bool {
    notification
        .iter()
        .all(|(key, value)| client.get(key).is_some_and(|own| strict_equal(own, value)))
}

// === de JS: los números se comparan por valor (1 === 1.0) y objetos o arreglos
// nunca son iguales porque JS compara referencias
fn strict_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Null, Value::Null) => true,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Number(a), Value::Number(b)) => a.as_f64() == b.as_f64(),
        (Value::String(a), Value::String(b)) => a == b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn filters(value: Value) -> Filters {
        match value {
            Value::Object(map) => map,
            _ => panic!("se esperaba un objeto"),
        }
    }

    fn keys(map: &Filters) -> Vec<&str> {
        map.keys().map(String::as_str).collect()
    }

    #[test]
    fn objetos_validos() {
        assert!(is_js_object(&json!({})));
        assert!(is_js_object(&json!([])));
        for value in [json!(null), json!("x"), json!(1), json!(true), json!(0)] {
            assert!(!is_js_object(&value), "{value}");
        }
    }

    #[test]
    fn falsy_de_js() {
        assert!(is_js_falsy(None));
        for value in [json!(null), json!(false), json!(0), json!(-0.0), json!(0.0), json!("")] {
            assert!(is_js_falsy(Some(&value)), "{value}");
        }
        for value in [
            json!(true),
            json!(1),
            json!(-1),
            json!("0"),
            json!(" "),
            json!([]),
            json!({}),
            json!(0.1),
        ] {
            assert!(!is_js_falsy(Some(&value)), "{value}");
        }
    }

    #[test]
    fn merge_agrega_el_servicio_primero() {
        let complete = complete_filters("devex", &json!({"environment": "production", "user_id": 7}));
        assert_eq!(
            Value::Object(complete.clone()),
            json!({"service": "devex", "environment": "production", "user_id": 7})
        );
        assert_eq!(keys(&complete), ["service", "environment", "user_id"]);
    }

    #[test]
    fn service_dentro_de_filtros_lo_reemplaza_en_su_lugar() {
        let complete = complete_filters("devex", &json!({"a": 1, "service": "hack"}));
        assert_eq!(keys(&complete), ["service", "a"]);
        assert_eq!(complete["service"], json!("hack"));
    }

    #[test]
    fn arreglo_aporta_indices_como_claves() {
        let complete = complete_filters("devex", &json!(["a", {"b": 1}]));
        assert_eq!(keys(&complete), ["0", "1", "service"]);
        assert_eq!(complete["0"], json!("a"));
        assert_eq!(complete["1"], json!({"b": 1}));
    }

    #[test]
    fn claves_numericas_van_primero_como_en_js() {
        let complete = complete_filters("devex", &json!({"b": 1, "10": "x", "2": "y", "01": "z", "-1": "w"}));
        assert_eq!(keys(&complete), ["2", "10", "service", "b", "01", "-1"]);
    }

    #[test]
    fn coincide_si_todos_los_filtros_de_la_notificacion_estan() {
        let client = filters(json!({"service": "devex", "business_id": 1, "user_id": 42, "role": "admin"}));
        assert!(filters_match(
            &client,
            &filters(json!({"service": "devex", "business_id": 1}))
        ));
        assert!(filters_match(
            &client,
            &filters(json!({"business_id": 1, "user_id": 42, "role": "admin"}))
        ));
        assert!(filters_match(&client, &filters(json!({}))));
        assert!(!filters_match(&client, &filters(json!({"business_id": 2}))));
        assert!(!filters_match(
            &client,
            &filters(json!({"business_id": 1, "store_id": 5}))
        ));
        assert!(!filters_match(&client, &filters(json!({"business_id": "1"}))));
    }

    #[test]
    fn numeros_se_comparan_por_valor() {
        let client = filters(json!({"n": 1, "f": 2.5, "neg": -3}));
        assert!(filters_match(&client, &filters(json!({"n": 1.0}))));
        assert!(filters_match(&client, &filters(json!({"f": 2.5, "neg": -3.0}))));
        assert!(!filters_match(&client, &filters(json!({"n": 1.5}))));
        let client = filters(json!({"n": 1.0}));
        assert!(filters_match(&client, &filters(json!({"n": 1}))));
    }

    #[test]
    fn objetos_y_arreglos_nunca_coinciden() {
        let client = filters(json!({"obj": {"a": 1}, "arr": [1], "empty": {}}));
        assert!(!filters_match(&client, &filters(json!({"obj": {"a": 1}}))));
        assert!(!filters_match(&client, &filters(json!({"arr": [1]}))));
        assert!(!filters_match(&client, &filters(json!({"empty": {}}))));
    }

    #[test]
    fn null_y_booleanos() {
        let client = filters(json!({"x": null, "b": false}));
        assert!(filters_match(&client, &filters(json!({"x": null}))));
        assert!(filters_match(&client, &filters(json!({"b": false}))));
        assert!(!filters_match(&client, &filters(json!({"b": 0}))));
        assert!(!filters_match(&client, &filters(json!({"x": false}))));
        // Una clave que el cliente no tiene nunca coincide, ni siquiera con null
        assert!(!filters_match(&client, &filters(json!({"missing": null}))));
    }
}
