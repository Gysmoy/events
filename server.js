const express = require("express")
const http = require("http")
const socketIo = require("socket.io")
const cors = require("cors")

const app = express()
const server = http.createServer(app)
const io = socketIo(server, {
    cors: {
        origin: "*",
        methods: ["GET", "POST"],
    },
})

// Middleware
app.use(cors())
app.use(express.json())

// Almacenar conexiones con sus filtros organizadas por servicio
const connectedClients = new Map() // service -> Map(socketId -> clientData)

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

// Función para obtener clientes que coincidan con los filtros en un servicio específico
function getMatchingClients(service, filters) {
    const matchingClients = []
    const serviceClients = connectedClients.get(service)

    if (!serviceClients) {
        return matchingClients
    }

    for (const [socketId, clientData] of serviceClients.entries()) {
        if (filtersMatch(clientData.filters, filters)) {
            matchingClients.push({
                socketId,
                socket: clientData.socket,
                filters: clientData.filters,
            })
        }
    }

    return matchingClients
}

// Función para inicializar un servicio si no existe
function initializeService(service) {
    if (!connectedClients.has(service)) {
        connectedClients.set(service, new Map())
    }
}

// Configurar namespace dinámico para cada servicio
io.of(/^\/(\w+)?$/).on("connection", (socket) => {
    // Extraer el nombre del servicio del namespace
    const service = socket.nsp.name.substring(1) // Remover el '/' inicial

    console.log(`Cliente conectado al servicio '${service}': ${socket.id}`)

    // Inicializar el servicio si no existe
    initializeService(service)

    // Evento para registrar filtros adicionales del cliente
    socket.on("register_filters", (filters) => {
        console.log(`Cliente ${socket.id} en servicio '${service}' registrado con filtros:`, filters)

        // Validar que filters sea un objeto
        if (typeof filters !== "object" || filters === null) {
            socket.emit("error", { message: "Los filtros deben ser un objeto válido" })
            return
        }

        // Los filtros incluyen automáticamente el servicio
        const completeFilters = {
            service: service,
            ...filters,
        }

        // Almacenar la conexión con sus filtros
        const serviceClients = connectedClients.get(service)
        serviceClients.set(socket.id, {
            socket: socket,
            filters: completeFilters,
            connectedAt: new Date(),
        })

        socket.emit("filters_registered", {
            message: "Filtros registrados correctamente",
            service: service,
            filters: completeFilters,
        })
    })

    // Evento para actualizar filtros
    socket.on("update_filters", (newFilters) => {
        console.log(`Cliente ${socket.id} en servicio '${service}' actualizando filtros:`, newFilters)

        if (typeof newFilters !== "object" || newFilters === null) {
            socket.emit("error", { message: "Los filtros deben ser un objeto válido" })
            return
        }

        const serviceClients = connectedClients.get(service)
        if (serviceClients && serviceClients.has(socket.id)) {
            const completeFilters = {
                service: service,
                ...newFilters,
            }

            const clientData = serviceClients.get(socket.id)
            clientData.filters = completeFilters
            serviceClients.set(socket.id, clientData)

            socket.emit("filters_updated", {
                message: "Filtros actualizados correctamente",
                service: service,
                filters: completeFilters,
            })
        }
    })

    // Evento para obtener filtros actuales
    socket.on("get_filters", () => {
        const serviceClients = connectedClients.get(service)
        const clientData = serviceClients?.get(socket.id)

        if (clientData) {
            socket.emit("current_filters", {
                service: service,
                filters: clientData.filters,
            })
        } else {
            socket.emit("error", { message: "No hay filtros registrados para este cliente" })
        }
    })

    // Manejar desconexión
    socket.on("disconnect", () => {
        console.log(`Cliente desconectado del servicio '${service}': ${socket.id}`)
        const serviceClients = connectedClients.get(service)
        if (serviceClients) {
            serviceClients.delete(socket.id)

            // Si no quedan clientes en el servicio, limpiar el servicio
            if (serviceClients.size === 0) {
                connectedClients.delete(service)
            }
        }
    })
})

