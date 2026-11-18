// Límites por IP: cuántos handshakes puede abrir una IP por minuto y cuántas
// conexiones puede tener abiertas a la vez.
//
// La IP real sale de X-Real-IP (o del último X-Forwarded-For) solo cuando la
// petición llega desde la red interna, es decir, desde el nginx del host. Si no
// se puede saber la IP real no se limita: así un proxy que no reenvía la IP nunca
// termina bloqueando a todos los clientes como si fueran uno solo.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv6Addr};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use axum::http::HeaderMap;

// Cada cuánto se limpian las IPs que ya recuperaron todos sus handshakes
const SWEEP_EVERY: Duration = Duration::from_secs(60);

// IP del cliente según el peer TCP y las cabeceras del proxy
pub fn client_ip(headers: &HeaderMap, peer: Option<IpAddr>) -> Option<IpAddr> {
    let peer = peer.map(|ip| ip.to_canonical());
    // Sin peer (pruebas en proceso) o desde la red interna: viene de nginx
    let behind_proxy = peer.is_none_or(is_internal);
    if !behind_proxy {
        return peer;
    }
    real_ip(headers).or_else(|| forwarded_for(headers))
}

fn real_ip(headers: &HeaderMap) -> Option<IpAddr> {
    let value = headers.get("x-real-ip")?.to_str().ok()?;
    parse_ip(value)
}

// nginx agrega la IP que ve al final con $proxy_add_x_forwarded_for: lo de la
// izquierda lo pudo escribir el cliente, así que solo vale el último
fn forwarded_for(headers: &HeaderMap) -> Option<IpAddr> {
    let last_header = headers.get_all("x-forwarded-for").iter().next_back()?;
    let last_entry = last_header.to_str().ok()?.rsplit(',').next()?;
    parse_ip(last_entry)
}

fn parse_ip(value: &str) -> Option<IpAddr> {
    value.trim().parse::<IpAddr>().ok().map(|ip| ip.to_canonical())
}

// Loopback, redes privadas y link-local: de ahí solo llega nginx (o el host)
fn is_internal(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local() || v4.is_unspecified(),
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                // fc00::/7 (unique local) y fe80::/10 (link-local)
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                || (v6.segments()[0] & 0xffc0) == 0xfe80
        }
    }
}

// Una IPv6 se agrupa por su /64: es lo que suele recibir un solo cliente
pub fn limit_key(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V4(_) => ip,
        IpAddr::V6(v6) => {
            let s = v6.segments();
            IpAddr::V6(Ipv6Addr::new(s[0], s[1], s[2], s[3], 0, 0, 0, 0))
        }
    }
}

// Cubeta de fichas por IP: `per_minute` de golpe y luego se recarga de a poco.
// Con per_minute = 0 no limita.
#[derive(Debug)]
pub struct HandshakeLimiter {
    per_minute: u32,
    state: Mutex<LimiterState>,
}

#[derive(Debug)]
struct LimiterState {
    buckets: HashMap<IpAddr, Bucket>,
    last_sweep: Instant,
}

#[derive(Debug, Clone, Copy)]
struct Bucket {
    tokens: f64,
    updated: Instant,
}

impl HandshakeLimiter {
    pub fn new(per_minute: u32) -> Self {
        Self {
            per_minute,
            state: Mutex::new(LimiterState {
                buckets: HashMap::new(),
                last_sweep: Instant::now(),
            }),
        }
    }

    pub fn allow(&self, ip: IpAddr) -> bool {
        self.allow_at(ip, Instant::now())
    }

    pub fn allow_at(&self, ip: IpAddr, now: Instant) -> bool {
        if self.per_minute == 0 {
            return true;
        }
        let capacity = f64::from(self.per_minute);
        let refill_per_sec = capacity / 60.0;
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);

        // Las IPs que ya se recargaron por completo no aportan nada: se olvidan
        if now.saturating_duration_since(state.last_sweep) >= SWEEP_EVERY {
            state.buckets.retain(|_, bucket| {
                let elapsed = now.saturating_duration_since(bucket.updated).as_secs_f64();
                bucket.tokens + elapsed * refill_per_sec < capacity
            });
            state.last_sweep = now;
        }

