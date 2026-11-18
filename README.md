# WebSocket Filter Service

A Rust service that speaks the Socket.IO protocol and delivers targeted real-time notifications using a filter system. Clients connect to a service namespace and register key-value filters; backends call `POST /emit` and the server delivers the event only to clients whose filters match.

Browsers connect with the standard `socket.io-client` 4.x and backends call plain HTTP endpoints, so neither side needs anything Rust-specific.

## How It Works

```
Client connects to /{service} with a signed token
       ↓
Client emits register_filters({ business_id: 1, user_id: 42 })
       ↓
Server stores client → filters mapping
       ↓
POST /emit { service, filters, eventType, data }
       ↓
Server finds clients where their filters ⊇ notification filters
       ↓
Matching clients receive the event
```

**Filter matching rule:** A client receives an event if **all** key-value pairs in the notification's `filters` are present in the client's registered filters. The client may have *more* filters, but none of the required ones can differ.

## Security

Only DevEx services may use this server. It refuses to start without `EVENTS_SECRET`, which lives in `.env` (never committed) and is shared with the backends.

| Who | Requirement |
|---|---|
| `POST /emit`, `GET /stats`, `POST /clients/filter` | `Authorization: Bearer <EVENTS_SECRET>` |
| Socket connections | `Origin` in `ALLOWED_ORIGINS`, a namespace in `ALLOWED_SERVICES`, and a signed token in `auth.token` |
| `GET /health` | Public, returns only `{"status":"OK"}` |

The socket token is `<expires unix seconds>.<hex hmac-sha256 of "service.expires" with EVENTS_SECRET>`. Each backend signs it for its logged-in users and it is only checked when connecting, so keep it short-lived and fetch a new one on each connection:

```javascript
const socket = io('https://events.devex.pe/devex', {
  auth: (send) => fetch('/api/events/token').then(r => r.json()).then(r => send({ token: r.data.token })),
});
```

The port is published only on `127.0.0.1`; nginx is the public entry point. Events can reach anyone holding a valid token, so send signals rather than sensitive data.

**Deploy:** a push to `main` fires the deploy hook of the DevEx panel (Servicios Docker). Its runner resets the server checkout to `origin/main`, runs `docker compose up -d --build` and waits for the healthcheck before reporting the result. There is no CI: run the tests (see Development) before pushing.

## Configuration

| Variable | Default | Description |
|---|---|---|
| `EVENTS_SECRET` | _(required)_ | Shared secret. Without it the server exits with code 1. |
| `ALLOWED_SERVICES` | `devex` | Comma-separated list. One Socket.IO namespace per service. |
| `ALLOWED_ORIGINS` | `https://devex.pe` | Comma-separated list of browser origins allowed to open a socket. |
| `PORT` | `3000` | HTTP port. |

## Stack

- **Language:** Rust (edition 2024, toolchain pinned in the `Dockerfile`)
- **HTTP:** axum on tokio (single-threaded runtime)
- **Socket.IO:** socketioxide (Socket.IO protocol v5 / Engine.IO v4, compatible with `socket.io-client` 4.x)
- **Image:** static musl binary on Alpine, runs as a non-root user (~17 MB image)

---

## Development

No local Rust install is needed: everything runs in the official Rust image. Named volumes keep the crate registry and build cache between runs.

```bash
docker run --rm -v "$PWD:/app" -v events-cargo-registry:/usr/local/cargo/registry -v events-cargo-target:/app/target -w /app rust:1.100.0-alpine3.24 sh -c "apk add --no-cache musl-dev >/dev/null && rustup component add clippy rustfmt >/dev/null && cargo fmt --check && cargo clippy --locked --all-targets -- -D warnings && cargo test --locked"
```

On Git Bash for Windows, prefix the command with `MSYS_NO_PATHCONV=1`.

With a local toolchain it is the usual:

```bash
cargo test
EVENTS_SECRET=dev cargo run
```

## Running

```bash
docker compose up -d --build
```