// Endpoint REST para enviar eventos
app.post("/emit", (req, res) => {
    const { service, filters, eventType, data } = req.body

    // Validaciones
    if (!service || typeof service !== "string") {
        return res.status(400).json({
            error: "Se requiere un nombre de servicio válido",
        })
    }

    if (!filters || typeof filters !== "object") {
        return res.status(400).json({
            error: "Se requieren filtros válidos (objeto clave-valor)",
        })
    }

    if (!eventType || typeof eventType !== "string") {
        return res.status(400).json({
            error: "Se requiere un identificador de evento (eventType)",
        })
    }

    // Los filtros deben incluir el servicio
    const completeFilters = {
        service: service,
        ...filters,
    }

    // Obtener clientes que coincidan con los filtros en el servicio específico
    const matchingClients = getMatchingClients(service, completeFilters)

    console.log(`Enviando evento '${eventType}' al servicio '${service}' con filtros:`, completeFilters)
    console.log(`Clientes que coinciden: ${matchingClients.length}`)

    // Enviar evento a clientes que coincidan (solo eventType y data)
    matchingClients.forEach((client) => {
        client.socket.emit(eventType, data || {})
        console.log(`Evento '${eventType}' enviado a cliente ${client.socketId} con filtros:`, client.filters)
    })

    res.json({
        success: true,
        message: "Evento enviado",
        service: service,
        eventType: eventType,
        clientsNotified: matchingClients.length,
        filters: completeFilters,
    })
})

// Endpoint para obtener estadísticas
app.get("/stats", (req, res) => {
    const stats = {
        totalServices: connectedClients.size,
        totalClients: 0,
        services: {},
    }

    for (const [service, serviceClients] of connectedClients.entries()) {
        stats.totalClients += serviceClients.size
        stats.services[service] = {
            connectedClients: serviceClients.size,
            clients: [],
        }

        for (const [socketId, clientData] of serviceClients.entries()) {
            stats.services[service].clients.push({
                socketId,
                filters: clientData.filters,
                connectedAt: clientData.connectedAt,
            })
        }
    }

    res.json(stats)
})

// Endpoint para obtener clientes por filtros específicos en un servicio
app.post("/clients/filter", (req, res) => {
    const { service, filters } = req.body

    if (!service || typeof service !== "string") {
        return res.status(400).json({
            error: "Se requiere un nombre de servicio válido",
        })
    }

    if (!filters || typeof filters !== "object") {
        return res.status(400).json({
            error: "Se requieren filtros válidos (objeto clave-valor)",
        })
    }

    const completeFilters = {
        service: service,
        ...filters,
    }

    const matchingClients = getMatchingClients(service, completeFilters)

    res.json({
        service: service,
        matchingClients: matchingClients.length,
        clients: matchingClients.map((client) => ({
            socketId: client.socketId,
            filters: client.filters,
        })),
    })
})

// Endpoint de salud
app.get("/health", (req, res) => {
    let totalClients = 0
    for (const [, serviceClients] of connectedClients.entries()) {
        totalClients += serviceClients.size
    }

    res.json({
        status: "OK",
        timestamp: new Date().toISOString(),
        totalServices: connectedClients.size,
        totalClients: totalClients,
    })
})

const PORT = process.env.PORT || 3000

server.listen(PORT, () => {
    console.log(`Servidor WebSocket ejecutándose en puerto ${PORT}`)
    console.log(`Conexiones WebSocket: ws://localhost:${PORT}/{service}`)
    console.log(`Endpoints disponibles:`)
    console.log(`- POST /emit - Enviar eventos`)
    console.log(`- GET /stats - Estadísticas de conexiones`)
    console.log(`- POST /clients/filter - Obtener clientes por filtros`)
    console.log(`- GET /health - Estado del servidor`)
})