        let bucket = state.buckets.entry(limit_key(ip)).or_insert(Bucket {
            tokens: capacity,
            updated: now,
        });
        let elapsed = now.saturating_duration_since(bucket.updated).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * refill_per_sec).min(capacity);
        bucket.updated = now;
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    #[cfg(test)]
    fn tracked(&self) -> usize {
        self.state.lock().unwrap().buckets.len()
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

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn desde_nginx_se_usa_x_real_ip() {
        let h = headers(&[("x-real-ip", "200.1.2.3"), ("x-forwarded-for", "9.9.9.9, 200.1.2.4")]);
        assert_eq!(client_ip(&h, Some(ip("172.18.0.1"))), Some(ip("200.1.2.3")));
        assert_eq!(client_ip(&h, Some(ip("127.0.0.1"))), Some(ip("200.1.2.3")));
        assert_eq!(client_ip(&h, None), Some(ip("200.1.2.3")));
    }

    #[test]
    fn sin_x_real_ip_vale_el_ultimo_forwarded_for() {
        let h = headers(&[
            ("x-forwarded-for", "1.1.1.1, 2.2.2.2"),
            ("x-forwarded-for", "9.9.9.9, 200.1.2.4 "),
        ]);
        assert_eq!(client_ip(&h, Some(ip("10.0.0.5"))), Some(ip("200.1.2.4")));
    }

    #[test]
    fn un_peer_publico_no_puede_falsificar_la_ip() {
        let h = headers(&[("x-real-ip", "1.2.3.4")]);
        assert_eq!(client_ip(&h, Some(ip("200.9.9.9"))), Some(ip("200.9.9.9")));
    }

    #[test]
    fn desde_la_red_interna_sin_cabeceras_no_se_sabe() {
        assert_eq!(client_ip(&HeaderMap::new(), Some(ip("172.17.0.1"))), None);
        assert_eq!(client_ip(&HeaderMap::new(), None), None);
        let h = headers(&[("x-real-ip", "no-es-ip")]);
        assert_eq!(client_ip(&h, Some(ip("192.168.1.1"))), None);
    }

    #[test]
    fn ipv4_mapeada_en_ipv6_se_normaliza() {
        assert_eq!(
            client_ip(&HeaderMap::new(), Some(ip("::ffff:200.1.2.3"))),
            Some(ip("200.1.2.3"))
        );
        let h = headers(&[("x-real-ip", "::ffff:200.1.2.3")]);
        assert_eq!(client_ip(&h, Some(ip("::ffff:172.18.0.1"))), Some(ip("200.1.2.3")));
    }

    #[test]
    fn redes_internas() {
        for internal in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.0.10",
            "169.254.1.1",
            "::1",
            "fd00::1",
            "fe80::1",
        ] {
            assert!(is_internal(ip(internal)), "{internal}");
        }
        for public in ["200.1.2.3", "8.8.8.8", "2800:200::1"] {
            assert!(!is_internal(ip(public)), "{public}");
        }
    }

    #[test]
    fn ipv6_se_agrupa_por_64() {
        assert_eq!(limit_key(ip("2800:200:1:2:aaaa::1")), ip("2800:200:1:2::"));
        assert_eq!(limit_key(ip("200.1.2.3")), ip("200.1.2.3"));
    }

    #[test]
    fn rafaga_y_recarga() {
        let limiter = HandshakeLimiter::new(60);
        let t0 = Instant::now();
        let a = ip("200.1.2.3");
        for _ in 0..60 {
            assert!(limiter.allow_at(a, t0));
        }
        assert!(!limiter.allow_at(a, t0));
        // Otra IP no se ve afectada
        assert!(limiter.allow_at(ip("200.1.2.4"), t0));
        // 60 por minuto: una ficha por segundo
        assert!(!limiter.allow_at(a, t0 + Duration::from_millis(500)));
        assert!(limiter.allow_at(a, t0 + Duration::from_millis(1500)));
        assert!(!limiter.allow_at(a, t0 + Duration::from_millis(1600)));
    }

    #[test]
    fn la_misma_64_comparte_cubeta() {
        let limiter = HandshakeLimiter::new(2);
        let t0 = Instant::now();
        assert!(limiter.allow_at(ip("2800:200:1:2::1"), t0));
        assert!(limiter.allow_at(ip("2800:200:1:2::2"), t0));
        assert!(!limiter.allow_at(ip("2800:200:1:2::3"), t0));
        assert!(limiter.allow_at(ip("2800:200:1:3::1"), t0));
    }

    #[test]
    fn cero_desactiva_el_limite() {
        let limiter = HandshakeLimiter::new(0);
        let t0 = Instant::now();
        for _ in 0..1000 {
            assert!(limiter.allow_at(ip("200.1.2.3"), t0));
        }
    }

    #[test]
    fn las_ips_recargadas_se_olvidan() {
        let limiter = HandshakeLimiter::new(60);
        let t0 = Instant::now();
        limiter.allow_at(ip("200.1.2.3"), t0);
        limiter.allow_at(ip("200.1.2.4"), t0);
        assert_eq!(limiter.tracked(), 2);
        // Pasado el barrido, las dos ya recuperaron su ficha y solo queda la nueva
        limiter.allow_at(ip("200.1.2.5"), t0 + SWEEP_EVERY);
        assert_eq!(limiter.tracked(), 1);
    }
}
