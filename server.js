const express = require('express')
const http = require('http')
const WebSocket = require('ws')
const cors = require('cors')
const path = require('path')

const app = express()
const server = http.createServer(app)

// Middleware
app.use(cors({
  origin: '*', // Allow all origins
  methods: ['GET', 'POST', 'PUT', 'DELETE', 'OPTIONS'], // Allow all methods
  allowedHeaders: '*' // Allow all headers
}))
app.use(express.json())
app.use(express.static('public'))
// Crear servidor WebSocket
const wss = new WebSocket.Server({ server })

// Almacenar conexiones con sus filtros
const connectedClients = new Map()

// Función para comparar filtros
function filtersMatch(clientFilters, notificationFilters) {
  // Verificar que todos los filtros de la notificación coincidan con los del cliente
  for (const [key, value] of Object.entries(notificationFilters)) {
    if (clientFilters[key] !== value) {
      return false
    }
  }
  return true
}

// Función para obtener clientes que coincidan con los filtros
function getMatchingClients(filters) {
  const matchingClients = []

  for (const [ws, clientData] of connectedClients.entries()) {
    if (ws.readyState === WebSocket.OPEN && filtersMatch(clientData.filters, filters)) {
      matchingClients.push({
        ws,
        filters: clientData.filters,
        id: clientData.id
      })
    }
  }

  return matchingClients
}

// Función para enviar mensaje a un cliente
function sendMessage(ws, type, data) {
  if (ws.readyState === WebSocket.OPEN) {
    ws.send(JSON.stringify({ type, data, timestamp: new Date().toISOString() }))
  }
}

// Función para generar ID único
function generateId() {
  return Math.random().toString(36).substr(2, 9)
}

// Configuración de WebSocket
wss.on('connection', (ws) => {
  const clientId = generateId()
  console.log(`Cliente conectado: ${clientId}`)

  // Inicializar datos del cliente
  connectedClients.set(ws, {
    id: clientId,
    filters: {},
    connectedAt: new Date()
  })

  // Enviar confirmación de conexión
  sendMessage(ws, 'connected', { clientId })

  // Manejar mensajes del cliente
  ws.on('message', (message) => {
    try {
      const parsedMessage = JSON.parse(message.toString())
      const { type, data } = parsedMessage

      switch (type) {
        case 'register_filters':
          handleRegisterFilters(ws, data)
          break

        case 'update_filters':
          handleUpdateFilters(ws, data)
          break

        case 'get_filters':
          handleGetFilters(ws)
          break

        case 'ping':
          sendMessage(ws, 'pong', { timestamp: new Date().toISOString() })
          break

        default:
          sendMessage(ws, 'error', { message: `Tipo de mensaje desconocido: ${type}` })
      }
    } catch (error) {
      console.error('Error procesando mensaje:', error)
      sendMessage(ws, 'error', { message: 'Mensaje inválido' })
    }
  })

  // Manejar desconexión
  ws.on('close', () => {
    console.log(`Cliente desconectado: ${clientId}`)
    connectedClients.delete(ws)
  })

  // Manejar errores
  ws.on('error', (error) => {
    console.error(`Error en cliente ${clientId}:`, error)
    connectedClients.delete(ws)
  })
})

// Handlers para diferentes tipos de mensajes
function handleRegisterFilters(ws, filters) {
  if (typeof filters !== 'object' || filters === null) {
    sendMessage(ws, 'error', { message: 'Los filtros deben ser un objeto válido' })
    return
  }

  const clientData = connectedClients.get(ws)
  if (clientData) {
    clientData.filters = filters
    connectedClients.set(ws, clientData)

    console.log(`Cliente ${clientData.id} registrado con filtros:`, filters)
    sendMessage(ws, 'filters_registered', { 
      message: 'Filtros registrados correctamente',
      filters: filters 
    })
  }
}

function handleUpdateFilters(ws, newFilters) {
  if (typeof newFilters !== 'object' || newFilters === null) {
    sendMessage(ws, 'error', { message: 'Los filtros deben ser un objeto válido' })
    return
  }

  const clientData = connectedClients.get(ws)
  if (clientData) {
    clientData.filters = newFilters
    connectedClients.set(ws, clientData)

    console.log(`Cliente ${clientData.id} actualizó filtros:`, newFilters)
    sendMessage(ws, 'filters_updated', {
      message: 'Filtros actualizados correctamente',
      filters: newFilters
    })
  }
}

function handleGetFilters(ws) {
  const clientData = connectedClients.get(ws)
  if (clientData) {
    sendMessage(ws, 'current_filters', { filters: clientData.filters })
  } else {
    sendMessage(ws, 'error', { message: 'No hay filtros registrados para este cliente' })
  }
}

// Endpoint REST para enviar notificaciones
app.post('/notify', (req, res) => {
  const { filters, message, data } = req.body

  // Validaciones
  if (!filters || typeof filters !== 'object') {
    return res.status(400).json({
      error: 'Se requieren filtros válidos (objeto clave-valor)'
    })
  }

  if (!message) {
    return res.status(400).json({
      error: 'Se requiere un mensaje'
    })
  }

  // Obtener clientes que coincidan con los filtros
  const matchingClients = getMatchingClients(filters)

  console.log(`Enviando notificación con filtros:`, filters)
  console.log(`Clientes que coinciden: ${matchingClients.length}`)

  // Enviar notificación a clientes que coincidan
  const notification = {
    message,
    data: data || {},
    filters,
    timestamp: new Date().toISOString()
  }

  let sentCount = 0
  matchingClients.forEach((client) => {
    sendMessage(client.ws, 'notification', notification)
    console.log(`Notificación enviada a cliente ${client.id} con filtros:`, client.filters)
    sentCount++
  })

  res.json({
    success: true,
    message: 'Notificación enviada',
    clientsNotified: sentCount,
    filters: filters
  })
})

// Endpoint para obtener estadísticas
app.get('/stats', (req, res) => {
  const stats = {
    connectedClients: connectedClients.size,
    clients: []
  }

  for (const [ws, clientData] of connectedClients.entries()) {
    if (ws.readyState === WebSocket.OPEN) {
      stats.clients.push({
        id: clientData.id,
        filters: clientData.filters,
        connectedAt: clientData.connectedAt
      })
    }
  }

  res.json(stats)
})

// Endpoint para obtener clientes por filtros específicos
app.post('/clients/filter', (req, res) => {
  const { filters } = req.body

  if (!filters || typeof filters !== 'object') {
    return res.status(400).json({
      error: 'Se requieren filtros válidos (objeto clave-valor)'
    })
  }

  const matchingClients = getMatchingClients(filters)

  res.json({
    matchingClients: matchingClients.length,
    clients: matchingClients.map((client) => ({
      id: client.id,
      filters: client.filters
    }))
  })
})

// Endpoint de salud
app.get('/health', (req, res) => {
  res.json({
    status: 'OK',
    timestamp: new Date().toISOString(),
    connectedClients: connectedClients.size
  })
})

// Servir página de prueba
app.get('/', (req, res) => {
  res.sendFile(path.join(__dirname, 'public', 'index.html'))
})

const PORT = process.env.PORT || 3000

server.listen(PORT, () => {
  console.log(`Servidor WebSocket ejecutándose en puerto ${PORT}`)
  console.log(`Página de prueba: http://localhost:${PORT}`)
  console.log(`Endpoints disponibles:`)
  console.log(`- POST /notify - Enviar notificaciones`)
  console.log(`- GET /stats - Estadísticas de conexiones`)
  console.log(`- POST /clients/filter - Obtener clientes por filtros`)
  console.log(`- GET /health - Estado del servidor`)
})
