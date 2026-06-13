# WebSocket Filter Service

A Node.js backend service built with Socket.IO that enables targeted real-time notifications using a dynamic filter system. Clients subscribe to a service namespace and register key-value filters; the server delivers events only to clients whose filters match.

## How It Works

```
Client connects to /{service}
       ↓
Client emits register_filters({ business_id: 1, user_id: 42 })
       ↓
Backend stores client → filters mapping
       ↓
POST /emit { service, filters, eventType, data }
       ↓
Server finds clients where their filters ⊇ notification filters
       ↓
Matching clients receive the event
```

**Filter matching rule:** A client receives an event if **all** key-value pairs in the notification's `filters` are present in the client's registered filters. The client may have *more* filters, but none of the required ones can differ.

## Stack

- **Runtime:** Node.js
- **Framework:** Express
- **WebSocket:** Socket.IO
- **Namespaces:** Dynamic — one per service, created on first connection

---

## Installation

```bash
npm install
```

## Running

```bash
# Production
npm start

# Development (with auto-restart)
npm run dev
```

Server starts on port `3000` by default. Override with the `PORT` environment variable:

```bash
PORT=8080 npm start
```

---

## WebSocket Usage

### Connecting

Clients connect to a service-specific namespace:

```
ws://localhost:3000/{service}
```

Examples:
- `ws://localhost:3000/orders`
- `ws://localhost:3000/inventory`
- `ws://localhost:3000/notifications`

If the path is `/` (root), the service defaults to `atalaya`.

### JavaScript Client Example

```javascript
import { io } from 'socket.io-client';

const socket = io('http://localhost:3000/orders');

// 1. Register filters after connecting
socket.emit('register_filters', {
  business_id: 1,
  user_id: 42,
  role: 'admin'
});

// 2. Confirm registration
socket.on('filters_registered', (data) => {
  console.log('Registered:', data);
  // { message, service: 'orders', filters: { service, business_id, user_id, role } }
});

// 3. Listen for application events (any eventType you define)
socket.on('new_order', (data) => {
  console.log('New order:', data);
});

socket.on('status_update', (data) => {
  console.log('Status changed:', data);
});
```

### Client Events Reference

#### Emitted by client → received by server

| Event | Payload | Description |
|-------|---------|-------------|
| `register_filters` | `{ key: value, ... }` | Register this client's filters. The `service` key is added automatically. |
| `update_filters` | `{ key: value, ... }` | Replace all filters for this client. |
| `get_filters` | _(none)_ | Request the currently registered filters. |

#### Emitted by server → received by client

| Event | Payload | Description |
|-------|---------|-------------|
| `filters_registered` | `{ message, service, filters }` | Confirmation after `register_filters`. |
| `filters_updated` | `{ message, service, filters }` | Confirmation after `update_filters`. |
| `current_filters` | `{ service, filters }` | Response to `get_filters`. |
| `error` | `{ message }` | Validation error (e.g. invalid filters object). |
| _(your eventType)_ | _(your data)_ | Application events triggered via `POST /emit`. |

---

## REST API

### `POST /emit`

Sends an event to all clients in a service whose filters match.

**Request body:**

```json
{
  "service": "orders",
  "filters": { "business_id": 1 },
  "eventType": "new_order",
  "data": { "orderId": 99, "total": 250.00 }
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `service` | string | Yes | Target service namespace. |
| `filters` | object | Yes | Key-value pairs — only clients whose filters contain all of these will receive the event. |
| `eventType` | string | Yes | The Socket.IO event name clients should listen for. |
| `data` | object | No | Payload delivered to the client. Defaults to `{}`. |

**Response:**

```json
{
  "success": true,
  "message": "Evento enviado",
  "service": "orders",
  "eventType": "new_order",
  "clientsNotified": 3,
  "filters": { "service": "orders", "business_id": 1 }
}
```

---

### `GET /stats`

Returns a snapshot of all connected clients grouped by service.

**Response:**

```json
{
  "totalServices": 2,
  "totalClients": 5,
  "services": {
    "orders": {
      "connectedClients": 3,
      "clients": [
        {
          "socketId": "abc123",
          "filters": { "service": "orders", "business_id": 1, "user_id": 42 },
          "connectedAt": "2024-01-15T10:30:00.000Z"
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
  "service": "orders",
  "filters": { "business_id": 1 }
}
```

**Response:**

```json
{
  "service": "orders",
  "matchingClients": 2,
  "clients": [
    {
      "socketId": "abc123",
      "filters": { "service": "orders", "business_id": 1, "user_id": 42 }
    }
  ]
}
```

---

### `GET /health`

Health check endpoint.

**Response:**

```json
{
  "status": "OK",
  "timestamp": "2024-01-15T10:30:00.000Z",
  "totalServices": 2,
  "totalClients": 5
}
```

---

## Filter Matching Examples

A client connected to `/orders` with filters `{ business_id: 1, user_id: 42, role: "admin" }`:

| Notification filters | Delivered? | Reason |
|----------------------|------------|--------|
| `{ business_id: 1 }` | ✅ | All notification keys match |
| `{ business_id: 1, user_id: 42 }` | ✅ | All notification keys match |
| `{ business_id: 1, user_id: 42, role: "admin" }` | ✅ | Exact match |
| `{ business_id: 2 }` | ❌ | `business_id` differs |
| `{ business_id: 1, user_id: 99 }` | ❌ | `user_id` differs |
| `{ business_id: 1, store_id: 5 }` | ❌ | Client has no `store_id` filter |

Events sent to a **different service** (e.g. `/inventory`) are never delivered to `/orders` clients.

---

## Architecture Notes

- **Namespaces are created dynamically** via a regex pattern — no need to pre-register services.
- **In-memory storage** — connected clients and their filters are stored in a `Map`. Data is lost on server restart; this is by design for ephemeral WebSocket state.
- **Service isolation** — each service namespace has its own client map. Emitting to `orders` never touches `inventory` clients.
- **Stateless REST layer** — any backend service can trigger events via `POST /emit` without maintaining a WebSocket connection.
