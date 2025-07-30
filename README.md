# WebSocket Filter Service

Servicio backend en Node.js con Socket.IO que implementa un sistema de filtros para notificaciones dirigidas.

## Características

- ✅ Conexiones WebSocket con Socket.IO
- ✅ Sistema de filtros clave-valor
- ✅ Notificaciones dirigidas basadas en filtros
- ✅ API REST para enviar notificaciones
- ✅ Estadísticas de conexiones
- ✅ Actualización dinámica de filtros

## Instalación

```bash
npm install
```

## Uso

### Iniciar el servidor

```bash
npm start
# o para desarrollo
npm run dev
```

### Conectar un cliente por path

```javascript
const io = require('socket.io-client');
// Conectar al servicio específico
const socket = io('http://localhost:3000/orders');

// Registrar filtros adicionales (el service ya está implícito)
socket.emit('register_filters', {
  business_id: 1,
  user_id: 123,
  user_type: 'admin'
});

// Escuchar eventos específicos
socket.on('notification', (data) => {
  console.log('Notificación:', data);
});

socket.on('message', (data) => {
  console.log('Mensaje:', data);
});
```

### Enviar eventos

```bash
curl -X POST http://localhost:3000/emit \
  -H "Content-Type: application/json" \
  -d '{
    "service": "orders",
    "filters": {"business_id": 1},
    "eventType": "notification",
    "data": {"title": "Nuevo pedido", "orderId": 12345}
  }'
```

## API Endpoints

- `POST /emit` - Enviar evento a un servicio específico
- `GET /stats` - Estadísticas de conexiones por servicio
- `POST /clients/filter` - Obtener clientes por filtros en un servicio
- `GET /health` - Estado del servidor

## Eventos Socket.IO

### Cliente → Servidor
- `register_filters` - Registrar filtros del cliente
- `update_filters` - Actualizar filtros
- `get_filters` - Obtener filtros actuales

### Servidor → Cliente
- `notification` - Notificación recibida
- `filters_registered` - Confirmación de registro
- `filters_updated` - Confirmación de actualización
- `error` - Mensaje de error

## Estructura de conexión

Los clientes se conectan a: `ws://localhost:3000/{service}`

Ejemplos:
- `ws://localhost:3000/orders`
- `ws://localhost:3000/inventory`
- `ws://localhost:3000/notifications`

## Ejemplo de Filtros

Un cliente conectado a `/orders` con filtros:
```json
{"business_id": 1, "user_id": 123}
```

Recibirá eventos enviados a servicio `orders` con:
- `{"business_id": 1}` ✅
- `{"business_id": 1, "user_id": 123}` ✅
- `{"business_id": 2}` ❌
- Eventos enviados a otros servicios ❌