The image builds in two layers: dependencies first (cached while `Cargo.toml` and `Cargo.lock` don't change), then the service code. Build parallelism is limited to 2 jobs so it does not exhaust the server's RAM; change it with `--build-arg CARGO_BUILD_JOBS=4`.

On `SIGTERM` (e.g. `docker stop`) the server closes every socket and exits in about one second.

---

## WebSocket Usage

### Connecting

Clients connect to a service namespace, sending an allowed `Origin` (browsers do it automatically) and a signed token:

```
wss://events.devex.pe/{service}
```

- Unknown namespaces are rejected with `Invalid namespace`.
- The root namespace `/` is not used: connecting to it fails with `No autorizado`.
- A missing, expired or wrongly signed token fails with `No autorizado`.

### JavaScript Client Example

```javascript
import { io } from 'socket.io-client';

const socket = io('https://events.devex.pe/devex', {
  auth: (send) => fetch('/api/events/token').then(r => r.json()).then(r => send({ token: r.data.token })),
});

// 1. Register filters after connecting
socket.on('connect', () => {
  socket.emit('register_filters', { environment: 'production', user_id: 42 });
});

// 2. Confirm registration
socket.on('filters_registered', (data) => {
  console.log('Registered:', data);
  // { message, service: 'devex', filters: { service, environment, user_id } }
});

// 3. Listen for application events (any eventType your backend emits)
socket.on('activities_changed', (data) => {
  console.log('Activities changed:', data);
});
```

### Client Events Reference

#### Emitted by client → received by server

| Event | Payload | Description |
|-------|---------|-------------|
| `register_filters` | `{ key: value, ... }` | Register this client's filters. The `service` key is added automatically. |
| `update_filters` | `{ key: value, ... }` | Replace all filters for this client. Ignored if the client never registered. |
| `get_filters` | _(none)_ | Request the currently registered filters. |

#### Emitted by server → received by client

| Event | Payload | Description |
|-------|---------|-------------|
| `filters_registered` | `{ message, service, filters }` | Confirmation after `register_filters`. |
| `filters_updated` | `{ message, service, filters }` | Confirmation after `update_filters`. |
| `current_filters` | `{ service, filters }` | Response to `get_filters`. |
| `error` | `{ message }` | Invalid filters (not an object) or `get_filters` before registering. |
| _(your eventType)_ | _(your data)_ | Application events triggered via `POST /emit`. |

---

## REST API

### `POST /emit`

Sends an event to all clients in a service whose filters match.

**Request body:**

```json
{
  "service": "devex",
  "filters": { "environment": "production" },
  "eventType": "activities_changed",
  "data": { "id": 99 }
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `service` | string | Yes | Must be in `ALLOWED_SERVICES`. |
| `filters` | object | Yes | Key-value pairs — only clients whose filters contain all of these will receive the event. |
| `eventType` | string | Yes | The Socket.IO event name clients listen for. |
| `data` | any | No | Payload delivered to the client as a single argument. Missing or falsy (`null`, `false`, `0`, `""`) sends `{}`. |

**Response:**

```json
{
  "success": true,
  "message": "Evento enviado",
  "service": "devex",
  "eventType": "activities_changed",
  "clientsNotified": 3,
  "filters": { "service": "devex", "environment": "production" }
}
```

Validation errors answer `400` with `{ "error": "..." }`. The body must be `application/json` (up to 100kb).

---

### `GET /stats`

Returns a snapshot of all connected clients grouped by service.

**Response:**

```json
{
  "totalServices": 1,
  "totalClients": 1,
  "services": {
    "devex": {
      "connectedClients": 1,
      "clients": [
        {
          "socketId": "Kx2ZqyPd0Gm1u1aT",
          "filters": { "service": "devex", "environment": "production", "user_id": 42 },
          "connectedAt": "2026-11-18T10:30:00.000Z"
        }
      ]
    }
  }
}
```

---

### `POST /clients/filter`

Lists all clients in a service that match the given filters (without sending an event).

**Request body:**

```json
{
  "service": "devex",
  "filters": { "environment": "production" }
}
```

**Response:**

```json
{
  "service": "devex",
  "matchingClients": 1,
  "clients": [
    {
      "socketId": "Kx2ZqyPd0Gm1u1aT",
      "filters": { "service": "devex", "environment": "production", "user_id": 42 }
    }
  ]
}
```

---

### `GET /health`

Health check endpoint, used by the Docker healthcheck.

**Response:**

```json
{ "status": "OK" }
```

---

## Filter Matching Examples

A client connected to `/devex` with filters `{ business_id: 1, user_id: 42, role: "admin" }`:

| Notification filters | Delivered? | Reason |
|----------------------|------------|--------|
| `{ business_id: 1 }` | ✅ | All notification keys match |
| `{ business_id: 1, user_id: 42 }` | ✅ | All notification keys match |
| `{ business_id: 1, user_id: 42, role: "admin" }` | ✅ | Exact match |
| `{ business_id: 1.0 }` | ✅ | Numbers compare by value |
| `{ business_id: "1" }` | ❌ | `"1"` is a string, not the number `1` |
| `{ business_id: 2 }` | ❌ | `business_id` differs |
| `{ business_id: 1, store_id: 5 }` | ❌ | Client has no `store_id` filter |
| `{ meta: { a: 1 } }` | ❌ | Object or array values never match |

Events sent to a **different service** are never delivered to `/devex` clients.

---

## Architecture Notes

- **One namespace per allowed service**, registered at startup from `ALLOWED_SERVICES`.
- **JavaScript semantics kept on purpose:** filters are merged as `{ service, ...filters }` (a `service` key inside `filters` overrides it), comparisons follow `===`, and JSON keys keep their insertion order.
- **In-memory storage** — connected clients and their filters live in ordered maps. Data is lost on restart; this is by design for ephemeral WebSocket state.
- **Service isolation** — each service has its own client map. Emitting to one service never touches another.
- **One encode per event** — each socket joins a room named after its id, so `/emit` encodes the packet once and sends it to every matching socket.
- **Stateless REST layer** — any backend can trigger events via `POST /emit` without keeping a WebSocket connection.
