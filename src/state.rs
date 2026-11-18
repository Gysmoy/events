// Almacenar conexiones con sus filtros organizadas por servicio.
// Mapas con orden de inserción, igual que los Map de JS.

use std::sync::{Mutex, MutexGuard, PoisonError};

use indexmap::IndexMap;
use serde_json::{Value, json};
use socketioxide::socket::Sid;
use time::OffsetDateTime;

use crate::filters::{Filters, filters_match};

#[derive(Debug, Clone)]
pub struct ClientData {
    pub filters: Filters,
    // Ya formateado como Date#toJSON: "2026-11-18T10:00:00.000Z"
    pub connected_at: String,
}

type ServiceClients = IndexMap<Sid, ClientData>;

// service -> (socketId -> clientData)
#[derive(Debug, Default)]
pub struct Registry {
    services: Mutex<IndexMap<String, ServiceClients>>,
}

impl Registry {
    // Nunca se hace await con el candado tomado, así que un panic no deja datos a medias:
    // si el mutex quedó envenenado se sigue usando igual.
    fn lock(&self) -> MutexGuard<'_, IndexMap<String, ServiceClients>> {
        self.services.lock().unwrap_or_else(PoisonError::into_inner)
    }

    // Inicializar el servicio si no existe
    pub fn ensure_service(&self, service: &str) {
        self.lock().entry(service.to_string()).or_default();
    }

    // Registra o reemplaza los filtros del socket. Si el servicio se borró porque se
    // fue el último cliente registrado, se vuelve a crear (en Node esto tumbaba el proceso).
    pub fn register(&self, service: &str, sid: Sid, filters: Filters) {
        let client = ClientData {
            filters,
            connected_at: iso_now(),
        };
        self.lock().entry(service.to_string()).or_default().insert(sid, client);
    }

    // Reemplaza los filtros si el socket está registrado; conserva connectedAt
    pub fn update(&self, service: &str, sid: Sid, filters: Filters) -> bool {
        let mut services = self.lock();
        match services.get_mut(service).and_then(|clients| clients.get_mut(&sid)) {
            Some(client) => {
                client.filters = filters;
                true
            }
            None => false,
        }
    }

    pub fn filters_of(&self, service: &str, sid: Sid) -> Option<Filters> {
        let services = self.lock();
        services
            .get(service)
            .and_then(|clients| clients.get(&sid))
            .map(|client| client.filters.clone())
    }

    // Al desconectarse: si no quedan clientes en el servicio, limpiar el servicio
    pub fn remove(&self, service: &str, sid: Sid) {
        let mut services = self.lock();
        if let Some(clients) = services.get_mut(service) {
            clients.shift_remove(&sid);
            if clients.is_empty() {
                services.shift_remove(service);
            }
        }
    }

    // Ids de los clientes del servicio cuyos filtros coinciden, en orden de registro
    pub fn matching_ids(&self, service: &str, filters: &Filters) -> Vec<Sid> {
        let services = self.lock();
        services
            .get(service)
            .map(|clients| {
                clients
                    .iter()
                    .filter(|(_, client)| filters_match(&client.filters, filters))
                    .map(|(sid, _)| *sid)
                    .collect()
            })
            .unwrap_or_default()
    }

    // [{ socketId, filters }] de los clientes que coinciden
    pub fn matching_clients(&self, service: &str, filters: &Filters) -> Vec<Value> {
        let services = self.lock();
        services
            .get(service)
            .map(|clients| {
                clients
                    .iter()
                    .filter(|(_, client)| filters_match(&client.filters, filters))
                    .map(|(sid, client)| json!({ "socketId": sid.to_string(), "filters": client.filters }))
                    .collect()
            })
            .unwrap_or_default()
    }

    // Respuesta de GET /stats
    pub fn stats(&self) -> Value {
        let services = self.lock();
        let mut total_clients = 0;
        let mut by_service = serde_json::Map::new();
        for (service, clients) in services.iter() {
            total_clients += clients.len();
            let list: Vec<Value> = clients
                .iter()
                .map(|(sid, client)| {
                    json!({
                        "socketId": sid.to_string(),
                        "filters": client.filters,
                        "connectedAt": client.connected_at,
                    })
                })
                .collect();
            by_service.insert(
                service.clone(),
                json!({ "connectedClients": clients.len(), "clients": list }),
            );
        }
        json!({
            "totalServices": services.len(),
            "totalClients": total_clients,
            "services": by_service,
        })
    }
}

