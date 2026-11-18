// Configuración desde variables de entorno. Sin secreto el servidor no arranca.

pub const MISSING_SECRET: &str = "Falta EVENTS_SECRET: el servidor no arranca sin autenticación";

const DEFAULT_SERVICES: &str = "devex";
const DEFAULT_ORIGINS: &str = "https://devex.pe";
const DEFAULT_PORT: u16 = 3000;

#[derive(Debug, Clone)]
pub struct Config {
    // Secreto compartido con los backends: firma los tokens y autoriza /emit, /stats y /clients/filter
    pub secret: String,
    // Solo los servicios de DevEx usan este servidor
    pub allowed_services: Vec<String>,
    // Un navegador siempre envía Origin: lo que no venga de un dominio permitido no pasa
    pub allowed_origins: Vec<String>,
    pub port: u16,
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    // Igual que `process.env.X || "default"`: una variable vacía cuenta como ausente
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let var = |name: &str| get(name).filter(|value| !value.is_empty());

        let secret = var("EVENTS_SECRET").ok_or_else(|| MISSING_SECRET.to_string())?;
        let allowed_services = parse_list(var("ALLOWED_SERVICES").as_deref().unwrap_or(DEFAULT_SERVICES));
        let allowed_origins = parse_list(var("ALLOWED_ORIGINS").as_deref().unwrap_or(DEFAULT_ORIGINS));
        let port = match var("PORT") {
            None => DEFAULT_PORT,
            Some(raw) => raw.trim().parse().map_err(|_| format!("PORT inválido: {raw}"))?,
        };

        Ok(Self {
            secret,
            allowed_services,
            allowed_origins,
            port,
        })
    }

    pub fn allows_service(&self, service: &str) -> bool {
        self.allowed_services.iter().any(|allowed| allowed == service)
    }

    pub fn allows_origin(&self, origin: &str) -> bool {
        self.allowed_origins.iter().any(|allowed| allowed == origin)
    }
}

// "a, b,,c" -> ["a", "b", "c"]
fn parse_list(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(String::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn load(vars: &[(&str, &str)]) -> Result<Config, String> {
        let vars: HashMap<String, String> = vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        Config::from_lookup(|name| vars.get(name).cloned())
    }

    #[test]
    fn sin_secreto_no_arranca() {
        assert_eq!(load(&[]).unwrap_err(), MISSING_SECRET);
        assert_eq!(load(&[("EVENTS_SECRET", "")]).unwrap_err(), MISSING_SECRET);
    }

    #[test]
    fn valores_por_defecto() {
        let config = load(&[("EVENTS_SECRET", "s")]).unwrap();
        assert_eq!(config.secret, "s");
        assert_eq!(config.allowed_services, ["devex"]);
        assert_eq!(config.allowed_origins, ["https://devex.pe"]);
        assert_eq!(config.port, 3000);
    }

    #[test]
    fn listas_se_separan_por_coma_y_se_limpian() {
        let config = load(&[
            ("EVENTS_SECRET", "s"),
            ("ALLOWED_SERVICES", " devex , otro,, "),
            ("ALLOWED_ORIGINS", "https://a.pe,https://b.pe"),
            ("PORT", "8080"),
        ])
        .unwrap();
        assert_eq!(config.allowed_services, ["devex", "otro"]);
        assert_eq!(config.allowed_origins, ["https://a.pe", "https://b.pe"]);
        assert_eq!(config.port, 8080);
        assert!(config.allows_service("otro"));
        assert!(!config.allows_service("Devex"));
        assert!(config.allows_origin("https://b.pe"));
        assert!(!config.allows_origin("https://b.pe/"));
    }

    #[test]
    fn variables_vacias_usan_el_valor_por_defecto() {
        let config = load(&[("EVENTS_SECRET", "s"), ("ALLOWED_SERVICES", ""), ("PORT", "")]).unwrap();
        assert_eq!(config.allowed_services, ["devex"]);
        assert_eq!(config.port, 3000);
    }

    #[test]
    fn puerto_invalido_es_error() {
        assert!(load(&[("EVENTS_SECRET", "s"), ("PORT", "abc")]).is_err());
    }
}
