# Native WebSocket Filter Service

Servicio backend en Node.js con WebSockets nativos que implementa un sistema de filtros para notificaciones dirigidas.

## Características

- ✅ WebSockets nativos (sin librerías externas)
- ✅ Sistema de filtros clave-valor
- ✅ Notificaciones dirigidas basadas en filtros
- ✅ API REST para enviar notificaciones
- ✅ Cliente web de prueba incluido
- ✅ Estadísticas de conexiones en tiempo real

## Instalación

\`\`\`bash
npm install
\`\`\`

## Uso

### Iniciar el servidor

\`\`\`bash
npm start
# o para desarrollo
npm run dev
\`\`\`

### Abrir cliente de prueba

Visita: http://localhost:3000

### Conectar cliente programáticamente

\`\`\`javascript
const ws = new WebSocket('ws://localhost:3000');

ws.onopen = function() {
    // Registrar filtros
    ws.send(JSON.stringify({
        type: 'register_filters',
        data: {
            business_id: 1,
            service_id: 2,
            user_type: 'admin'
        }
    }));
};

ws.onmessage = function(event) {
    const message = JSON.parse(event.data);
    if (message.type === 'notification') {
        console.log('Notificación:', message.data);
    }
};
\`\`\`

### Enviar notificaciones

\`\`\`bash
curl -X POST http://localhost:3000/notify \\
  -H "Content-Type: application/json" \\
  -d '{
    "filters": {"business_id": 1, "service_id": 2},
    "message": "Nuevo pedido recibido",
    "data": {"orderId": 12345}
  }'
\`\`\`

## Protocolo WebSocket

### Mensajes Cliente → Servidor

\`\`\`javascript
// Registrar filtros
{
  "type": "register_filters",
  "data": {"business_id": 1, "service_id": 2}
}

// Actualizar filtros
{
  "type": "update_filters", 
  "data": {"business_id": 1, "service_id": 3}
}

// Obtener filtros actuales
{
  "type": "get_filters",
  "data": {}
}

// Ping
{
  "type": "ping",
  "data": {}
}
\`\`\`

### Mensajes Servidor → Cliente

\`\`\`javascript
// Conexión establecida
{
  "type": "connected",
  "data": {"clientId": "abc123"},
  "timestamp": "2024-01-01T12:00:00.000Z"
}

// Notificación
{
  "type": "notification",
  "data": {
    "message": "Nuevo pedido",
    "data": {"orderId": 123},
    "filters": {"business_id": 1}
  },
  "timestamp": "2024-01-01T12:00:00.000Z"
}

// Error
{
  "type": "error",
  "data": {"message": "Filtros inválidos"},
  "timestamp": "2024-01-01T12:00:00.000Z"
}
\`\`\`

## API REST

- \`POST /notify\` - Enviar notificación
- \`GET /stats\` - Estadísticas de conexiones  
- \`POST /clients/filter\` - Obtener clientes por filtros
- \`GET /health\` - Estado del servidor

## Ejemplo de Filtros

Un cliente con filtros:
\`\`\`json
{"business_id": 1, "service_id": 2, "user_type": "admin"}
\`\`\`

Recibirá notificaciones enviadas con:
- \`{"business_id": 1}\` ✅
- \`{"business_id": 1, "service_id": 2}\` ✅  
- \`{"business_id": 1, "service_id": 3}\` ❌
- \`{"business_id": 2}\` ❌

## Archivos incluidos

- \`server.js\` - Servidor WebSocket principal
- \`public/index.html\` - Cliente web de prueba
- \`public/client.js\` - Lógica del cliente web
- \`client-example.js\` - Cliente Node.js de ejemplo
- \`notification-sender.js\` - Script para enviar notificaciones