// new Date().toJSON()
pub fn iso_now() -> String {
    iso_string(OffsetDateTime::now_utc())
}

fn iso_string(t: OffsetDateTime) -> String {
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        t.year(),
        u8::from(t.month()),
        t.day(),
        t.hour(),
        t.minute(),
        t.second(),
        t.millisecond()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filters(value: Value) -> Filters {
        match value {
            Value::Object(map) => map,
            _ => panic!("se esperaba un objeto"),
        }
    }

    #[test]
    fn formato_iso_como_js() {
        let t = OffsetDateTime::from_unix_timestamp_nanos(1_767_323_045_006_000_000).unwrap();
        assert_eq!(iso_string(t), "2026-01-02T03:04:05.006Z");
        let now = iso_now();
        assert_eq!(now.len(), 24);
        assert!(now.ends_with('Z'));
    }

    #[test]
    fn el_servicio_existe_antes_de_registrar_y_se_borra_con_el_ultimo() {
        let registry = Registry::default();
        registry.ensure_service("devex");
        assert_eq!(registry.stats()["totalServices"], 1);
        assert_eq!(registry.stats()["services"]["devex"]["connectedClients"], 0);

        let (a, b) = (Sid::new(), Sid::new());
        registry.register("devex", a, filters(json!({"service": "devex"})));
        registry.register("devex", b, filters(json!({"service": "devex"})));
        assert_eq!(registry.stats()["totalClients"], 2);

        registry.remove("devex", a);
        assert_eq!(registry.stats()["totalClients"], 1);
        registry.remove("devex", b);
        assert_eq!(
            registry.stats(),
            json!({"totalServices": 0, "totalClients": 0, "services": {}})
        );
    }

    #[test]
    fn desconectar_sin_registrar_tambien_limpia_el_servicio_vacio() {
        let registry = Registry::default();
        registry.ensure_service("devex");
        registry.remove("devex", Sid::new());
        assert_eq!(registry.stats()["totalServices"], 0);
    }

    #[test]
    fn registrar_tras_borrar_el_servicio_lo_vuelve_a_crear() {
        let registry = Registry::default();
        let sid = Sid::new();
        registry.register("devex", sid, filters(json!({"a": 1})));
        assert_eq!(registry.filters_of("devex", sid), Some(filters(json!({"a": 1}))));
    }

    #[test]
    fn volver_a_registrar_conserva_la_posicion() {
        let registry = Registry::default();
        let (a, b) = (Sid::new(), Sid::new());
        registry.register("devex", a, filters(json!({"x": 1})));
        registry.register("devex", b, filters(json!({"x": 1})));
        registry.register("devex", a, filters(json!({"x": 1})));
        assert_eq!(registry.matching_ids("devex", &filters(json!({"x": 1}))), [a, b]);
    }

    #[test]
    fn actualizar_solo_si_esta_registrado_y_conserva_connected_at() {
        let registry = Registry::default();
        let sid = Sid::new();
        assert!(!registry.update("devex", sid, filters(json!({"x": 2}))));
        assert_eq!(registry.filters_of("devex", sid), None);

        registry.register("devex", sid, filters(json!({"x": 1})));
        let before = registry.stats()["services"]["devex"]["clients"][0]["connectedAt"].clone();
        assert!(registry.update("devex", sid, filters(json!({"x": 2}))));
        assert_eq!(registry.filters_of("devex", sid), Some(filters(json!({"x": 2}))));
        assert_eq!(
            registry.stats()["services"]["devex"]["clients"][0]["connectedAt"],
            before
        );
    }

    #[test]
    fn coincidencias_por_servicio() {
        let registry = Registry::default();
        let (a, b, c) = (Sid::new(), Sid::new(), Sid::new());
        registry.register("devex", a, filters(json!({"service": "devex", "env": "production"})));
        registry.register("devex", b, filters(json!({"service": "devex", "env": "local"})));
        registry.register("otro", c, filters(json!({"service": "otro", "env": "production"})));

        let wanted = filters(json!({"service": "devex", "env": "production"}));
        assert_eq!(registry.matching_ids("devex", &wanted), [a]);
        assert_eq!(registry.matching_ids("nadie", &wanted), []);
        assert_eq!(
            registry.matching_clients("devex", &wanted),
            [json!({"socketId": a.to_string(), "filters": {"service": "devex", "env": "production"}})]
        );
    }
}
